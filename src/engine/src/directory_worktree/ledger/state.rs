use super::record::{AddedFile, AddedFileResult, Checkpoint, EditRecord, Event, Genesis, commitment};
use super::LedgerError;
use crate::directory_worktree::lifecycle::{
    Phase, Publication, PublicationMode, PublicationScope, Worktree,
};
use crate::directory_worktree::projection::{FileResult, SparseProjection};
use crate::directory_worktree::projection::namespace::{NamespaceChange, NamespaceView};
use crate::working_tree::snapshot::AdmittedSnapshotManifest;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::OperationId;
use storage_contract::working_tree::MaterialCommitment;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind { File, Edits, Namespace, Commit, CommitAdded, Delete, Restore }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Stage { Requested, Completed }
impl Event {
    fn identity(&self) -> (OperationId, OperationKind, Stage) {
        match self {
            Self::File { operation, .. } => (*operation, OperationKind::File, Stage::Requested),
            Self::Edits { operation, .. } => (*operation, OperationKind::Edits, Stage::Requested),
            Self::Namespace { operation, .. } => {
                (*operation, OperationKind::Namespace, Stage::Requested)
            }
            Self::Commit { operation, .. } => (*operation, OperationKind::Commit, Stage::Requested),
            Self::CommitAdded { operation, .. } => {
                (*operation, OperationKind::CommitAdded, Stage::Requested)
            }
            Self::AddedCompleted { publication, .. } => (
                publication.intent.operation, OperationKind::CommitAdded, Stage::Completed,
            ),
            Self::Delete { operation, .. } => (*operation, OperationKind::Delete, Stage::Requested),
            Self::Restored { operation, .. } => {
                (*operation, OperationKind::Restore, Stage::Requested)
            }
            Self::Completed { publication, .. } => (
                publication.intent.operation,
                match publication.intent.mode {
                    PublicationMode::DirectoryCommit { .. } => OperationKind::Commit,
                    PublicationMode::FilesBeforeDelete => OperationKind::Delete,
                },
                Stage::Completed,
            ),
        }
    }
}

#[derive(Clone)]
enum NamespaceState {
    Original,
    Edited {
        // Keep ordered batches for the revision commitment; the view is their
        // derived mapping, admitted with each event's material during replay.
        batches: Vec<Vec<NamespaceChange>>,
        view: NamespaceView,
    },
}
impl NamespaceState {
    fn batches(&self) -> &[Vec<NamespaceChange>] {
        match self {
            Self::Original => &[],
            Self::Edited { batches, .. } => batches,
        }
    }
}

