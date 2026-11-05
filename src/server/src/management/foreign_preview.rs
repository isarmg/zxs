//! An explicit, bounded source inspection, never a writable foreign admission.
use super::*;
use storage_contract::storage_foreign::*;
use storage_engine::{ForeignImportLimits, ForeignImportPreview, ForeignVersionAvailability};

const MAX_HISTORIES: usize = 128;
const MAX_METADATA: usize = 64 * 1024 * 1024;
const MAX_JOURNAL: usize = 16 * 1024 * 1024;
const MAX_VERSIONS: usize = 10_000;
const MAX_PAYLOAD: u64 = 8 * 1024 * 1024 * 1024;

impl ManagementBackend {
    pub(super) fn foreign_disk_preview(
        &self,
        identity: &Identity,
        request: ForeignDiskPreviewRequest,
    ) -> Result<ForeignDiskPreview, ApiError> {
        let preview = self.inspect_foreign_source(identity, &request.source)?;
        let output = response(request.source, &preview);
        crate::history_window_encode::encode_bounded(&output, crate::JSON_BODY_LIMIT)?;
        Ok(output)
    }

    pub(super) fn inspect_foreign_source(
        &self,
        identity: &Identity,
        observed: &storage_contract::storage_directories::DirectorySource,
    ) -> Result<ForeignImportPreview, ApiError> {
        self.inner.current_management_identity(identity)?;
        let (excluded, stopped) = self.discovery_scope(identity)?;
        let file = crate::storage_directories::reopen_source(
            self.inner.discovery_root.as_str(),
            observed,
            &excluded,
            &stopped,
        )?;
        let limits = ForeignImportLimits::new(MAX_METADATA, MAX_JOURNAL, MAX_VERSIONS, MAX_PAYLOAD)
            .expect("invariant: service foreign-inspection budgets are within engine limits");
        let preview = ForeignImportPreview::inspect(file, PathBuf::from(&observed.path), limits)
            .map_err(crate::engine_backend::error)?;
        if preview.histories().len() > MAX_HISTORIES {
            return Err(ApiError::new(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                storage_contract::ErrorCode::ResourceLimit,
                "foreign preview exceeds the complete 128-history response budget",
            ));
        }
        // The observed parent and child must still name the original source;
        // renewed authentication precedes releasing any slow inspection result.
        drop(crate::storage_directories::reopen_source(
            self.inner.discovery_root.as_str(),
            observed,
            &excluded,
            &stopped,
        )?);
        self.inner.current_management_identity(identity)?;
        Ok(preview)
    }
}

pub(super) fn response(
    observed: storage_contract::storage_directories::DirectorySource,
    preview: &ForeignImportPreview,
) -> ForeignDiskPreview {
    let source = preview.source();
    let identity = source.registration();
    let checkpoint = preview.checkpoint();
    ForeignDiskPreview {
        source: observed,
        identity: ForeignDiskIdentity {
            pool_id: identity.pool_id.clone(),
            disk_id: identity.disk_id.clone(),
            slot_id: identity.slot_id.clone(),
            binding_generation: identity.binding_generation,
        },
        source_level: preview.source_level(),
        role_evidence: match source.role_evidence() {
            storage_engine::DiskRoleEvidence::InitialConfiguration => {
                storage_contract::storage_directories::DiskRoleEvidence::InitialDeclaration
            }
            storage_engine::DiskRoleEvidence::MembershipGeneration { .. } => {
                storage_contract::storage_directories::DiskRoleEvidence::GenerationEvidence
            }
        },
        seal_digest: source.seal_digest().to_string(),
        review_digest: preview.digest().to_string(),
        checkpoint: ForeignDiskCheckpoint {
            candidate_hash: checkpoint.candidate_hash().into(),
            publication_hash: checkpoint.publication_hash().into(),
            sequence: checkpoint.sequence(),
            authority_hash: checkpoint.authority_hash().map(str::to_owned),
            unpublished_tail: checkpoint.has_unpublished_tail(),
        },
        histories: preview.histories().iter().map(history_summary).collect(),
        execution: ForeignExecutionCapability::Unavailable,
    }
}

fn history_summary(history: &storage_engine::ForeignHistory) -> ForeignHistorySummary {
    let manifest = history.manifest();
    let origin = history.origin();
    let mut summary = ForeignHistorySummary {
        group_id: history.group_id().into(),
        history_tree_id: manifest.history.history_tree_id.clone(),
        repository_name: manifest.repository.name.clone(),
        origin: ForeignDiskIdentity {
            pool_id: origin.pool_id().into(),
            disk_id: origin.disk_id().into(),
            slot_id: origin.slot_id().into(),
            binding_generation: origin.binding_generation(),
        },
        manifest_hash: history.manifest_hash().into(),
        branches: manifest.branches.len(),
        directory_branches: manifest.directory_branches.len(),
        directory_trees: manifest.directory_trees.len(),
        commit_records: manifest.commits.len(),
        versions: manifest.versions.len(),
        verified_local_versions: 0,
        temporarily_unavailable_versions: Vec::new(),
        recorded_purged_versions: 0,
        recorded_migrated_versions: 0,
    };
    for (version, availability) in history.availability() {
        match availability {
            ForeignVersionAvailability::VerifiedLocal => summary.verified_local_versions += 1,
            ForeignVersionAvailability::TemporarilyUnavailable => summary
                .temporarily_unavailable_versions
                .push(version.clone()),
            ForeignVersionAvailability::RecordedPurged => summary.recorded_purged_versions += 1,
            ForeignVersionAvailability::RecordedMigrated => summary.recorded_migrated_versions += 1,
        }
    }
    summary
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/foreign_preview.rs"]
mod tests;
