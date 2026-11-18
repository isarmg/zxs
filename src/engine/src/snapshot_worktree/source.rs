//! FILE provenance is constructed in the actual reserve mutation. A directory
//! main branch, matching body or a global selection alone is not a FILE source.
use crate::business::{ensure_shared_file_branch, file_nodes};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotFileSource, SnapshotManifestError,
};
use crate::{BranchKind, ContentState, Engine, Error, ErrorCode, NamespaceEntryKind, State};
use storage_contract::snapshot_worktree::SnapshotNamespace;
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{FileId, OperationId, VersionId};

#[derive(Debug, error_definition::Error)]
pub enum SnapshotSourceError {
    #[error("preparing the complete selected directory publication")]
    DirectoryPublication(#[source] DirectoryPublicationFailure),
    #[error("snapshot source no longer names the admitted current root namespace")]
    Namespace,
    #[error("this snapshot mode requires the registered root namespace")]
    RootFiles,
    #[error("snapshot source exceeds the complete manifest budget")]
    Budget,
    #[error("snapshot occurrence has no exact independent FILE source: {0}")]
    File(FileId),
    #[error("snapshot selected node does not resolve to its current occurrence: {0}")]
    Selection(FileId),
    #[error("snapshot original content is unavailable: {0}")]
    Unavailable(VersionId),
    #[error("admitting the constructed complete snapshot manifest")]
    Manifest(#[source] SnapshotManifestError),
    #[error("encoding the actual snapshot reserve result")]
    Encoding(#[source] crate::persist::snapshot_worktree::CodecError),
    #[error("reserved worktree version differs from its exact FILE publication: {0}")]
    ReservedVersion(FileId),
    #[error("opening the exact sealed worktree occurrence for the FILE batch")]
    OriginalBody(#[source] Box<super::SnapshotWorktreeError>),
    #[error("whole Backup cannot transfer the registered immutable snapshot {0}")]
    BackupRegisteredSnapshot(OperationId),
    #[error("checking permanent snapshot registration before whole Backup")]
    BackupRegistration(#[source] super::SnapshotWorktreeError),
    #[error("checking worktree mount reservations before ordinary namespace publication")]
    WorktreeMounts(#[source] Box<super::SnapshotWorktreeError>),
}

impl SnapshotSourceError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::DirectoryPublication(source) => source.0.code(),
            Self::Namespace | Self::File(_) | Self::Selection(_) | Self::ReservedVersion(_) => {
                ErrorCode::StateInvalid
            }
            Self::RootFiles => ErrorCode::UnsupportedOperation,
            Self::BackupRegisteredSnapshot(_) => ErrorCode::UnsupportedOperation,
            Self::BackupRegistration(source) => source.code(),
            Self::WorktreeMounts(source) => source.code(),
            Self::OriginalBody(source) => source.code(),
            Self::Budget | Self::Manifest(SnapshotManifestError::Budget) => {
                ErrorCode::ResourceLimit
            }
            Self::Unavailable(_) => ErrorCode::ContentUnavailable,
            Self::Manifest(_) | Self::Encoding(_) => ErrorCode::PublicationInvalid,
        }
    }
}

fn reject(error: SnapshotSourceError) -> Error {
    Error::SnapshotSource(Box::new(error))
}

/// Called only inside the real reserve mutation after its host was prepared.
/// Lazy main rows enter the same prospective State and permanent publication;
/// no separately guessed branch or private snapshot-only fallback is emitted.
pub(super) fn construct(
    engine: &Engine,
    next: &mut State,
    source: &SnapshotNamespace,
    repository: SnapshotRepositoryId,
    node: SnapshotNodeId,
    operation: OperationId,
) -> crate::Result<AdmittedSnapshotManifest> {
    if source.root.is_some() {
        return Err(reject(SnapshotSourceError::RootFiles));
    }
    let branch_key = source.branch_id.to_string();
    let owner = engine.repository_for_branch(&branch_key)?;
    let branch = engine.state().branches.get(&branch_key)
        .ok_or_else(|| reject(SnapshotSourceError::Namespace))?;
    if owner.main_branch_id != branch_key
        || branch.generation != source.expected_branch_generation.get()
    {
        return Err(reject(SnapshotSourceError::Namespace));
    }
    let snapshot = engine.namespace_snapshot(&branch_key)?;
    if snapshot.entries().len() > crate::working_tree::snapshot::MAX_ENTRIES {
        return Err(reject(SnapshotSourceError::Budget));
    }
    let mut current = engine.state().commits.get(&branch.head_commit_id)
        .cloned().ok_or_else(|| reject(SnapshotSourceError::Namespace))?;
    current.entries = snapshot.entries().to_vec();
    let mut entries = Vec::new();
    for entry in snapshot.tree().visible_entries() {
        if entry.kind == NamespaceEntryKind::Directory {
            entries.push(SnapshotEntry {
                path: entry.path.clone(),
                content: SnapshotEntryContent::Directory {},
            });
            continue;
        }
        let file = entry.file_id.parse::<FileId>()?;
        let version = entry.version_id.parse::<VersionId>()?;
        let material = next.versions.get(&entry.version_id)
            .filter(|body| body.file_id == entry.file_id
                && body.history_tree_id == branch.history_tree_id
                && body.logical_size == entry.size)
            .ok_or_else(|| reject(SnapshotSourceError::Selection(file)))?;
        if next.file_deletions.contains_key(&entry.file_id)
            || next.content_states.get(&entry.version_id)
                .is_some_and(|state| *state != ContentState::Available)
        {
            return Err(reject(SnapshotSourceError::Unavailable(version)));
        }
        let material = SnapshotFile {
            file_id: file,
            version_id: version,
            size: material.logical_size,
            content_hash: material.content_hash,
        };
        let context = engine.file_context_for_current_entry(&owner.repository_id, entry)
            .ok_or_else(|| reject(SnapshotSourceError::File(file)))?;
        if context.inherited_from.is_some() {
            return Err(reject(SnapshotSourceError::File(file)));
        }
        let selected = next.branches.get(&context.branch_id)
            .ok_or_else(|| reject(SnapshotSourceError::File(file)))?;
        let selected = if selected.file_id.as_deref() == Some(entry.file_id.as_str()) {
            context.branch_id.clone()
        } else if context.branch_id == branch_key && context.node_id.is_none() {
            ensure_shared_file_branch(next, &current, entry)?
        } else {
            return Err(reject(SnapshotSourceError::File(file)));
        };
        let file_branch = next.branches.get(&selected)
            .filter(|selected| selected.branch_kind == BranchKind::File
                && selected.file_id.as_deref() == Some(entry.file_id.as_str())
                && selected.history_tree_id == branch.history_tree_id)
            .ok_or_else(|| reject(SnapshotSourceError::File(file)))?;
        let head = file_nodes::branch_head(next, &file_branch.branch_id)?;
        let selected_node = match &context.node_id {
            Some(node) => node.clone(),
            None => head.clone(),
        };
        if !file_nodes::belongs_to_branch(next, &selected, &head, &selected_node)
            || file_nodes::node_version(next, &entry.file_id, &selected_node)? != entry.version_id
        {
            return Err(reject(SnapshotSourceError::Selection(file)));
        }
        entries.push(SnapshotEntry {
            path: entry.path.clone(),
            content: SnapshotEntryContent::File {
                material,
                source: SnapshotFileSource::Container {
                    branch: selected.parse()?,
                    node: selected_node.parse()?,
                },
            },
        });
    }
    entries.sort_by(|left, right| left.path.key().cmp(right.path.key()));
    AdmittedSnapshotManifest::prepare(repository, node, operation, entries)
        .map_err(|source| reject(SnapshotSourceError::Manifest(source)))
}

pub(super) fn encoding(error: crate::persist::snapshot_worktree::CodecError) -> Error {
    reject(SnapshotSourceError::Encoding(error))
}

/// The private pipeline's complete typed cause, without exposing a constructor
/// for its internal working-source or physical products in the public API.
#[derive(Debug, error_definition::Error)]
#[error("{0}")]
pub struct DirectoryPublicationFailure(
    #[source] Box<crate::snapshot_worktree::drafts::DirectoryPublicationError>,
);

pub(super) fn directory_publication(
    source: crate::snapshot_worktree::drafts::DirectoryPublicationError,
) -> crate::Error {
    reject(SnapshotSourceError::DirectoryPublication(
        DirectoryPublicationFailure(Box::new(source)),
    ))
}
