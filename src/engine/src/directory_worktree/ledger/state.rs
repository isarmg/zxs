use super::record::{Checkpoint, EditRecord, Event, Genesis, commitment};
use super::LedgerError;
use crate::directory_worktree::lifecycle::{
    Phase, Publication, PublicationMode, Worktree,
};
use crate::directory_worktree::projection::SparseProjection;
use crate::working_tree::snapshot::AdmittedSnapshotManifest;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::OperationId;
use storage_contract::working_tree::MaterialCommitment;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind { Edits, Commit, Delete, Restore }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Stage { Requested, Completed }
impl Event {
    fn identity(&self) -> (OperationId, OperationKind, Stage) {
        match self {
            Self::Edits { operation, .. } => (*operation, OperationKind::Edits, Stage::Requested),
            Self::Commit { operation, .. } => (*operation, OperationKind::Commit, Stage::Requested),
            Self::Delete { operation, .. } => (*operation, OperationKind::Delete, Stage::Requested),
            Self::Restored { operation, .. } => {
                (*operation, OperationKind::Restore, Stage::Requested)
            }
            Self::Completed { publication } => (
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
pub(super) struct State {
    pub(super) worktree: Worktree,
    pub(super) edits: Vec<EditRecord>,
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
        verify: &mut impl FnMut(&Publication) -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        if self.prior(event)?.is_some() {
            return Err(LedgerError::Chain);
        }
        let (operation, kind, stage) = event.identity();
        match event {
            Event::Edits { expected, edits, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(_) }) {
                    return Err(LedgerError::RebaseRequired);
                }
                if edits.is_empty() {
                    return Err(LedgerError::Chain);
                }
                admit_edits(source, edits)?;
                let mut merged = self.edits.clone();
                for edit in edits {
                    if self.versions.contains(&edit.material.version_id) {
                        return Err(LedgerError::OperationConflict);
                    }
                    match merged.binary_search_by(|old| old.path.key().cmp(edit.path.key())) {
                        Ok(index) => {
                            if merged[index].material.version_id == edit.material.version_id {
                                return Err(LedgerError::OperationConflict);
                            }
                            merged[index] = edit.clone();
                        }
                        Err(index) => merged.insert(index, edit.clone()),
                    }
                }
                admit_edits(source, &merged)?;
                let bytes = storage_contract::canonical_json(&(source.reference(), &merged))
                    .map_err(LedgerError::Codec)?;
                self.worktree.record_edit(commitment(&bytes)).map_err(LedgerError::Lifecycle)?;
                self.edits = merged;
                self.versions.extend(edits.iter().map(|edit| edit.material.version_id));
            }
            Event::Commit { expected, current_head, fork_branch, .. } => {
                self.expected(*expected)?;
                if matches!(self.worktree.phase(), Phase::Active { saved: Some(_) }) {
                    return Err(LedgerError::RebaseRequired);
                }
                self.worktree.prepare_commit(operation, *current_head, *fork_branch)
                    .map_err(LedgerError::Lifecycle)?;
            }
            Event::Delete { expected, .. } => {
                self.expected(*expected)?;
                self.worktree.prepare_delete(operation).map_err(LedgerError::Lifecycle)?;
            }
            Event::Completed { publication } => {
                let mut next = self.worktree.clone();
                next.complete_publication(*publication).map_err(LedgerError::Lifecycle)?;
                // A JSON completion field never publishes FILE or directory nodes.
                verify(publication)?;
                self.worktree = next;
            }
            Event::Restored { mount, .. } => {
                self.worktree.restore(mount.clone(), false).map_err(LedgerError::Lifecycle)?;
            }
        }
        self.operations.insert(operation, kind);
        self.results.insert((operation, stage), (event_commitment(event)?, checkpoint));
        Ok(())
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
