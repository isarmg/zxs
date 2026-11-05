//! Explicit save-intent review. Namespace execution remains unavailable.
use super::*;
use storage_contract::storage_foreign_intent::*;
use storage_engine::{
    ForeignEvidenceDigest, ForeignImportIntentInfo, ForeignImportIntentRequest,
    ForeignImportIntentSource, ForeignImportIntentTarget, PreparedForeignImportIntent,
};

struct PreparedIntent {
    backend: Arc<EngineBackend>,
    prepared: PreparedForeignImportIntent,
    review: ForeignIntentPreview,
}
impl ManagementBackend {
    pub(super) fn foreign_intent_operation(
        &self,
        action: &str,
        input: Value,
        identity: &Identity,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        match action {
            "foreign_intent_preview" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let prepared = self.prepare_foreign_intent(identity, request)?;
                Ok(json_codec::to_value(prepared.review)
                    .expect("invariant: intent preview serializes"))
            }
            "foreign_intent_save" => {
                let request = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let status = self.save_foreign_intent(identity, request)?;
                Ok(json_codec::to_value(status).expect("invariant: intent observation serializes"))
            }
            "foreign_intent_status" => {
                let query: ForeignIntentQuery =
                    json_codec::from_value(input).map_err(ApiError::invalid_source)?;
                let expected: ForeignEvidenceDigest = query
                    .intent_digest
                    .parse()
                    .map_err(ApiError::invalid_source)?;
                let info = self
                    .inner
                    .backend()?
                    .foreign_intent_status(identity, &query)?
                    .ok_or_else(|| {
                        ApiError::new(
                            http::StatusCode::NOT_FOUND,
                            ErrorCode::OperationNotFound,
                            "target-local foreign intent was not found",
                        )
                    })?;
                if info.digest() != &expected {
                    return Err(stale_intent());
                }
                Ok(json_codec::to_value(status(info))
                    .expect("invariant: intent observation serializes"))
            }
            _ => Err(ApiError::invalid("unknown foreign intent operation")),
        }
    }
    fn prepare_foreign_intent(
        &self,
        identity: &Identity,
        request: ForeignIntentRequest,
    ) -> Result<PreparedIntent, ApiError> {
        let record = self.load_foreign_plan(identity, request.plan.operation_id)?;
        record.require_digest(&request.plan.review_digest)?;
        let backend = self.inner.backend()?;
        self.prepare_foreign_intent_admitted(identity, request, record, backend)
    }
    fn prepare_foreign_intent_admitted(
        &self,
        identity: &Identity,
        request: ForeignIntentRequest,
        record: StoredPlan,
        backend: Arc<EngineBackend>,
    ) -> Result<PreparedIntent, ApiError> {
        let source = self.inspect_foreign_source(identity, &record.review.request.source)?;
        if source.digest().to_string() != record.review.request.source_review_digest {
            return Err(stale_intent());
        }
        // Keep the same admitted backend across this bounded but potentially long
        // scan. The health sample's TTL cannot revoke already admitted work.
        let source =
            ForeignImportIntentSource::capture(source).map_err(crate::engine_backend::error)?;
        let target = &record.review.request.target;
        let target = ForeignImportIntentTarget::new(
            target.slot_id.clone(),
            target.parent.clone(),
            target.history_names.clone(),
        )
        .map_err(crate::engine_backend::error)?;
        let prepared = backend.prepare_foreign_intent(
            identity,
            source,
            target,
            ForeignImportIntentRequest {
                operation_id: request.operation_id,
                review_operation_id: request.plan.operation_id,
                review_digest: request
                    .plan
                    .review_digest
                    .parse()
                    .map_err(ApiError::invalid_source)?,
                principal: identity.principal.clone(),
            },
        )?;
        if prepared.target_mapping_digest() != record.review.target.mapping_digest {
            return Err(stale_intent());
        }
        let review = ForeignIntentPreview {
            request,
            plan: record.review,
            principal: identity.principal.clone(),
            intent_digest: prepared.digest().to_string(),
            effect: ForeignIntentEffect::SaveExecutionIntent,
            execution: ForeignExecutionCapability::Unavailable,
        };
        crate::history_window_encode::encode_bounded(&review, 2 * crate::JSON_BODY_LIMIT)?;
        self.inner.current_management_identity(identity)?;
        Ok(PreparedIntent {
            backend,
            prepared,
            review,
        })
    }
    fn save_foreign_intent(
        &self,
        identity: &Identity,
        request: ForeignIntentSave,
    ) -> Result<ForeignIntentStatus, ApiError> {
        if !request.confirm_save_intent {
            return Err(ApiError::invalid(
                "confirm saving only a target-local execution intent; copying and namespace publication remain unavailable",
            ));
        }
        let review = request.preview;
        let record = self.load_foreign_plan(identity, review.request.plan.operation_id)?;
        record.require_digest(&review.request.plan.review_digest)?;
        if record.review != review.plan || review.principal != identity.principal {
            return Err(stale_intent());
        }
        let expected = review
            .intent_digest
            .parse()
            .map_err(ApiError::invalid_source)?;
        let query = ForeignIntentQuery {
            operation_id: review.request.operation_id,
            target: record.review.target.identity.clone(),
            intent_digest: review.intent_digest.clone(),
        };
        let backend = self.inner.backend()?;
        // Exact replay reads only retained target facts; never reopens an old
        // source or pretends that a saved intent reserved its namespace baseline.
        if let Some(info) = backend
            .reconcile_foreign_intent(identity, &query, &expected, || self.inner.ensure_running())?
        {
            if info.review_operation_id() != review.request.plan.operation_id
                || info.review_digest().to_string() != review.request.plan.review_digest
            {
                return Err(stale_intent());
            }
            return Ok(status(info));
        }
        let prepared = self.prepare_foreign_intent_admitted(
            identity,
            review.request.clone(),
            record,
            backend,
        )?;
        if prepared.review != review {
            return Err(stale_intent());
        }
        self.finish_foreign_intent(identity, prepared)
    }
    fn finish_foreign_intent(
        &self,
        identity: &Identity,
        prepared: PreparedIntent,
    ) -> Result<ForeignIntentStatus, ApiError> {
        let digest = *prepared.prepared.digest();
        let confirmed = prepared
            .prepared
            .confirm_save(&digest)
            .map_err(crate::engine_backend::error)?;
        let info = prepared
            .backend
            .save_foreign_intent(identity, confirmed, || self.inner.ensure_running())?;
        Ok(status(info))
    }
}
fn stale_intent() -> ApiError {
    ApiError::conflict(
        ErrorCode::PlanStale,
        "foreign execution intent differs from its complete confirmed review",
    )
}
fn status(info: ForeignImportIntentInfo) -> ForeignIntentStatus {
    ForeignIntentStatus {
        address: ForeignIntentQuery {
            operation_id: info.operation_id(),
            target: ForeignDiskIdentity {
                pool_id: info.target().pool_id.clone(),
                disk_id: info.target().disk_id.clone(),
                slot_id: info.target().slot_id.clone(),
                binding_generation: info.target().binding_generation,
            },
            intent_digest: info.digest().to_string(),
        },
        plan: ForeignPlanSelection {
            operation_id: info.review_operation_id(),
            review_digest: info.review_digest().to_string(),
        },
        stage: ForeignIntentStage::IntentSaved,
        histories: info.history_count(),
        temporarily_unavailable_versions: info.temporarily_unavailable_versions(),
        execution: ForeignExecutionCapability::Unavailable,
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/management/foreign_intent.rs"]
mod tests;
