//! In-memory source capture for a future single-decision history move.
//! A capture is not a copy receipt, payload lease, database proof or write permit.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

mod projection;
pub(in crate::business) mod replacement;
pub use replacement::FileHistoryMoveReplacement;
pub(in crate::business) mod target_path;
pub use projection::{CurrentFileHistoryMoveProjection, FileHistoryMoveProjection};
pub(crate) use projection::{
    FileHistoryMoveReplay, NATIVE_KIND, NativeMoveCandidate, NativeMoveDraft, admit_deletions,
    admit_displaced, original_move_intent, reject_unclosed_publication, replay_selected_move,
    retained_deletion, verify_current_move_transition,
};

/// Exact source identity in addition to the two namespace heads. The history
/// copy address contract is reused, not its source-retaining operation semantics.
pub struct FileHistoryMoveRequest<'a> {
    pub namespace: &'a FileHistoryCopyRequest,
    pub source_file_id: &'a HistoryFileId,
}

/// Fixed logical file-database digest, not a body or physical SQLite checksum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileHistoryMoveSourceDigest(blake3::Hash);

impl std::fmt::Display for FileHistoryMoveSourceDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0.to_hex().as_str())
    }
}

/// Complete retained source frontier and bounded logical database observation.
/// Fields are private and there is deliberately no deserializer or publication
/// method. A fresh Engine, any State replacement, configuration replacement or
/// either namespace projection change invalidates this capture.
pub struct FileHistoryMoveCapture {
    namespace: FileHistoryCopyRequest,
    source_file_id: HistoryFileId,
    source_sequence: u64,
    source_records_digest: FileHistoryMoveSourceDigest,
    branch_heads: BTreeMap<String, String>,
    parent_node_ids: Vec<String>,
    version_ids: Vec<String>,
    target_slots: Vec<String>,
    replacement: Option<replacement::Capture>,
    state: Weak<State>,
    config: crate::ValidatedConfig,
    source_namespace: NamespaceSnapshotWitness,
    target_namespace: NamespaceSnapshotWitness,
}

impl FileHistoryMoveCapture {
    pub fn namespace(&self) -> &FileHistoryCopyRequest {
        &self.namespace
    }

    pub fn source_file_id(&self) -> &HistoryFileId {
        &self.source_file_id
    }

    /// Diagnostic capture sequence. This is not a durable CAS or authority token.
    pub fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    pub fn source_records_digest(&self) -> FileHistoryMoveSourceDigest {
        self.source_records_digest
    }

    /// Includes retained inactive and alias branches, not only the selected one.
    pub fn branch_heads(&self) -> &BTreeMap<String, String> {
        &self.branch_heads
    }

    /// Sorted, deduplicated parents required by the eventual source D.
    pub fn parent_node_ids(&self) -> &[String] {
        &self.parent_node_ids
    }

    pub fn version_ids(&self) -> &[String] {
        &self.version_ids
    }

    pub fn target_slots(&self) -> &[String] {
        &self.target_slots
    }

    pub(crate) fn replacement(&self) -> Option<&FileHistoryMoveReplacement> {
        self.replacement.as_ref().map(|capture| &capture.identity)
    }

    pub(crate) fn retained_version_ids(&self) -> impl Iterator<Item = &String> {
        self.version_ids.iter().chain(
            self.replacement
                .iter()
                .flat_map(|capture| capture.versions.iter()),
        )
    }
}

/// A current metadata observation tied to the Engine borrow. Even this value
/// does not attest copied bytes, three-scope databases or a recoverable decision.
pub struct CurrentFileHistoryMove<'a> {
    capture: &'a FileHistoryMoveCapture,
    _engine: &'a Engine,
}

impl CurrentFileHistoryMove<'_> {
    pub fn capture(&self) -> &FileHistoryMoveCapture {
        self.capture
    }
}

fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "file history move capture cancelled",
        ));
    }
    Ok(())
}

fn changed() -> Error {
    Error::new(
        ErrorCode::WorkspaceChanged,
        "file history move capture is no longer the exact current source, target and configuration",
    )
}

fn require_idle(engine: &Engine, namespace: &FileHistoryCopyRequest) -> Result<()> {
    for repository in [
        &namespace.source_repository_id,
        &namespace.target_repository_id,
    ] {
        batch::require_completed_file_saves(
            engine,
            engine.state(),
            &engine.state().repositories[repository].history_tree_id,
        )?;
    }
    Ok(())
}

fn require_ordinary_move_source(
    state: &State,
    repository: &Repository,
    entries: &[Entry],
    path: &name_policy::LookupPath,
) -> Result<()> {
    if !repository.file_manager_root
        || state
            .directory_git_repositories
            .contains_key(&repository.repository_id)
    {
        return Err(Error::new(
            ErrorCode::FileBranchScope,
            "history move requires an ordinary disk-root source",
        ));
    }
    let entry = namespace::find_unordered_key(entries, path.key())
        .ok_or_else(|| Error::new(ErrorCode::FileNotFound, "move source is absent"))?;
    target_path::require_parent(state, entries, &entry.path)
}

impl Engine {
    /// Capture the complete source before any move preparation is allowed to
    /// start. This method makes no mutation, writes no durable intent and never
    /// hides the source. It preserves the current retained-copy restrictions:
    /// available uniform routes, no backup overrides, no target replacement, no import
    /// provenance or graph-only copy whose D/undo prefix cannot be proven.
    ///
    /// Move additionally requires one disk-root occurrence outside initialized
    /// directory repositories. A shared logical file must first be explicitly
    /// separated; moving one path cannot silently freeze its other occurrences.
    /// The caller authenticates before invoking this trusted local API.
    pub fn capture_file_history_move(
        &self,
        request: FileHistoryMoveRequest<'_>,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveCapture> {
        checkpoint(cancellation)?;
        self.require_write()?;
        self.capture_retained_file_history_move(request, cancellation)
    }

