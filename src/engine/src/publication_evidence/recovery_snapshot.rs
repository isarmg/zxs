//! Exact original snapshot evidence, before any recovered outcome is overlaid.
use super::verified_snapshot_at;
use crate::{
    DiskConfig, Error, ErrorCode, OperationId, OperationResult, Result, State,
    disk::DiskResources,
    history_package::{AdmittedState, SharedAdmittedState},
    journal::Candidate,
};
use std::sync::Arc;

/// Constructed only after the referenced snapshot and its complete closure have
/// passed the ordinary disk authentication and RawState admission path.
/// No constructor accepts a visible Engine State or a caller-supplied proof.
pub(crate) struct AdmittedSnapshot {
    snapshot_hash: String,
    original: AdmittedState,
}

impl AdmittedSnapshot {
    pub(crate) fn for_candidate(&self, candidate: &Candidate) -> Result<&AdmittedState> {
        if self.snapshot_hash != candidate.snapshot_hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "admitted snapshot differs from candidate root",
            ));
        }
        Ok(&self.original)
    }

    pub(crate) fn into_shared(self) -> SharedAdmittedSnapshot {
        SharedAdmittedSnapshot {
            snapshot_hash: self.snapshot_hash,
            original: self.original.into_shared(),
        }
    }

    /// End the original disk-snapshot proof before producing visible recovery
    /// state. The returned State cannot stand in for the original snapshot when
    /// a recovery publication replaces its operation outcome.
    pub(crate) fn into_visible_state(
        self,
        operation: OperationId,
        recovered_result: Option<OperationResult>,
    ) -> State {
        let mut state = self.original.into_state();
        if let Some(result) = recovered_result {
            state.operations.insert(operation, result);
        }
        state
    }
}

/// The short workspace-admission loan retains the same snapshot identity.
pub(crate) struct SharedAdmittedSnapshot {
    snapshot_hash: String,
    original: SharedAdmittedState,
}

impl SharedAdmittedSnapshot {
    pub(crate) fn share_state(&self) -> Arc<State> {
        self.original.share_state()
    }

    pub(crate) fn into_admitted(self) -> AdmittedSnapshot {
        AdmittedSnapshot {
            snapshot_hash: self.snapshot_hash,
            original: self.original.into_admitted(),
        }
    }
}

pub(crate) fn verified_recovery_snapshot_at(
    resources: &DiskResources,
    members: &[DiskConfig],
    hash: &str,
) -> Result<AdmittedSnapshot> {
    let original = verified_snapshot_at(resources, members, hash)?;
    Ok(AdmittedSnapshot {
        snapshot_hash: hash.to_owned(),
        original,
    })
}
