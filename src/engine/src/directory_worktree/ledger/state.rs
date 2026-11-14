use super::record::{Checkpoint, EditRecord, Event, Genesis, commitment};
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
enum OperationKind { Edits, Namespace, Commit, Delete, Restore }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Stage { Requested, Completed }
impl Event {
    fn identity(&self) -> (OperationId, OperationKind, Stage) {
        match self {
            Self::Edits { operation, .. } => (*operation, OperationKind::Edits, Stage::Requested),
            Self::Namespace { operation, .. } => {
                (*operation, OperationKind::Namespace, Stage::Requested)
            }
            Self::Commit { operation, .. } => (*operation, OperationKind::Commit, Stage::Requested),
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
    ) -> Result<(), LedgerError> {
        if self.prior(event)?.is_some() {
            return Err(LedgerError::Chain);
        }
        let (operation, kind, stage) = event.identity();
        match event {
            Event::Edits { expected, edits, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(publication) }
                    if matches!(publication.scope, PublicationScope::Directory { .. })) {
                    return Err(LedgerError::RebaseRequired);
                }
                if edits.is_empty() {
                    return Err(LedgerError::Chain);
                }
                admit_edits(source, edits)?;
                if matches!(self.namespace, NamespaceState::Edited { .. }) {
                    let view = self.projection(source)?;
                    if edits.iter().any(|edit| !view.contains_source_file(&edit.path)) {
                        return Err(LedgerError::Projection(
                            crate::directory_worktree::projection::ProjectionError::Source,
                        ));
                    }
                }
                let mut merged = self.edits.clone();
                for edit in edits {
                    if self.versions.contains(&edit.material.version_id) {
                        return Err(LedgerError::OperationConflict);
                    }
                    match merged.binary_search_by(|old| old.path.key().cmp(edit.path.key())) {
                        Ok(index) => merged[index] = edit.clone(),
                        Err(index) => merged.insert(index, edit.clone()),
                    }
                }
                admit_edits(source, &merged)?;
                if matches!(self.namespace, NamespaceState::Edited { .. }) {
                    self.projection_with_edits(source, &merged)?;
                }
                let manifest = revision_commitment(source, &merged, self.namespace.batches())?;
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
                let manifest = revision_commitment(source, &self.edits, &batches)?;
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
                    .collect::<Result<Vec<_>, _>>()?;
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
        let edits = edits.iter().map(EditRecord::file_edit).collect::<Result<Vec<_>, _>>()?;
        let view = SparseProjection::admit_with_results(source, edits, &self.file_results)
            .map_err(LedgerError::Projection)?;
        match &self.namespace {
            NamespaceState::Original => Ok(view),
            NamespaceState::Edited { view: namespace, .. } => {
                view.with_namespace(namespace).map_err(LedgerError::Namespace)
            }
        }
    }
    fn require_publication_scope(&self) -> Result<(), LedgerError> {
        // Publication still associates FILE results with original paths. Until
        // durable obligations cover removed and renamed occurrences, neither a
        // directory commit nor files-before-delete can consume this namespace.
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
fn revision_commitment(
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
fn event_commitment(event: &Event) -> Result<MaterialCommitment, LedgerError> {
    let bytes = storage_contract::canonical_json(event).map_err(LedgerError::Codec)?;
    Ok(commitment(&bytes))
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
    let converted = edits.iter().map(EditRecord::file_edit).collect::<Result<Vec<_>, _>>()?;
    SparseProjection::admit(source, converted).map_err(LedgerError::Projection)?;
    Ok(())
}