    pub(crate) fn capture_retained_file_history_move(
        &self,
        request: FileHistoryMoveRequest<'_>,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveCapture> {
        self.capture_retained_file_history_move_target(request, None, cancellation)
    }

    pub(crate) fn capture_retained_file_history_move_target(
        &self,
        request: FileHistoryMoveRequest<'_>,
        replacement: Option<&FileHistoryMoveReplacement>,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveCapture> {
        checkpoint(cancellation)?;
        self.require_move_source_read()?;
        let repository_id = HistoryRepositoryId::parse(&request.namespace.source_repository_id)?;
        let path = name_policy::LookupPath::parse(&request.namespace.source_path)?;
        let deletion = self.file_deletion_preflight(
            FileDeletionTarget {
                repository_id: &repository_id,
                file_id: request.source_file_id,
                path: &path,
            },
            cancellation,
        )?;
        if deletion.occurrences().len() != 1 {
            return Err(Error::new(
                ErrorCode::FileBranchScope,
                "a single-path history move requires exactly one current source occurrence; explicitly separate the shared file first",
            ));
        }
        let database = crate::container_storage::file_database::capture_copy(
            self.state(),
            request.source_file_id.as_str(),
            &deletion.repository().history_tree_id,
        )?;
        checkpoint(cancellation)?;
        let source_entries = self.namespace_tree(&deletion.repository().main_branch_id)?;
        require_ordinary_move_source(
            self.state(),
            deletion.repository(),
            source_entries.entries(),
            &path,
        )?;
        let replacement = replacement
            .map(|identity| {
                replacement::capture(
                    self.state(),
                    self.admitted_current_namespaces()?,
                    request.namespace,
                    identity,
                    self.config.max_entries,
                    cancellation,
                )
            })
            .transpose()?;
        if replacement
            .as_ref()
            .is_some_and(|capture| capture.entry.file_id == request.source_file_id.as_str())
        {
            return Err(Error::new(
                ErrorCode::FileBranchScope,
                "history move source and replacement must have distinct stable file identities",
            ));
        }
        let source = history_copy::move_destination_sources(
            self.state(),
            self.admitted_current_namespaces()?,
            request.namespace,
            replacement.as_ref().map(|capture| &capture.identity),
        )?;
        // The deletion preflight and copy projection observed the same borrowed
        // State and exact path. Neither may substitute a same-path new file.
        assert_eq!(
            source.file,
            request.source_file_id.as_str(),
            "invariant: both captures resolve the same exact current occurrence"
        );
        require_idle(self, request.namespace)?;
        let target = &self.state().repositories[&request.namespace.target_repository_id];
        let target_head = &self.state().branches[&target.main_branch_id].head_commit_id;
        if replacement.is_none()
            && self.state().commits[target_head].entries.len() >= self.config.max_entries
        {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "move destination has reached its namespace entry limit",
            ));
        }
        let target_slots = self.route(&target.primary_slot_id, target.target_copies)?;
        let source_namespace = self.namespace_snapshot(&deletion.repository().main_branch_id)?;
        let target_namespace = self.namespace_snapshot(&target.main_branch_id)?;
        let capture = FileHistoryMoveCapture {
            namespace: request.namespace.clone(),
            source_file_id: request.source_file_id.clone(),
            source_sequence: self.state().sequence,
            source_records_digest: FileHistoryMoveSourceDigest(
                blake3::Hash::from_hex(&database.records_hash)
                    .expect("invariant: file database capture returns a canonical object digest"),
            ),
            branch_heads: deletion
                .branches()
                .iter()
                .map(|head| (head.branch().branch_id.clone(), head.node_id().to_owned()))
                .collect(),
            parent_node_ids: deletion.parent_node_ids().to_vec(),
            version_ids: source.versions,
            target_slots,
            replacement,
            state: Arc::downgrade(&self.state),
            config: self.config.clone(),
            source_namespace: source_namespace.witness(),
            target_namespace: target_namespace.witness(),
        };
        checkpoint(cancellation)?;
        Ok(capture)
    }

    /// Recheck under the same Engine owner, including volatile write-workspace
    /// admission. Every State publication invalidates the capture, even a note,
    /// tag, lifecycle or branch change that leaves both namespace HEADs alone.
    /// This conservative fence is O(1) in history size and pins no old State.
    /// Workspace scans remain O(open workspaces), without body I/O or fsync.
    ///
    /// This is only the source/target metadata fence. A future executor must
    /// additionally install and independently verify the complete target and
    /// publish target plus source D in one original recoverable operation.
    pub fn recheck_file_history_move<'a>(
        &'a self,
        capture: &'a FileHistoryMoveCapture,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<CurrentFileHistoryMove<'a>> {
        checkpoint(cancellation)?;
        self.require_move_source_read()?;
        if !capture.state.ptr_eq(&Arc::downgrade(&self.state))
            || !self.config.same_admission(&capture.config)
            || !self.namespace_snapshot_matches(&capture.source_namespace)
            || !self.namespace_snapshot_matches(&capture.target_namespace)
        {
            return Err(changed());
        }
        require_idle(self, &capture.namespace)?;
        if let Some(view) = self.admitted_current_namespaces()? {
            for repository in [
                &capture.namespace.source_repository_id,
                &capture.namespace.target_repository_id,
            ] {
                view.require_branch_write(
                    self.state(),
                    &self.state().repositories[repository].main_branch_id,
                )?;
            }
        }
        checkpoint(cancellation)?;
        Ok(CurrentFileHistoryMove {
            capture,
            _engine: self,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/history_move_tests.rs"]
mod tests;