#[derive(Clone)]
pub(super) struct State {
    pub(super) worktree: Worktree,
    pub(super) edits: Vec<EditRecord>,
    pub(super) file_results: Vec<FileResult>,
    pub(super) added: Vec<AddedFile>,
    pub(super) added_results: Vec<AddedFileResult>,
    // Frame and ledger byte limits bound the retained history on append/replay.
    namespace: NamespaceState,
    creation: OperationId,
    versions: BTreeSet<storage_contract::VersionId>,
    operations: BTreeMap<OperationId, OperationKind>,
    results: BTreeMap<(OperationId, Stage), (MaterialCommitment, Checkpoint)>,
}
impl State {
    pub(super) fn new(genesis: &Genesis) -> Self {
        Self {
            worktree: Worktree::fresh(
                genesis.binding.worktree(), genesis.source, genesis.mount.clone(),
            ),
            edits: Vec::new(),
            file_results: Vec::new(),
            added: Vec::new(),
            added_results: Vec::new(),
            namespace: NamespaceState::Original,
            creation: genesis.binding.operation(),
            versions: BTreeSet::new(),
            operations: BTreeMap::new(),
            results: BTreeMap::new(),
        }
    }
    pub(super) fn prior(&self, event: &Event) -> Result<Option<Checkpoint>, LedgerError> {
        let (operation, kind, stage) = event.identity();
        if operation == self.creation
            || self.operations.get(&operation).is_some_and(|existing| *existing != kind)
        {
            return Err(LedgerError::OperationConflict);
        }
        let Some((digest, checkpoint)) = self.results.get(&(operation, stage)) else {
            return Ok(None);
        };
        if *digest != event_commitment(event)? {
            return Err(LedgerError::OperationConflict);
        }
        Ok(Some(*checkpoint))
    }
    pub(super) fn apply(
        &mut self,
        source: &AdmittedSnapshotManifest,
        event: &Event,
        checkpoint: Checkpoint,
        verify: &mut impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
        verify_added: &mut impl FnMut(&Publication, &AddedFileResult) -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        if self.prior(event)?.is_some() {
            return Err(LedgerError::Chain);
        }
        let (operation, kind, stage) = event.identity();
        match event {
            Event::File { expected, file, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(publication) }
                    if matches!(publication.scope, PublicationScope::Directory { .. })) {
                    return Err(LedgerError::RebaseRequired);
                }
                if file.target.worktree.worktree_id != self.worktree.id() {
                    return Err(LedgerError::AddedFile);
                }
                let mut added = self.added.clone();
                match added.binary_search_by_key(&file.target.entry, |old| old.target.entry) {
                    Ok(index) => {
                        let old = &added[index];
                        if old.target != file.target || old.birth != file.birth {
                            return Err(LedgerError::AddedFile);
                        }
                        let storage_contract::file_draft::UnbornBody::Saved { manifest } =
                            file.working.body() else {
                                return Err(LedgerError::AddedFile);
                            };
                        if manifest.operation() != operation {
                            return Err(LedgerError::AddedFile);
                        }
                        let next = old.working.saved(file.target.file, manifest.clone(),
                            crate::working_tree::snapshot::MAX_LOGICAL_BYTES)
                            .map_err(LedgerError::WorkingManifest)?;
                        if next != file.working {
                            return Err(LedgerError::AddedFile);
                        }
                        added[index] = file.clone();
                    }
                    Err(index) => {
                        if file.birth != operation || file.working
                            != storage_contract::file_draft::UnbornWorking::birth() {
                            return Err(LedgerError::AddedFile);
                        }
                        added.insert(index, file.clone());
                    }
                }
                self.projection_with_files(source, &self.edits, &added)?;
                let manifest = revision_commitment_with_files(
                    source, &self.edits, self.namespace.batches(), &added,
                )?;
                self.worktree.record_edit(manifest).map_err(LedgerError::Lifecycle)?;
                self.added = added;
            }
            Event::Edits { expected, edits, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(publication) }
                    if matches!(publication.scope, PublicationScope::Directory { .. })) {
                    return Err(LedgerError::RebaseRequired);
                }
                if edits.is_empty() {
                    return Err(LedgerError::Chain);
                }
                if matches!(self.namespace, NamespaceState::Edited { .. }) {
                    let view = self.projection(source)?;
                    if edits.iter().any(|edit| !view.contains_source_file(&edit.path)) {
                        return Err(LedgerError::Projection(
                            crate::directory_worktree::projection::ProjectionError::Source,
                        ));
                    }
                }
                if edits.iter().any(|edit| self.versions.contains(&edit.material.version_id)) {
                    return Err(LedgerError::OperationConflict);
                }
                let merged = merge_original_edits(source, &self.edits, edits)?;
                if matches!(self.namespace, NamespaceState::Edited { .. })
                    || !self.added.is_empty()
                {
                    self.projection_with_edits(source, &merged)?;
                }
                let manifest = revision_commitment_with_files(
                    source, &merged, self.namespace.batches(), &self.added,
                )?;
                self.worktree.record_edit(manifest).map_err(LedgerError::Lifecycle)?;
                self.edits = merged;
                self.versions.extend(edits.iter().map(|edit| edit.material.version_id));
            }
            Event::Namespace { expected, changes, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(publication) }
                    if matches!(publication.scope, PublicationScope::Directory { .. })) {
                    return Err(LedgerError::RebaseRequired);
                }
                if changes.is_empty() {
                    return Err(LedgerError::Chain);
                }
                let view = self.projection(source)?.namespace(changes)
                    .map_err(LedgerError::Namespace)?.into_namespace();
                let mut batches = self.namespace.batches().to_vec();
                batches.push(changes.clone());
                let manifest = revision_commitment_with_files(
                    source, &self.edits, &batches, &self.added,
                )?;
                self.worktree.record_edit(manifest).map_err(LedgerError::Lifecycle)?;
                self.namespace = NamespaceState::Edited { batches, view };
            }
            Event::Commit { expected, current_head, fork_branch, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(publication) }
                    if matches!(publication.scope, PublicationScope::Directory { .. })) {
                    return Err(LedgerError::RebaseRequired);
                }
                self.require_publication_scope()?;
                self.worktree.prepare_commit(operation, *current_head, *fork_branch)
                    .map_err(LedgerError::Lifecycle)?;
            }
            Event::CommitAdded { expected, current_head, fork_branch, .. } => {
                self.expected(*expected)?;
                self.require_added_scope()?;
                self.worktree.prepare_commit(operation, *current_head, *fork_branch)
                    .map_err(LedgerError::Lifecycle)?;
            }
            Event::AddedCompleted { publication, file } => {
                self.require_added_files()?;
                if !matches!(publication.intent.mode, PublicationMode::DirectoryCommit { .. })
                    || !matches!(publication.scope, PublicationScope::Directory { .. })
                    || self.operations.get(&operation) != Some(&OperationKind::CommitAdded)
                {
                    return Err(LedgerError::FileResults);
                }
                file.require_file(&self.added[0])?;
                let mut next = self.worktree.clone();
                next.complete_publication(*publication).map_err(LedgerError::Lifecycle)?;
                self.projection(source)?.with_added_results(std::slice::from_ref(file))
                    .map_err(LedgerError::Projection)?;
                verify_added(publication, file)?;
                self.added_results = vec![file.clone()];
                self.worktree = next;
            }
            Event::Delete { expected, .. } => {
                self.expected(*expected)?;
                self.require_publication_scope()?;
                self.worktree.prepare_delete(operation).map_err(LedgerError::Lifecycle)?;
            }
            Event::Completed { publication, files } => {
                let mut next = self.worktree.clone();
                next.complete_publication(*publication).map_err(LedgerError::Lifecycle)?;
                let dirty = self.dirty_edits();
                if files.len() != dirty.len() {
                    return Err(LedgerError::FileResults);
                }
                let mut results = self.file_results.clone();
                for (file, edit) in files.iter().zip(dirty) {
                    if file.path != edit.path || file.material != edit.material {
                        return Err(LedgerError::FileResults);
                    }
                    match results.binary_search_by(|saved| saved.path.key().cmp(file.path.key())) {
                        Ok(index) => {
                            if results[index].source
                                == crate::working_tree::snapshot::SnapshotFileSource::SnapshotOnly
                                && file.source != results[index].source
                            {
                                return Err(LedgerError::FileResults);
                            }
                            results[index] = file.clone();
                        }
                        Err(index) => results.insert(index, file.clone()),
                    }
                }
                let edits = self.edits.iter().map(EditRecord::file_edit)
                    .collect::<Vec<_>>();
                SparseProjection::admit_with_results(source, edits, &results)
                    .map_err(LedgerError::Projection)?;
                // Verify the exact per-file results as well as the overall
                // publication; matching local JSON cannot manufacture receipts.
                verify(publication, files)?;
                self.worktree = next;
                self.file_results = results;
            }
            Event::Restored { mount, .. } => {
                self.worktree.restore(mount.clone(), false).map_err(LedgerError::Lifecycle)?;
            }
        }
        self.operations.insert(operation, kind);
        self.results.insert((operation, stage), (event_commitment(event)?, checkpoint));
        Ok(())
    }
    pub(super) fn projection<'a>(
        &self,
        source: &'a AdmittedSnapshotManifest,
    ) -> Result<SparseProjection<'a>, LedgerError> {
        self.projection_with_edits(source, &self.edits)
    }
    fn projection_with_edits<'a>(
        &self,
        source: &'a AdmittedSnapshotManifest,
        edits: &[EditRecord],
    ) -> Result<SparseProjection<'a>, LedgerError> {
        self.projection_with_files(source, edits, &self.added)
    }
    fn projection_with_files<'a>(
        &self,
        source: &'a AdmittedSnapshotManifest,
        edits: &[EditRecord],
        added: &[AddedFile],
    ) -> Result<SparseProjection<'a>, LedgerError> {
        let edits = edits.iter().map(EditRecord::file_edit).collect::<Vec<_>>();
        let view = SparseProjection::admit_with_results(source, edits, &self.file_results)
            .map_err(LedgerError::Projection)?;
        let view = match &self.namespace {
            NamespaceState::Original => view,
            NamespaceState::Edited { view: namespace, .. } => {
                view.with_namespace(namespace).map_err(LedgerError::Namespace)?
            }
        };
        view.with_added(added).map_err(LedgerError::Projection)?
            .with_added_results(&self.added_results).map_err(LedgerError::Projection)
    }
    fn require_added_files(&self) -> Result<(), LedgerError> {
        if self.added.len() != 1 || !self.edits.is_empty() || !self.file_results.is_empty()
            || !self.added_results.is_empty()
            || !matches!(self.namespace, NamespaceState::Original)
        {
            return Err(LedgerError::AddedPublicationRequired);
        }
        Ok(())
    }
    pub(super) fn require_added_scope(&self) -> Result<(), LedgerError> {
        self.require_added_files()?;
        if !matches!(self.worktree.phase(), Phase::Active { saved: None }) {
            return Err(LedgerError::RebaseRequired);
        }
        Ok(())
    }
    pub(super) fn require_publication_scope(&self) -> Result<(), LedgerError> {
        // Publication still associates FILE results with original paths. Until
        // durable obligations cover removed and renamed occurrences, neither a
        // directory commit nor files-before-delete can consume this namespace.
        if !self.added.is_empty() {
            return Err(LedgerError::AddedPublicationRequired);
        }
        if matches!(self.namespace, NamespaceState::Edited { .. }) {
            return Err(LedgerError::NamespacePublicationRequired);
        }
        Ok(())
    }
    pub(super) fn dirty_edits(&self) -> Vec<&EditRecord> {
        self.edits.iter().filter(|edit| {
            // A previously verified version result suppresses republication.
            // Explicit writes always allocate a fresh version; no body hash
            // is consulted to infer whether a user operation occurred.
            !self.file_results.iter().any(|saved| {
                saved.path == edit.path && saved.material.version_id == edit.material.version_id
            })
        }).collect()
    }
    fn expected(&self, revision: crate::directory_worktree::lifecycle::Revision)
        -> Result<(), LedgerError>
    {
        if self.worktree.revision() != revision {
            return Err(LedgerError::Stale);
        }
        Ok(())
    }
}
pub(crate) fn revision_commitment(
    source: &AdmittedSnapshotManifest,
    edits: &[EditRecord],
    namespace: &[Vec<NamespaceChange>],
) -> Result<MaterialCommitment, LedgerError> {
    // Preserve the existing edit-only revision bytes. Once namespace intent is
    // recorded, every later revision binds its complete ordered batch history.
    let bytes = if namespace.is_empty() {
        storage_contract::canonical_json(&(source.reference(), edits))
    } else {
        storage_contract::canonical_json(&(source.reference(), edits, namespace))
    }.map_err(LedgerError::Codec)?;
    Ok(commitment(&bytes))
}
pub(crate) fn revision_commitment_with_files(
    source: &AdmittedSnapshotManifest,
    edits: &[EditRecord],
    namespace: &[Vec<NamespaceChange>],
    added: &[AddedFile],
) -> Result<MaterialCommitment, LedgerError> {
    if added.is_empty() {
        return revision_commitment(source, edits, namespace);
    }
    let bytes = storage_contract::canonical_json(
        &("worktree-added-files", source.reference(), edits, namespace, added),
    ).map_err(LedgerError::Codec)?;
    Ok(commitment(&bytes))
}
fn event_commitment(event: &Event) -> Result<MaterialCommitment, LedgerError> {
    let bytes = storage_contract::canonical_json(event).map_err(LedgerError::Codec)?;
    Ok(commitment(&bytes))
}
/// Receipts and physical frames must describe the same complete overlay.
/// A new version replaces only its exact original occurrence, never a sibling.
pub(crate) fn merge_original_edits(
    source: &AdmittedSnapshotManifest, previous: &[EditRecord], changes: &[EditRecord],
) -> Result<Vec<EditRecord>, LedgerError> {
    admit_edits(source, changes)?;
    let mut merged = previous.to_vec();
    for edit in changes {
        match merged.binary_search_by(|old| old.path.key().cmp(edit.path.key())) {
            Ok(index) => merged[index] = edit.clone(),
            Err(index) => merged.insert(index, edit.clone()),
        }
    }
    admit_edits(source, &merged)?;
    Ok(merged)
}

fn admit_edits(source: &AdmittedSnapshotManifest, edits: &[EditRecord]) -> Result<(), LedgerError> {
    if edits.len() > source.entries().len() {
        return Err(LedgerError::Budget);
    }
    let mut bytes = 0u64;
    let mut leaves = BTreeSet::new();
    let mut entities = BTreeSet::new();
    let mut versions = BTreeSet::new();
    for (index, edit) in edits.iter().enumerate() {
        if index > 0 && edits[index - 1].path.key() >= edit.path.key() {
            return Err(LedgerError::Chain);
        }
        if edit.path.file_name().len() > 255 - 16 {
            return Err(LedgerError::Budget);
        }
        bytes = bytes.checked_add(edit.material.size)
            .filter(|total| *total <= 64 * 1024 * 1024).ok_or(LedgerError::Budget)?;
        if !leaves.insert(edit.leaf.key())
            || !entities.insert((edit.entity.device(), edit.entity.inode()))
            || !versions.insert(edit.material.version_id)
        {
            return Err(LedgerError::Binding);
        }
    }
    let converted = edits.iter().map(EditRecord::file_edit).collect::<Vec<_>>();
    SparseProjection::admit(source, converted).map_err(LedgerError::Projection)?;
    Ok(())
}
