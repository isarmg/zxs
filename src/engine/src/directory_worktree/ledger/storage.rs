mod published;
mod clean_mount;
mod historical_mounts;
pub(crate) use clean_mount::{MountFrame, install_mount_frame, observe_mount_frame};
mod content_material;
pub(crate) use content_material::ScopedFrames;

use crate::directory_worktree::projection::{FileResult, SparseProjection};
use crate::directory_worktree::projection::namespace::NamespaceChange;
use crate::name_budget::NameBudget;
use super::record::{Body, Checkpoint, Event, Format, Frame, Genesis};
use super::state::State;
use super::{AddedFile, AddedFileResult, LedgerError, MAX_FRAME_BYTES, MAX_FRAMES, MAX_LEDGER_BYTES};
use crate::directory_worktree::lifecycle::{
    Mount, Phase, Publication, PublicationScope, SnapshotSource, Worktree, WorktreeId,
};
use crate::directory_worktree::store::{HeldSparseWorktree, SparseWorktreeBinding};
use crate::disk::{DirectoryCreateOutcome, DiskRoot};
use crate::persist::layout::worktree as layout;
use crate::working_tree::records::{file_identity, local_entity};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotReference};
use data_serialization::{Deserialize, Serialize};
use platform_io::payload::Expectation;
use std::{collections::BTreeSet, io::Cursor};
use storage_contract::Sequence;
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment, SnapshotBranchId};

/// The independently retained anchor, never read from an unknown local marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Binding {
    worktree: WorktreeId,
    source: SnapshotReference,
    parent_entity: LocalEntityIdentity,
    ledger_entity: LocalEntityIdentity,
    genesis: MaterialCommitment,
}
impl Binding {
    pub(crate) fn ledger_entity(self) -> LocalEntityIdentity { self.ledger_entity }
    pub(crate) fn worktree(self) -> WorktreeId {
        self.worktree
    }

    pub(crate) fn source(self) -> SnapshotReference {
        self.source
    }

