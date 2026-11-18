//! A name reservation and its single native business outcome. Neither record
//! grants disk write authority when decoded from a local directory.
use super::{HostEvidence, LedgerBinding};
use crate::directory_worktree::ledger::{Checkpoint, Event};
use crate::directory_worktree::lifecycle::Mount;
use crate::directory_worktree::store::SparseWorktreeBinding;
use data_serialization::{Deserialize, Serialize};
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::{SnapshotHandle, SnapshotHost};
use storage_contract::snapshot_worktree::mount_change::MountChange;
use storage_contract::working_tree::MaterialCommitment;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Resolved {
    Hide {},
    Restore { mount: Mount },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub(crate) request: MountChange,
    pub(crate) content_reserve: OperationId,
    pub(crate) source: SnapshotHandle,
    pub(crate) committed: SnapshotHandle,
    pub(crate) host: HostEvidence,
    pub(crate) root: SnapshotHost,
    pub(crate) instance: SparseWorktreeBinding,
    pub(crate) ledger: LedgerBinding,
    pub(crate) before: Checkpoint,
    pub(crate) prior_mount: Mount,
    pub(crate) resolved: Resolved,
    pub(crate) publish_operation: OperationId,
}

impl Reservation {
    pub(crate) fn event(&self) -> Event {
        let operation = self.request.operation_id;
        match &self.resolved {
            Resolved::Hide {} => Event::Delete { operation, expected: self.request.expected },
            Resolved::Restore { mount } => Event::Restored { operation, mount: mount.clone() },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Publication {
    pub(crate) reserve: OperationId,
    pub(crate) reservation: MaterialCommitment,
    pub(crate) checkpoint: Checkpoint,
}
