//! Persisted review intent is not a data-plane import capability or receipt.
use super::*;
use crate::engine_backend::foreign_plan::ForeignDestination;
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use storage_contract::{ErrorCode, OperationId, storage_foreign::*, storage_foreign_plan::*};

mod intent;
mod persist;
use persist::{StoredPlan, open, owned, read, store};

const DIRECTORY: &str = "foreign-reconstruction-plans";
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

fn digest(preview: &ForeignPlanPreview) -> String {
    let mut review = preview.clone();
    review.review_digest.clear();
    storage_addition::digest(&review)
}

fn validate_review(preview: &ForeignPlanPreview) -> Result<(), ApiError> {
    if digest(preview) != preview.review_digest
        || preview.request.source != preview.source.source
        || preview.request.source_review_digest != preview.source.review_digest
        || preview.target.source_digest != preview.source.review_digest
    {
        return Err(ApiError::conflict(
            ErrorCode::PlanStale,
            "foreign plan does not bind its complete source and target review",
        ));
    }
    Ok(())
}

fn mapping_response(mapping: &storage_engine::ForeignImportMapping) -> ForeignTargetMapping {
    let target = mapping.target();
    ForeignTargetMapping {
        identity: ForeignDiskIdentity {
            pool_id: target.pool_id.clone(),
            disk_id: target.disk_id.clone(),
            slot_id: target.slot_id.clone(),
            binding_generation: target.binding_generation,
        },
        sequence: mapping.target_sequence(),
        source_digest: mapping.source_digest().to_string(),
        mapping_digest: mapping.digest().to_string(),
        histories: mapping
            .histories()
            .iter()
            .map(|history| ForeignMappedHistory {
                group_id: history.group_id().into(),
                history_tree_id: history.history_tree_id().into(),
                destination: history.path().as_str().into(),
            })
            .collect(),
    }
}