    pub(crate) fn initial_checkpoint(self) -> Checkpoint {
        Checkpoint { sequence: Sequence::new(0), commitment: self.genesis }
    }
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum LedgerPreparationError {
    #[error("worktree ledger preparation was rejected before creating its directory")]
    Rejected(#[source] LedgerError),
    #[error("worktree ledger preparation is incomplete; retain its exact instance and evidence")]
    Incomplete {
        worktree: WorktreeId,
        entity: Option<LocalEntityIdentity>,
        #[source]
        source: LedgerError,
    },
}
#[derive(Clone, Copy)]
enum AppendState { Ready, Uncertain }
#[derive(Clone, Copy)]
enum EditAccess { Seal, ReadOnly }

/// Pending publication is metadata for the original cross-domain publisher,
/// not an admitted readable/attachable view. No ordinary ledger escapes it.
pub(crate) enum RecoveredLedger<'a> {
    Ready(HeldWorktreeLedger<'a>),
    Publishing(PendingPublication<'a>),
}
pub(crate) struct PendingPublication<'a> {
    ledger: HeldWorktreeLedger<'a>,
}
impl PendingPublication<'_> {
    pub(crate) fn worktree(&self) -> &Worktree {
        self.ledger.worktree()
    }
    pub(crate) fn edits(&self) -> &[super::record::EditRecord] {
        self.ledger.edits()
    }
    pub(crate) fn file_results(&self) -> &[FileResult] {
        self.ledger.file_results()
    }
    pub(crate) fn dirty_edits(&self) -> Vec<&super::record::EditRecord> {
        self.ledger.dirty_edits()
    }
    /// Resume partial body transfers using the publisher's independent decision
    /// first. Only its exact verified permanent result can finish this ledger.
    pub(crate) fn finish(
        &mut self,
        publication: Publication,
        files: Vec<FileResult>,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<Checkpoint, LedgerError> {
        self.ledger.append(Event::Completed { publication, files }, verify)
    }
}

pub(crate) struct HeldWorktreeLedger<'a> {
    source: &'a HeldSnapshotNode,
    directory: DiskRoot,
    binding: Binding,
    instance: SparseWorktreeBinding,
    checkpoint: Checkpoint,
    bytes: usize,
    state: State,
    append: AppendState,
}
impl<'a> HeldWorktreeLedger<'a> {
    /// Reserve source/branch/mount in the owning repository before this call.
    /// The resulting ledger must be registered by that same independent owner.
    pub(crate) fn prepare(
        source: &'a HeldSnapshotNode,
        instance: SparseWorktreeBinding,
        branch: SnapshotBranchId,
        mount: Mount,
    ) -> Result<Self, LedgerPreparationError> {
        if source.manifest().reference() != instance.source() {
            return Err(LedgerPreparationError::Rejected(LedgerError::Binding));
        }
        HeldSparseWorktree::reopen(source, instance)
            .map_err(LedgerError::Instance).map_err(LedgerPreparationError::Rejected)?;
        let genesis = Genesis {
            binding: instance,
            source: SnapshotSource {
                repository: instance.source().repository,
                node: instance.source().node,
                branch,
                manifest: instance.source().manifest,
            },
            mount,
        };
        let frame = Frame {
            format: Format::WorktreeTransitions,
            sequence: Sequence::new(0),
            previous: None,
            body: Body::Genesis(genesis.clone()),
        };
        let bytes = frame.bytes().map_err(LedgerPreparationError::Rejected)?;
        let checkpoint = frame.checkpoint(&bytes);
        let parent = source.worktrees_directory();
        let _activity = parent.retain_io_activity().map_err(LedgerError::Disk)
            .map_err(LedgerPreparationError::Rejected)?;
        let parent_entity = parent.held_identity().map(local_entity)
            .map_err(LedgerError::Directory).map_err(LedgerPreparationError::Rejected)?;
        let worktree = instance.worktree();
        let directory = match parent.create_private_entry(&layout::ledger(worktree)) {
            DirectoryCreateOutcome::Rejected(error) => {
                return Err(LedgerPreparationError::Rejected(LedgerError::Directory(error)));
            }
            DirectoryCreateOutcome::Uncertain { identity, source } => {
                return Err(LedgerPreparationError::Incomplete {
                    worktree, entity: identity.map(local_entity),
                    source: LedgerError::Directory(source),
                });
            }
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(|source| LedgerPreparationError::Incomplete {
                    worktree, entity: Some(local_entity(identity)),
                    source: LedgerError::Directory(source),
                })?;
                directory
            }
        };
        let entity = directory.held_identity().map(local_entity)
            .map_err(|source| LedgerPreparationError::Incomplete {
                worktree, entity: None, source: LedgerError::Directory(source),
            })?;
        write_frame(&directory, &bytes, checkpoint).map_err(|source| {
            LedgerPreparationError::Incomplete { worktree, entity: Some(entity), source }
        })?;
        parent.sync_private_entry_matching(&layout::ledger(worktree), file_identity(entity))
            .map_err(|source| LedgerPreparationError::Incomplete {
                worktree, entity: Some(entity),
                source: LedgerError::Persisted { intended: checkpoint, source },
            })?;
        Ok(Self {
            source,
            directory,
            binding: Binding {
                worktree, source: instance.source(), parent_entity,
                ledger_entity: entity, genesis: checkpoint.commitment,
            },
            instance,
            checkpoint,
            bytes: bytes.len(),
            state: State::new(&genesis),
            append: AppendState::Ready,
        })
    }

