//! Request identities for the full-snapshot repository and sparse worktrees.
//!
//! A handle names the independently published completion, not a local marker.
//! Decoding these values does not admit a root, node, worktree or publication.
pub mod draft_commit;
pub mod additions;
pub mod discovery;
pub mod root;
pub mod outcome;
pub mod creation;
pub mod history;
pub mod instances;
pub mod mount_change;

use crate::working_tree::{
    DirectoryId, RootId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use crate::{BranchId, EntryId, Generation, Id, OperationId, RepositoryId};
use data_serialization::{Deserialize, Serialize};
use name_policy::LookupPath;

pub mod kind {
    pub enum Worktree {}
    impl crate::IdKind for Worktree {
        const PREFIX: &'static str = "worktree";
    }
}

pub type WorktreeId = Id<kind::Worktree>;

/// A namespace identity is not a physical working-directory identity. In
/// particular, a repository root is not its disk's common-data anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "id", rename_all = "snake_case", deny_unknown_fields)]
pub enum NamespaceDirectoryId {
    Repository(RepositoryId),
    Entry(EntryId),
}

/// Stable current namespace addressing, independent of SMB or Web exposure.
/// Physical registration must separately bind the exact observed host entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceHostAddress {
    pub repository_id: RepositoryId,
    pub branch_id: BranchId,
    pub directory: NamespaceDirectoryId,
}

/// The actual source owner resolves this host under the named current root.
/// It is not a pathname or permission to traverse an unregistered directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotHost {
    pub root_id: RootId,
    pub directory_id: DirectoryId,
    pub expected_root_generation: Generation,
}

/// The host and source are separate inputs. Their relationship is established
/// by the actual current namespace owner, never inferred from a storage path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotNamespace {
    pub branch_id: BranchId,
    pub root: Option<LookupPath>,
    pub expected_branch_generation: Generation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct InitializeSnapshot {
    pub host: SnapshotHost,
    pub source: SnapshotNamespace,
    pub operation_id: OperationId,
}

/// A caller cannot substitute an equally shaped local node: opening requires
/// the exact completed reservation with all four identities still matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotHandle {
    pub publication: OperationId,
    pub repository_id: SnapshotRepositoryId,
    pub branch_id: SnapshotBranchId,
    pub node_id: SnapshotNodeId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CreateWorktree {
    pub source: SnapshotHandle,
    pub parent_directory_id: DirectoryId,
    pub name: name_policy::Name,
    pub expected_root_generation: Generation,
    pub operation_id: OperationId,
}

/// The public result identifies the completion receipt as well as the stable
/// instance. A reusable display name is deliberately not the instance key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeHandle {
    pub publication: OperationId,
    pub worktree_id: WorktreeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeRevision {
    pub generation: Generation,
    pub manifest: crate::working_tree::MaterialCommitment,
}

/// One explicit completed write to an existing original occurrence. The BODY
/// facts participate in the request digest; they do not identify its container.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct EditWorktree {
    pub target: WorktreeHandle,
    pub path: name_policy::PortablePath,
    pub expected: WorktreeRevision,
    #[serialization(with = "crate::u64_string")]
    pub size: u64,
    pub content_hash: crate::ContentHash,
    pub operation_id: OperationId,
}

/// Explicit directory publication of one completed sparse revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitWorktree {
    pub target: WorktreeHandle,
    pub expected: WorktreeRevision,
    pub message: String,
    pub operation_id: OperationId,
}

/// Current, independently registered root facts for a first snapshot request.
/// This is a read projection, not a retained prepare or permission to use a path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotRootContext {
    pub host: SnapshotHost,
    pub source: SnapshotNamespace,
}

/// The original source and current decided revision of an independently opened
/// instance. A display name does not claim an SMB or filesystem mount exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorktreeStatus {
    pub target: WorktreeHandle,
    /// Creation completion or last completed restore request, never the content revision.
    pub visible: OperationId,
    pub original: SnapshotHandle,
    pub parent_directory_id: DirectoryId,
    pub name: name_policy::LookupName,
    pub revision: WorktreeRevision,
}