impl ManagementBackend {
    pub(super) fn foreign_plan_operation(
        &self,
        action: &str,
        input: Value,
        identity: &Identity,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        match action {
            "foreign_intent_preview" | "foreign_intent_save" | "foreign_intent_status" => {
                self.foreign_intent_operation(action, input, identity)
            }
            "foreign_plan_preview" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let preview = self.preview_foreign_plan(identity, request)?;
                Ok(json_codec::to_value(preview).expect("invariant: foreign review serializes"))
            }
            "foreign_plan_save" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let status = self.save_foreign_plan(identity, request)?;
                Ok(
                    json_codec::to_value(status)
                        .expect("invariant: foreign plan status serializes"),
                )
            }
            "foreign_plan_status" => {
                #[derive(data_serialization::Deserialize)]
                #[serialization(deny_unknown_fields)]
                struct Query {
                    operation_id: OperationId,
                }
                let request: Query =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let record = self.load_foreign_plan(identity, request.operation_id)?;
                Ok(
                    json_codec::to_value(record.status(ForeignPlanValidation::Required))
                        .expect("invariant: foreign plan status serializes"),
                )
            }
            "foreign_plan_revalidate" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let status = self.revalidate_foreign_plan(identity, request)?;
                Ok(
                    json_codec::to_value(status)
                        .expect("invariant: foreign plan status serializes"),
                )
            }
            "foreign_plan_execute" => {
                let request: ForeignPlanSelection =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let record = self.load_foreign_plan(identity, request.operation_id)?;
                record.require_digest(&request.review_digest)?;
                // No DTO or saved plan can manufacture an engine write capability.
                Err(ApiError::conflict(
                    ErrorCode::FormatUnsupported,
                    "foreign import execution is unavailable; the saved review does not authorize disk admission, pruning or data writes",
                ))
            }
            _ => Err(ApiError::invalid("unknown foreign plan operation")),
        }
    }

    fn preview_foreign_plan(
        &self,
        identity: &Identity,
        request: ForeignPlanRequest,
    ) -> Result<ForeignPlanPreview, ApiError> {
        let reviewed: storage_engine::ForeignEvidenceDigest = request
            .source_review_digest
            .parse()
            .map_err(ApiError::invalid_source)?;
        let target = ForeignDestination::try_from(&request.target)?;
        let backend = self.inner.backend()?;
        let source = self.inspect_foreign_source(identity, &request.source)?;
        if source.digest() != &reviewed {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "foreign source evidence differs from the inspected review",
            ));
        }
        let mapping = backend.map_foreign_plan(identity, &source, &target)?;
        let mut preview = ForeignPlanPreview {
            source: foreign_preview::response(request.source.clone(), &source),
            target: mapping_response(&mapping),
            request,
            review_digest: String::new(),
            execution: ForeignExecutionCapability::Unavailable,
            blocker: ForeignPlanBlocker::PortableImportWriterUnavailable,
        };
        preview.review_digest = digest(&preview);
        crate::history_window_encode::encode_bounded(&preview, crate::JSON_BODY_LIMIT)?;
        self.inner.current_management_identity(identity)?;
        Ok(preview)
    }

    fn save_foreign_plan(
        &self,
        identity: &Identity,
        request: ForeignPlanSave,
    ) -> Result<ForeignPlanStatus, ApiError> {
        if !request.acknowledge_plan_only {
            return Err(ApiError::invalid(
                "confirm saving only a non-executable review plan",
            ));
        }
        validate_review(&request.preview)?;
        let operation = request.preview.request.operation_id;
        if let Some(directory) = open(&self.inner.management_root)?
            && let Some(existing) = read(&directory, operation)?
        {
            owned(&existing, identity)?;
            if existing.review != request.preview {
                return Err(ApiError::conflict(
                    ErrorCode::IdempotencyKeyReused,
                    "foreign plan identifier already names another immutable review",
                ));
            }
            return Ok(existing.status(ForeignPlanValidation::Required));
        }
        // Rebuild the complete review before any new persistent plan is created.
        // This is still only an observation, not a reservation of the target.
        let current = self.preview_foreign_plan(identity, request.preview.request.clone())?;
        if current != request.preview {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "source, destination identity, empty parent or target checkpoint changed",
            ));
        }
        let actor = self.inner.current_management_identity(identity)?;
        let directory = PrivateDirectory::open_existing(&self.inner.management_root)
            .map_err(storage_addition::storage_error)?
            .create_child(
                &EntryName::new(DIRECTORY).expect("invariant: fixed private directory name"),
            )
            .map_err(storage_addition::storage_error)?;
        let record = StoredPlan::new(actor.principal, current);
        store(&directory, &record)?;
        Ok(record.status(ForeignPlanValidation::Required))
    }

    fn load_foreign_plan(
        &self,
        identity: &Identity,
        operation: OperationId,
    ) -> Result<StoredPlan, ApiError> {
        self.inner.current_management_identity(identity)?;
        let missing = || {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                ErrorCode::OperationNotFound,
                "foreign review plan was not found",
            )
        };
        let directory = open(&self.inner.management_root)?.ok_or_else(missing)?;
        let record = read(&directory, operation)?.ok_or_else(missing)?;
        owned(&record, identity)?;
        Ok(record)
    }

    fn revalidate_foreign_plan(
        &self,
        identity: &Identity,
        request: ForeignPlanSelection,
    ) -> Result<ForeignPlanStatus, ApiError> {
        let record = self.load_foreign_plan(identity, request.operation_id)?;
        record.require_digest(&request.review_digest)?;
        let current = self.preview_foreign_plan(identity, record.review.request.clone())?;
        if current != record.review {
            return Err(ApiError::conflict(
                ErrorCode::PlanStale,
                "saved foreign review no longer matches the complete source and empty target",
            ));
        }
        Ok(record.status(ForeignPlanValidation::MatchedAtObservation))
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/foreign_plan.rs"]
mod tests;