    /// Exact checkpoint recovery: an unregistered tail is not auto-adopted.
    /// The owner must reconcile it with its original operation decision first.
    pub(crate) fn reopen(
        source: &'a HeldSnapshotNode,
        binding: Binding,
        expected: Checkpoint,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<Self, LedgerError> {
        match Self::recover(source, binding, expected, verify)? {
            RecoveredLedger::Ready(ledger) => Ok(ledger),
            RecoveredLedger::Publishing(_) => Err(LedgerError::PublicationPending),
        }
    }

    pub(crate) fn recover(
        source: &'a HeldSnapshotNode,
        binding: Binding,
        expected: Checkpoint,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<RecoveredLedger<'a>, LedgerError> {
        Self::recover_with_added(source, binding, expected, verify,
            |_, _| Err(LedgerError::PublicationPending))
    }

    pub(crate) fn recover_with_added(
        source: &'a HeldSnapshotNode,
        binding: Binding,
        expected: Checkpoint,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
        verify_added: impl FnMut(&Publication, &AddedFileResult) -> Result<(), LedgerError>,
    ) -> Result<RecoveredLedger<'a>, LedgerError> {
        Self::recover_frames(source, binding, expected, verify, verify_added, None, None)
    }

    // Active ledger reads normally require all edits still at the source.
    // A natively FINALIZED batch has proved the complete installed destination
    // before appending its two frames; only that call may read this older prefix.
    fn recover_frames(
        source: &'a HeldSnapshotNode, binding: Binding, expected: Checkpoint,
        mut verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
        mut verify_added: impl FnMut(&Publication, &AddedFileResult) -> Result<(), LedgerError>,
        installing: Option<&crate::engine::FinalizedOriginalContent<'_>>,
        suffix: Option<(Checkpoint, usize)>,
    ) -> Result<RecoveredLedger<'a>, LedgerError> {
        if source.manifest().reference() != binding.source
            || expected.sequence.get() >= MAX_FRAMES
        {
            return Err(LedgerError::Binding);
        }
        let directory = open_bound_directory(source.worktrees_directory(), binding)?;
        verify_inventory(&directory, suffix.map_or(expected, |s| s.0))?;
        let mut previous = None;
        let mut total = 0usize;
        let mut state = None;
        let mut instance = None;
        for index in 0..=expected.sequence.get() {
            let sequence = Sequence::new(index);
            let bytes = directory.read_entry_bytes(&layout::frame(sequence), MAX_FRAME_BYTES)
                .map_err(LedgerError::Disk)?;
            total = bounded_total(total, bytes.len())?;
            let frame: Frame = storage_contract::decode_strict(&bytes).map_err(LedgerError::Codec)?;
            if frame.sequence != sequence || frame.previous != previous || frame.bytes()? != bytes {
                return Err(LedgerError::Chain);
            }
            let checkpoint = frame.checkpoint(&bytes);
            match frame.body {
                Body::Genesis(genesis) => {
                    if index != 0 || checkpoint.commitment != binding.genesis
                        || genesis.binding.worktree() != binding.worktree
                        || genesis.binding.source() != binding.source
                        || genesis.source.repository != binding.source.repository
                        || genesis.source.node != binding.source.node
                        || genesis.source.manifest != binding.source.manifest
                    {
                        return Err(LedgerError::Binding);
                    }
                    instance = Some(genesis.binding);
                    state = Some(State::new(&genesis));
                }
                Body::Event(event) => {
                    let admitted = state.as_mut().ok_or(LedgerError::Chain)?;
                    admitted.apply(source.manifest(), &event, checkpoint,
                        &mut verify, &mut verify_added)?;
                }
            }
            previous = Some(checkpoint.commitment);
        }
        if let Some((_, extra)) = suffix { bounded_total(total, extra)?; }
        if previous != Some(expected.commitment) {
            return Err(LedgerError::Chain);
        }
        let instance = instance.ok_or(LedgerError::Chain)?;
        HeldSparseWorktree::reopen(source, instance).map_err(LedgerError::Instance)?;
        let ledger = Self {
            source, directory, binding, instance, checkpoint: expected, bytes: total,
            state: state.ok_or(LedgerError::Chain)?, append: AppendState::Ready,
        };
        if matches!(ledger.state.worktree.phase(), Phase::Publishing { .. }) {
            return Ok(RecoveredLedger::Publishing(PendingPublication { ledger }));
        }
        let saved = match ledger.state.worktree.phase() {
            Phase::Active { saved } | Phase::Publishing { saved, .. } => saved,
            Phase::Deleted { publication, .. } => Some(publication),
        };
        let moved = saved.is_some_and(|publication| {
            publication.intent.revision == ledger.state.worktree.revision()
                && matches!(publication.scope, PublicationScope::Directory { .. })
        });
        if !moved && installing.is_none() {
            ledger.verify_edits(&ledger.state.edits, EditAccess::ReadOnly)?;
        }
        Ok(RecoveredLedger::Ready(ledger))
    }

    /// The owner holds the complete namespace/branch fence and has sealed edits.
    /// Success is durable local intent, not publication of any FILE or SMB state.
    /// Retain the returned checkpoint in the owning decision before exposing it.
    pub(crate) fn append(
        &mut self,
        event: Event,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<Checkpoint, LedgerError> {
        self.append_with_added(event, verify, |_, _| Err(LedgerError::PublicationPending))
    }

    pub(crate) fn append_with_added(
        &mut self,
        event: Event,
        mut verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
        mut verify_added: impl FnMut(&Publication, &AddedFileResult) -> Result<(), LedgerError>,
    ) -> Result<Checkpoint, LedgerError> {
        if matches!(self.append, AppendState::Uncertain) {
            return Err(LedgerError::Uncertain);
        }
        self.append = AppendState::Uncertain;
        self.check_current_location()?;
        let parent = self.source.worktrees_directory();
        self.append = AppendState::Ready;
        if let Event::Edits { edits, .. } = &event {
            if edits.len() > self.source.manifest().entries().len() {
                return Err(LedgerError::Budget);
            }
            let mut path_bytes = 0usize;
            for edit in edits {
                path_bytes = path_bytes.checked_add(edit.path.as_str().len())
                    .and_then(|bytes| bytes.checked_add(edit.leaf.as_str().len()))
                    .filter(|bytes| *bytes <= 2 * 1024 * 1024).ok_or(LedgerError::Budget)?;
            }
        }
        if let Event::Completed { files, .. } = &event {
            if files.len() > self.state.edits.len() {
                return Err(LedgerError::Budget);
            }
            let mut path_bytes = 0usize;
            for file in files {
                path_bytes = path_bytes.checked_add(file.path.as_str().len())
                    .filter(|bytes| *bytes <= 1024 * 1024).ok_or(LedgerError::Budget)?;
            }
        }
        if self.state.prior(&event)?.is_some() {
            return Ok(self.checkpoint);
        }
        let (bytes, checkpoint) = event_frame(self.checkpoint, &event)?;
        let total = bounded_total(self.bytes, bytes.len())?;
        let mut next = self.state.clone();
        next.apply(self.source.manifest(), &event, checkpoint,
            &mut verify, &mut verify_added)?;
        match &event {
            Event::Edits { edits, .. } => self.verify_edits(edits, EditAccess::Seal)?,
            Event::Completed { publication, .. }
                if matches!(publication.scope, PublicationScope::FilesOnly) => {
                    self.verify_edits(&self.state.edits, EditAccess::ReadOnly)?;
                }
            Event::Namespace { changes, .. } => self.verify_prospective_names(changes)?,
            Event::File { file, .. } => {
                self.verify_prospective_paths(std::iter::once(&file.target.path))?;
            }
            Event::Commit { .. } | Event::CommitAdded { .. } | Event::AddedCompleted { .. }
                | Event::Delete { .. } | Event::Restored { .. } | Event::Completed { .. } => {}
        }
        self.append = AppendState::Uncertain;
        write_frame(&self.directory, &bytes, checkpoint)?;
        parent.sync_private_entry_matching(
            &layout::ledger(self.binding.worktree), file_identity(self.binding.ledger_entity),
        ).map_err(|source| LedgerError::Persisted { intended: checkpoint, source })?;
        self.state = next;
        self.checkpoint = checkpoint;
        self.bytes = total;
        self.append = AppendState::Ready;
        Ok(checkpoint)
    }
    fn verify_prospective_names(&self, changes: &[NamespaceChange]) -> Result<(), LedgerError> {
        let paths = changes.iter().filter_map(|change| match change {
            NamespaceChange::AddDirectory { path } => Some(path),
            NamespaceChange::Rename { target, .. } => Some(target),
            NamespaceChange::RemoveSubtree { .. } => None,
        });
        self.verify_prospective_paths(paths)
    }
    fn verify_prospective_paths<'p>(
        &self,
        mut paths: impl Iterator<Item = &'p name_policy::PortablePath>,
    ) -> Result<(), LedgerError> {
        let Some(first) = paths.next() else {
            return Ok(());
        };
        let instance = HeldSparseWorktree::reopen(self.source, self.instance)
            .map_err(LedgerError::Instance)?;
        let maximum = instance.directory().component_name_maximum().map_err(LedgerError::Disk)?;
        let budget = NameBudget::from_maximum(maximum).map_err(LedgerError::NameBudget)?;
        // Only new durable intents are admitted against today's filesystem budget.
        for path in std::iter::once(first).chain(paths) {
            budget.component(path.file_name(), path.as_str()).map_err(LedgerError::NameBudget)?;
        }
        Ok(())
    }

    fn verify_edits(
        &self, edits: &[super::record::EditRecord], access: EditAccess,
    ) -> Result<(), LedgerError> {
        let instance = HeldSparseWorktree::reopen(self.source, self.instance)
            .map_err(LedgerError::Instance)?;
        for edit in edits {
            let name = layout::edited_file(edit.material.version_id, &edit.path);
            if &name != edit.leaf.entry() {
                return Err(LedgerError::Binding);
            }
            let edit = edit.file_edit();
            let file = instance.open_material(&edit.material)
                .map_err(LedgerError::Instance)?;
            if matches!(access, EditAccess::Seal) {
                file.sync_all().map_err(LedgerError::Io)?;
            }
        }
        if matches!(access, EditAccess::Seal) {
            let identity = instance.directory().held_identity().map_err(LedgerError::Directory)?;
            self.source.worktrees_directory().sync_private_entry_matching(
                &layout::instance(self.instance.worktree()), identity,
            ).map_err(LedgerError::Directory)?;
        }
        Ok(())
    }
    /// Read/list consumers must not expose the cached state after an uncertain
    /// append, replacement of the bound directory, or another writer's advance.
    pub(crate) fn admit_read_view(&self) -> Result<(), LedgerError> {
        if matches!(self.append, AppendState::Uncertain) {
            return Err(LedgerError::Uncertain);
        }
        self.check_current_location()
    }
    fn check_current_location(&self) -> Result<(), LedgerError> {
        // Revalidate the name binding; inventory checks continue on the held directory.
        open_bound_directory(self.source.worktrees_directory(), self.binding)?;
        verify_inventory(&self.directory, self.checkpoint)?;
        let bytes = self.directory.read_entry_bytes(
            &layout::frame(self.checkpoint.sequence), MAX_FRAME_BYTES,
        ).map_err(LedgerError::Disk)?;
        if super::record::commitment(&bytes) != self.checkpoint.commitment {
            return Err(LedgerError::Chain);
        }
        Ok(())
    }
    /// Complete preflight before publishing the native FILE decision. This
    /// retains no new checkpoint and writes nothing; the same append admission
    /// is repeated after that decision before its independent completion.
    pub(crate) fn check_file(
        &self,
        operation: storage_contract::OperationId,
        expected: crate::directory_worktree::lifecycle::Revision,
        file: &AddedFile,
    ) -> Result<(), LedgerError> {
        self.admit_read_view()?;
        let event = Event::File { operation, expected, file: file.clone() };
        if self.state.prior(&event)?.is_some() {
            return Ok(());
        }
        let (bytes, checkpoint) = event_frame(self.checkpoint, &event)?;
        bounded_total(self.bytes, bytes.len())?;
        self.state.clone().apply(
            self.source.manifest(), &event, checkpoint, &mut |_, _| Err(LedgerError::FileResults),
            &mut |_, _| Err(LedgerError::FileResults),
        )?;
        self.verify_prospective_paths(std::iter::once(&file.target.path))
    }

    /// Append only after this exact FILE birth/save's independent native decision.
    /// The owning publisher must decide CompleteFile before exposing the checkpoint.
    pub(crate) fn file(
        &mut self,
        operation: storage_contract::OperationId,
        expected: crate::directory_worktree::lifecycle::Revision,
        file: AddedFile,
    ) -> Result<Checkpoint, LedgerError> {
        self.append(Event::File { operation, expected, file }, |_, _| Err(LedgerError::FileResults))
    }
    /// Exact normal scope is checked before reserving the original operation.
    pub(crate) fn check_single_added(&self, expected: &AddedFile) -> Result<(), LedgerError> {
        self.admit_read_view()?;
        self.state.require_added_scope()?;
        if self.state.added.first() != Some(expected) {
            return Err(LedgerError::AddedFile);
        }
        Ok(())
    }

    /// Pure, complete two-frame admission before the owner publishes the first
    /// FILE. Local structural planning deliberately cannot certify that receipt.
    pub(crate) fn check_added_publication(
        &self,
        commit: &Event,
        publication: Publication,
        file: &AddedFileResult,
    ) -> Result<(), LedgerError> {
        self.admit_read_view()?;
        if !matches!(commit, Event::CommitAdded { .. }) {
            return Err(LedgerError::FileResults);
        }
        let mut next = self.state.clone();
        let (checkpoint, total) = match self.state.prior(commit)? {
            Some(checkpoint) if checkpoint == self.checkpoint => (checkpoint, self.bytes),
            Some(_) => return Err(LedgerError::FileResults),
            None => {
                let (bytes, checkpoint) = event_frame(self.checkpoint, commit)?;
                let total = bounded_total(self.bytes, bytes.len())?;
                next.apply(self.source.manifest(), commit, checkpoint,
                    &mut |_, _| Err(LedgerError::FileResults),
                    &mut |_, _| Err(LedgerError::FileResults))?;
                (checkpoint, total)
            }
        };
        let completion = Event::AddedCompleted { publication, file: file.clone() };
        let (bytes, completed) = event_frame(checkpoint, &completion)?;
        bounded_total(total, bytes.len())?;
        next.apply(self.source.manifest(), &completion, completed,
            &mut |_, _| Err(LedgerError::FileResults), &mut |_, _| Ok(()))?;
        Ok(())
    }
    pub(crate) fn added_file_results(&self) -> &[AddedFileResult] {
        &self.state.added_results
    }
    pub(crate) fn added_files(&self) -> &[AddedFile] {
        &self.state.added
    }
    pub(crate) fn binding(&self) -> Binding {
        self.binding
    }
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }
    pub(crate) fn instance(&self) -> SparseWorktreeBinding {
        self.instance
    }
    pub(crate) fn worktree(&self) -> &Worktree {
        &self.state.worktree
    }
    pub(crate) fn file_results(&self) -> &[FileResult] {
        &self.state.file_results
    }
    pub(crate) fn require_existing_content_scope(&self) -> Result<(), LedgerError> {
        self.state.require_publication_scope()
    }
    pub(crate) fn dirty_edits(&self) -> Vec<&super::record::EditRecord> {
        self.state.dirty_edits()
    }
    pub(crate) fn projection(&self) -> Result<SparseProjection<'_>, LedgerError> {
        self.state.projection(self.source.manifest())
    }
    pub(crate) fn edits(&self) -> &[super::record::EditRecord] {
        &self.state.edits
    }
}

fn open_bound_directory(parent: &DiskRoot, binding: Binding) -> Result<DiskRoot, LedgerError> {
    if parent.held_identity().map_err(LedgerError::Directory)?
        != file_identity(binding.parent_entity)
    {
        return Err(LedgerError::Binding);
    }
    parent.private_entry_matching(
        &layout::ledger(binding.worktree), file_identity(binding.ledger_entity),
    ).map_err(LedgerError::Directory)
}

fn bounded_total(before: usize, additional: usize) -> Result<usize, LedgerError> {
    before.checked_add(additional).filter(|total| *total <= MAX_LEDGER_BYTES)
        .ok_or(LedgerError::Budget)
}

fn event_frame(before: Checkpoint, event: &Event) -> Result<(Vec<u8>, Checkpoint), LedgerError> {
    let sequence = before.next_frame_sequence()?;
    let frame = Frame {
        format: Format::WorktreeTransitions, sequence,
        previous: Some(before.commitment), body: Body::Event(event.clone()),
    };
    let bytes = frame.bytes()?;
    let checkpoint = frame.checkpoint(&bytes);
    Ok((bytes, checkpoint))
}
fn write_frame(directory: &DiskRoot, bytes: &[u8], checkpoint: Checkpoint)
    -> Result<(), LedgerError>
{
    directory.copy_new_entry_verified_synced(
        &layout::frame(checkpoint.sequence), &mut Cursor::new(bytes),
        Expectation { bytes: bytes.len() as u64, blake3: Some(blake3::hash(bytes)) },
    ).map_err(|source| LedgerError::Append { intended: checkpoint, source })?;
    Ok(())
}
fn verify_inventory(directory: &DiskRoot, checkpoint: Checkpoint) -> Result<(), LedgerError> {
    let mut required: BTreeSet<_> = (0..=checkpoint.sequence.get())
        .map(|value| layout::frame(Sequence::new(value))).collect();
    for name in directory.root_entries().map_err(LedgerError::Directory)? {
        if !required.remove(&name.map_err(LedgerError::Inventory)?) {
            return Err(LedgerError::Chain);
        }
    }
    if !required.is_empty() {
        return Err(LedgerError::Chain);
    }
    Ok(())
}

/// Hash the actual two canonical frame encodings, not sequence arithmetic.
pub(crate) fn mount_checkpoint(
    plan: &crate::persist::snapshot_worktree::mount_change::Reservation,
) -> Result<Checkpoint, LedgerError> {
    event_frame(plan.before, &plan.event()).map(|(_, checkpoint)| checkpoint)
}

pub(crate) fn content_checkpoints(before: Checkpoint, events: &[Event; 2])
    -> Result<[Checkpoint; 2], LedgerError>
{
    let (_, committing) = event_frame(before, &events[0])?;
    let (_, completed) = event_frame(committing, &events[1])?;
    Ok([committing, completed])
}

impl HeldWorktreeLedger<'_> {
    /// The on-media ledger owns cumulative bytes and current phase. Checking
    /// them cannot advance the inode or manufacture permission to append.
    pub(crate) fn preview_content(&self, events: &[Event; 2])
        -> Result<[Checkpoint; 2], LedgerError>
    {
        self.state.require_publication_scope()?;
        let (first, committing) = event_frame(self.checkpoint, &events[0])?;
        let (second, completed) = event_frame(committing, &events[1])?;
        bounded_total(bounded_total(self.bytes, first.len())?, second.len())?;
        let Event::Completed { publication: expected, files: expected_files } = &events[1]
            else { return Err(LedgerError::FileResults); };
        let mut logical = self.state.clone();
        let mut verify = |p: &Publication, files: &[FileResult]| {
            if p == expected && files == expected_files { Ok(()) } else { Err(LedgerError::FileResults) }
        };
        for (event, checkpoint) in events.iter().zip([committing, completed]) {
            logical.apply(self.source.manifest(), event, checkpoint, &mut verify,
                &mut |_, _| Err(LedgerError::FileResults))?;
        }
        Ok([committing, completed])
    }
}

pub(crate) fn content_frame_bytes(before: Checkpoint, events: &[Event; 2]) -> Result<u64, LedgerError> {
    let (first, next) = event_frame(before, &events[0])?;
    let (second, _) = event_frame(next, &events[1])?;
    Ok(bounded_total(first.len(), second.len())? as u64)
}

#[cfg(test)]
pub(crate) fn mount_test_frame(plan: &crate::persist::snapshot_worktree::mount_change::Reservation)
    -> Result<Vec<u8>,LedgerError> {
    event_frame(plan.before, &plan.event()).map(|(bytes,_)| bytes)
}
