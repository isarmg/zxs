use crate::directory_worktree::ledger::{Binding, Checkpoint};
use crate::persist::snapshot_worktree::{
    self, Decoded, HostEvidence, NamespaceDirectoryBinding, NodeAllocation, NodeCompletion,
    NodeReservation, Record,
    WorktreeCompletion, WorktreeReservation,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotNodeBinding, SnapshotReference,
};
use crate::{OperationResult, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::snapshot_worktree::{
    CreateWorktree, InitializeSnapshot, NamespaceDirectoryId, SnapshotHandle,
    WorktreeHandle, WorktreeId,
};
use storage_contract::working_tree::{
    BindingLifecycle, DirectoryId, LocalEntityIdentity, MaterialCommitment, RootId,
    SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchName, EntryId, OperationId, Principal, RepositoryId, Sequence};

mod backup;
pub(crate) use backup::require_backup_snapshot_scope;
mod edits;
mod commits;
use commits::CommitRecord;
pub(super) use commits::{CommitRetry, verify_file_publication};
pub(crate) use commits::PublishedCommitTail;
use edits::{EditRecordEntry, LedgerHead};

/// This is a relation among independently admitted SYSTEM receipts. It holds
/// neither a directory handle nor namespace/write authority. Uncompleted
/// reservations remain occupied and cannot be replaced by local observations.
pub(super) struct ReceiptIndex<'a> {
    source: &'a State,
    nodes: BTreeMap<OperationId, NodeRecord<'a>>,
    worktrees: BTreeMap<OperationId, WorktreeRecord<'a>>,
    completions: BTreeMap<OperationId, CompletionOwner>,
    directories: BTreeMap<(RootId, EntryId), RegisteredDirectory>,
    directory_ids: BTreeMap<DirectoryId, (RootId, EntryId)>,
    directory_entities: BTreeMap<(RootId, LocalEntityIdentity), EntryId>,
    edits: BTreeMap<OperationId, EditRecordEntry<'a>>,
    heads: BTreeMap<WorktreeId, LedgerHead>,
    pending_edits: BTreeMap<WorktreeId, OperationId>,
    commits: BTreeMap<OperationId, CommitRecord<'a>>,
    branch_heads: BTreeMap<(SnapshotRepositoryId, SnapshotBranchId), SnapshotNodeId>,
    pending_branches: BTreeMap<(SnapshotRepositoryId, SnapshotBranchId), OperationId>,
    planned_files: BTreeSet<OperationId>,
}

struct RegisteredDirectory {
    repository: RepositoryId,
    binding: NamespaceDirectoryBinding,
    reservation: OperationId,
    complete: bool,
}

/// Only this index over exact admitted SYSTEM receipts constructs the view.
/// Pending rows are occupancy, not a source from which another writer may grow.
pub(crate) struct RegisteredNamespaceDirectories<'a> {
    index: &'a ReceiptIndex<'a>,
}

pub(crate) enum DirectoryRegistration<'a> {
    Published(&'a NamespaceDirectoryBinding),
    Pending(OperationId),
    Conflict,
}

impl RegisteredNamespaceDirectories<'_> {
    pub(crate) fn source(&self) -> &State {
        self.index.source
    }

    pub(crate) fn contains_id(&self, id: DirectoryId) -> bool {
        self.index.directory_ids.contains_key(&id)
    }

    pub(crate) fn get(
        &self,
        root: RootId,
        repository: RepositoryId,
        entry: EntryId,
    ) -> Option<DirectoryRegistration<'_>> {
        self.index.directories.get(&(root, entry)).map(|registered| {
            if registered.repository != repository {
                DirectoryRegistration::Conflict
            } else if registered.complete {
                DirectoryRegistration::Published(&registered.binding)
            } else {
                DirectoryRegistration::Pending(registered.reservation)
            }
        })
    }
}

struct Receipt<'a> {
    original: &'a OperationResult,
    principal: Principal,
}

enum Publication<T> {
    Reserved,
    Complete(T),
}

struct NodePlan {
    request: InitializeSnapshot,
    host: HostEvidence,
    allocated: NodeAllocation,
    branch_name: BranchName,
    material: SnapshotReference,
}

struct NodeRecord<'a> {
    receipt: Receipt<'a>,
    plan: NodePlan,
    manifest: AdmittedSnapshotManifest,
    commitment: MaterialCommitment,
    publication: Publication<NodeCompletion>,
}

struct WorktreeRecord<'a> {
    receipt: Receipt<'a>,
    plan: WorktreeReservation,
    commitment: MaterialCommitment,
    publication: Publication<WorktreeCompletion>,
}

#[derive(Clone, Copy)]
enum CompletionOwner {
    Node(OperationId),
    Worktree(OperationId),
    Edit(OperationId),
    CommitNode(OperationId),
    Commit,
}
impl CompletionOwner {
    fn kind(self) -> &'static str {
        match self {
            Self::Node(_) => snapshot_worktree::NODE_COMPLETE,
            Self::Worktree(_) => snapshot_worktree::WORKTREE_COMPLETE,
            Self::Edit(_) => snapshot_worktree::EDIT_COMPLETE,
            Self::CommitNode(_) => snapshot_worktree::COMMIT_NODE,
            Self::Commit => snapshot_worktree::COMMIT_COMPLETE,
        }
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct NameKey(String);

#[derive(Default)]
struct Allocations {
    repositories: BTreeSet<SnapshotRepositoryId>,
    nodes: BTreeSet<SnapshotNodeId>,
    branches: BTreeSet<SnapshotBranchId>,
    worktrees: BTreeSet<WorktreeId>,
    hosts: BTreeSet<(RootId, DirectoryId)>,
    mounts: BTreeSet<(RootId, DirectoryId, NameKey)>,
    edit_versions: BTreeSet<storage_contract::VersionId>,
}

pub(super) struct PublishedNode<'a> {
    plan: &'a NodePlan,
    manifest: &'a AdmittedSnapshotManifest,
    binding: SnapshotNodeBinding,
}

impl PublishedNode<'_> {
    pub(super) fn handle(&self) -> SnapshotHandle {
        SnapshotHandle {
            publication: self.plan.allocated.completion,
            repository_id: self.plan.allocated.repository,
            branch_id: self.plan.allocated.branch,
            node_id: self.plan.allocated.node,
        }
    }

    pub(super) fn initialization(&self) -> &InitializeSnapshot {
        &self.plan.request
    }

    pub(super) fn host(&self) -> &HostEvidence {
        &self.plan.host
    }

    pub(super) fn manifest(&self) -> &AdmittedSnapshotManifest {
        self.manifest
    }

    pub(super) fn branch_name(&self) -> &BranchName {
        &self.plan.branch_name
    }

    pub(super) fn binding(&self) -> SnapshotNodeBinding {
        self.binding
    }
}

pub(super) struct PublishedWorktree<'a> {
    plan: &'a WorktreeReservation,
    source: PublishedNode<'a>,
    binding: Binding,
    checkpoint: Checkpoint,
    revision: storage_contract::snapshot_worktree::WorktreeRevision,
    publication: Option<&'a CommitRecord<'a>>,
}

impl PublishedWorktree<'_> {
    pub(super) fn handle(&self) -> WorktreeHandle {
        WorktreeHandle {
            publication: self.plan.allocated.completion,
            worktree_id: self.plan.allocated.worktree,
        }
    }

    pub(super) fn reservation(&self) -> &WorktreeReservation {
        self.plan
    }

    pub(super) fn source(&self) -> &PublishedNode<'_> {
        &self.source
    }

    pub(super) fn binding(&self) -> Binding {
        self.binding
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }

    pub(super) fn revision(&self) -> storage_contract::snapshot_worktree::WorktreeRevision {
        self.revision
    }

    pub(super) fn require_publication(
        &self,
        publication: &crate::directory_worktree::lifecycle::Publication,
        files: &[crate::directory_worktree::projection::FileResult],
    ) -> std::result::Result<(), crate::directory_worktree::ledger::LedgerError> {
        let entry = self.publication
            .ok_or(crate::directory_worktree::ledger::LedgerError::PublicationPending)?;
        if *publication != entry.plan.publication()
            || files != std::slice::from_ref(&entry.file_result)
        {
            return Err(crate::directory_worktree::ledger::LedgerError::FileResults);
        }
        Ok(())
    }
}

impl<'a> ReceiptIndex<'a> {
    pub(super) fn retry_worktree(
        &self,
        request: &CreateWorktree,
        principal: &Principal,
    ) -> Result<Option<WorktreeHandle>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let tree = self.worktrees.get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if &tree.plan.request != request || &tree.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        let Publication::Complete(_) = &tree.publication else {
            return Err(RegistrationError::Pending(request.operation_id));
        };
        Ok(Some(WorktreeHandle {
            publication: tree.plan.allocated.completion,
            worktree_id: tree.plan.allocated.worktree,
        }))
    }

    /// A retry may only return a completed independent receipt. A failed reserve
    /// never scans local material or mints an in-process continuation.
    pub(super) fn retry_initialization(
        &self,
        request: &InitializeSnapshot,
        principal: &Principal,
    ) -> Result<Option<SnapshotHandle>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        let node = self.nodes.get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if &node.plan.request != request || &node.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        let Publication::Complete(_) = &node.publication else {
            return Err(RegistrationError::Pending(request.operation_id));
        };
        Ok(Some(SnapshotHandle {
            publication: node.plan.allocated.completion,
            repository_id: node.plan.allocated.repository,
            branch_id: node.plan.allocated.branch,
            node_id: node.plan.allocated.node,
        }))
    }

    pub(super) fn capture(source: &'a State) -> Result<Self> {
        let records = ordered_records(source)?;
        require_events(source, &records)?;
        let mut index = Self {
            source,
            nodes: BTreeMap::new(),
            worktrees: BTreeMap::new(),
            completions: BTreeMap::new(),
            directories: BTreeMap::new(),
            directory_ids: BTreeMap::new(),
            directory_entities: BTreeMap::new(),
            edits: BTreeMap::new(),
            heads: BTreeMap::new(),
            pending_edits: BTreeMap::new(),
            commits: BTreeMap::new(),
            branch_heads: BTreeMap::new(),
            pending_branches: BTreeMap::new(),
            planned_files: BTreeSet::new(),
        };
        // Ordinary Mkdir bindings and snapshot host bindings share one identity
        // index. Only their originating decisions differ. A snapshot may borrow
        // the already-published binding, never reallocate or overwrite it.
        crate::engine::fresh_layout::directories::validate_state(source)
            .map_err(RegistrationError::Publication)?;
        for record in source.namespace_directories.values() {
            let directory = crate::engine::fresh_layout::directories::owner_binding(source, record)
                .map_err(RegistrationError::Publication)?;
            let key = (directory.root_id, record.entry);
            if index.directory_ids.insert(directory.directory_id, key).is_some()
                || index.directory_entities.insert(
                    (directory.root_id, directory.entity), record.entry,
                ).is_some()
                || index.directories.insert(key, RegisteredDirectory {
                    repository: record.repository,
                    binding: NamespaceDirectoryBinding { entry: record.entry, directory },
                    reservation: record.created_by, complete: true,
                }).is_some()
            {
                return Err(RegistrationError::Host);
            }
        }
        let mut allocations = Allocations::default();
        for (_, (receipt, decoded)) in records {
            let author = source.operation_principals.get(&receipt.operation_id)
                .ok_or(RegistrationError::MissingPrincipal(receipt.operation_id))?;
            let principal: Principal = author.parse().map_err(RegistrationError::Principal)?;
            if decoded.record.digest(&principal).map_err(RegistrationError::Codec)?
                != receipt.request_digest
            {
                return Err(RegistrationError::Digest(receipt.operation_id));
            }
            let commitment = decoded.record.commitment().map_err(RegistrationError::Codec)?;
            let receipt = Receipt { original: receipt, principal };
            match decoded.record {
                Record::ReserveNode(plan) => {
                    index.reserve_node(receipt, plan, commitment, &mut allocations)?;
                }
                Record::CompleteNode(completion) => index.complete_node(receipt, completion)?,
                Record::ReserveWorktree(plan) => {
                    index.reserve_worktree(receipt, plan, commitment, &mut allocations)?;
                }
                Record::CompleteWorktree(completion) => {
                    index.complete_worktree(receipt, completion)?;
                }
                Record::ReserveEdit(plan) => {
                    index.reserve_edit(receipt, plan, commitment, &mut allocations)?;
                }
                Record::CompleteEdit(completion) => index.complete_edit(receipt, completion)?,
                Record::ReserveCommit(plan) => {
                    index.reserve_commit(receipt, plan, commitment, &mut allocations)?;
                }
                Record::PublishCommitNode(completion) => {
                    index.publish_commit_node(receipt, completion)?;
                }
                Record::CompleteCommit(completion) => index.complete_commit(receipt, completion)?,
            }
        }
        Ok(index)
    }

    pub(super) fn source(&self) -> &'a State {
        self.source
    }

    pub(super) fn directories(&self) -> RegisteredNamespaceDirectories<'_> {
        RegisteredNamespaceDirectories { index: self }
    }

    pub(super) fn node(&self, handle: SnapshotHandle) -> Result<PublishedNode<'_>> {
        if let Some(CompletionOwner::CommitNode(reserve)) =
            self.completions.get(&handle.publication)
        {
            return self.committed_node(*reserve, handle);
        }
        let Some(CompletionOwner::Node(reserve)) = self.completions.get(&handle.publication)
        else {
            return Err(RegistrationError::Source);
        };
        let node = self.nodes.get(reserve).ok_or(RegistrationError::Source)?;
        if handle.repository_id != node.plan.allocated.repository
            || handle.branch_id != node.plan.allocated.branch
            || handle.node_id != node.plan.allocated.node
        {
            return Err(RegistrationError::Source);
        }
        let Publication::Complete(completion) = &node.publication else {
            return Err(RegistrationError::Pending(*reserve));
        };
        Ok(PublishedNode {
            plan: &node.plan,
            manifest: &node.manifest,
            binding: completion.binding,
        })
    }

    pub(super) fn worktree(&self, handle: WorktreeHandle) -> Result<PublishedWorktree<'_>> {
        let Some(CompletionOwner::Worktree(reserve)) = self.completions.get(&handle.publication)
        else {
            return Err(RegistrationError::Source);
        };
        let tree = self.worktrees.get(reserve).ok_or(RegistrationError::Source)?;
        if handle.worktree_id != tree.plan.allocated.worktree {
            return Err(RegistrationError::Source);
        }
        let Publication::Complete(completion) = &tree.publication else {
            return Err(RegistrationError::Pending(*reserve));
        };
        if let Some(operation) = self.pending_edits.get(&handle.worktree_id) {
            return Err(RegistrationError::Pending(*operation));
        }
        let head = self.heads.get(&handle.worktree_id).ok_or(RegistrationError::Source)?;
        Ok(PublishedWorktree {
            plan: &tree.plan,
            source: self.node(tree.plan.request.source)?,
            binding: completion.binding,
            checkpoint: head.checkpoint,
            revision: head.revision,
            publication: self.worktree_publication(handle),
        })
    }

    fn reserve_completion(
        &mut self,
        reserve: OperationId,
        completion: OperationId,
        owner: CompletionOwner,
    ) -> Result<()> {
        if reserve == completion
            || self.completions.contains_key(&completion)
            || self.planned_files.contains(&completion)
            || self.source.operations.get(&completion)
                .is_some_and(|item| item.kind != owner.kind())
        {
            return Err(RegistrationError::Completion(completion));
        }
        self.completions.insert(completion, owner);
        Ok(())
    }

    fn reserve_node(
        &mut self,
        receipt: Receipt<'a>,
        plan: NodeReservation,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation {
            return Err(RegistrationError::Operation(operation));
        }
        if plan.host.namespace.branch_id != plan.request.source.branch_id {
            return Err(RegistrationError::Host);
        }
        match (plan.host.namespace.directory, &plan.request.source.root) {
            (NamespaceDirectoryId::Repository(repository), None)
                if repository == plan.host.namespace.repository_id => {}
            (NamespaceDirectoryId::Entry(_), Some(_)) => {}
            (NamespaceDirectoryId::Repository(_), None | Some(_))
            | (NamespaceDirectoryId::Entry(_), None) => return Err(RegistrationError::Host),
        }
        if !allocations.repositories.insert(plan.allocated.repository)
            || !allocations.nodes.insert(plan.allocated.node)
            || !allocations.branches.insert(plan.allocated.branch)
        {
            return Err(RegistrationError::Identity);
        }
        if !allocations.hosts.insert((plan.request.host.root_id, plan.request.host.directory_id)) {
            return Err(RegistrationError::Host);
        }
        self.register_directories(&plan)?;
        self.reserve_completion(
            operation,
            plan.allocated.completion,
            CompletionOwner::Node(operation),
        )?;
        let manifest = AdmittedSnapshotManifest::prepare(
            plan.allocated.repository, plan.allocated.node, operation, plan.entries,
        ).map_err(RegistrationError::Manifest)?;
        if manifest.reference() != plan.material {
            return Err(RegistrationError::Material);
        }
        let plan = NodePlan {
            request: plan.request,
            host: plan.host,
            allocated: plan.allocated,
            branch_name: plan.branch_name,
            material: plan.material,
        };
        self.nodes.insert(operation, NodeRecord {
            receipt, plan, manifest, commitment, publication: Publication::Reserved,
        });
        Ok(())
    }

    fn complete_node(&mut self, receipt: Receipt<'a>, completion: NodeCompletion) -> Result<()> {
        let operation = receipt.original.operation_id;
        let node = self.nodes.get_mut(&completion.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(
            &node.receipt, &receipt, node.plan.allocated.completion,
            node.commitment, completion.reservation,
        )?;
        if !matches!(node.publication, Publication::Reserved)
            || completion.binding.reference() != node.plan.material
        {
            return Err(RegistrationError::Completion(operation));
        }
        self.branch_heads.insert(
            (node.plan.allocated.repository, node.plan.allocated.branch), node.plan.allocated.node,
        );
        node.publication = Publication::Complete(completion);
        for directory in &node.plan.host.directories {
            self.directories.get_mut(&(node.plan.request.host.root_id, directory.entry))
                .expect("invariant: reserved directory entered this index before its completion")
                .complete = true;
        }
        Ok(())
    }

    fn register_directories(&mut self, plan: &NodeReservation) -> Result<()> {
        let host = &plan.host;
        let root = plan.request.host.root_id;
        let operation = plan.request.operation_id;
        if host.directories.len() > crate::working_tree::admission::MAX_DIRECTORY_DEPTH
            || host.host().directory_id != plan.request.host.directory_id
        {
            return Err(RegistrationError::Host);
        }
        match (host.namespace.directory, host.directories.last()) {
            (NamespaceDirectoryId::Repository(_), None) => {}
            (NamespaceDirectoryId::Entry(expected), Some(last)) if expected == last.entry => {}
            (NamespaceDirectoryId::Repository(_), Some(_))
            | (NamespaceDirectoryId::Entry(_), None | Some(_)) => {
                return Err(RegistrationError::Host);
            }
        }
        let mut parent = host.common_data.directory_id;
        let mut ids = BTreeSet::from([parent]);
        let mut entities = BTreeSet::from([host.common_data.entity]);
        for binding in &host.directories {
            let directory = &binding.directory;
            if directory.root_id != root || directory.parent_directory_id != parent
                || directory.lifecycle != BindingLifecycle::Current
                || directory.entity.device() != host.common_data.entity.device()
                || directory.entity.inode() == 0
                || !ids.insert(directory.directory_id) || !entities.insert(directory.entity)
            {
                return Err(RegistrationError::Host);
            }
            let key = (root, binding.entry);
            match self.directories.get(&key) {
                Some(previous) => {
                    if previous.repository != host.namespace.repository_id
                        || previous.binding != *binding || !previous.complete
                        || self.source.operations.get(&directory.operation_id)
                            .zip(self.source.operations.get(&operation))
                            .is_none_or(|(origin, snapshot)| origin.sequence > snapshot.sequence)
                    {
                        return Err(RegistrationError::Host);
                    }
                }
                None => {
                    if directory.operation_id != operation
                        || self.directory_ids.contains_key(&directory.directory_id)
                        || self.directory_entities.contains_key(&(root, directory.entity))
                    {
                        return Err(RegistrationError::Host);
                    }
                    self.directory_ids.insert(directory.directory_id, key);
                    self.directory_entities.insert((root, directory.entity), binding.entry);
                    self.directories.insert(key, RegisteredDirectory {
                        repository: host.namespace.repository_id,
                        binding: binding.clone(), reservation: operation, complete: false,
                    });
                }
            }
            parent = directory.directory_id;
        }
        Ok(())
    }

    fn reserve_worktree(
        &mut self,
        receipt: Receipt<'a>,
        plan: WorktreeReservation,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation {
            return Err(RegistrationError::Operation(operation));
        }
        let source = self.node(plan.request.source)?;
        if plan.material != source.manifest.reference() {
            return Err(RegistrationError::Material);
        }
        // This first mode mounts only beside the original registered repository.
        // A different parent or generation needs a separate future live issuer.
        if plan.host != source.plan.host
            || plan.request.parent_directory_id != plan.host.host().directory_id
            || plan.request.expected_root_generation
                != source.plan.request.host.expected_root_generation
        {
            return Err(RegistrationError::Host);
        }
        let root = source.plan.request.host.root_id;
        if !allocations.worktrees.insert(plan.allocated.worktree) {
            return Err(RegistrationError::Identity);
        }
        if !allocations.mounts.insert((
            root, plan.request.parent_directory_id, NameKey(plan.request.name.key()),
        )) {
            return Err(RegistrationError::Mount);
        }
        self.reserve_completion(
            operation,
            plan.allocated.completion,
            CompletionOwner::Worktree(operation),
        )?;
        self.worktrees.insert(operation, WorktreeRecord {
            receipt, plan, commitment, publication: Publication::Reserved,
        });
        Ok(())
    }

    fn complete_worktree(
        &mut self,
        receipt: Receipt<'a>,
        completion: WorktreeCompletion,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let tree = self.worktrees.get_mut(&completion.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(
            &tree.receipt, &receipt, tree.plan.allocated.completion,
            tree.commitment, completion.reservation,
        )?;
        if !matches!(tree.publication, Publication::Reserved)
            || completion.binding.worktree() != tree.plan.allocated.worktree
            || completion.binding.source() != tree.plan.material
            || completion.checkpoint != completion.binding.initial_checkpoint()
        {
            return Err(RegistrationError::Completion(operation));
        }
        tree.publication = Publication::Complete(completion);
        self.heads.insert(tree.plan.allocated.worktree, LedgerHead {
            checkpoint: completion.checkpoint,
            revision: storage_contract::snapshot_worktree::WorktreeRevision {
                generation: storage_contract::Generation::new(0),
                manifest: tree.plan.material.manifest,
            },
        });
        Ok(())
    }
}

fn require_completion(
    reserve: &Receipt<'_>,
    completion: &Receipt<'_>,
    expected_operation: OperationId,
    expected_commitment: MaterialCommitment,
    actual_commitment: MaterialCommitment,
) -> Result<()> {
    if completion.original.operation_id != expected_operation
        || completion.original.sequence <= reserve.original.sequence
        || completion.principal != reserve.principal
        || actual_commitment != expected_commitment
    {
        return Err(RegistrationError::Completion(completion.original.operation_id));
    }
    Ok(())
}

fn ordered_records(source: &State) -> Result<BTreeMap<Sequence, (&OperationResult, Decoded)>> {
    let mut records = BTreeMap::new();
    for (operation, receipt) in source.operations.iter() {
        let Some(decoded) = snapshot_worktree::decode(receipt).map_err(RegistrationError::Codec)?
        else {
            continue;
        };
        if operation != &receipt.operation_id {
            return Err(RegistrationError::Operation(*operation));
        }
        if receipt.sequence > source.sequence
            || records.insert(Sequence::new(receipt.sequence), (receipt, decoded)).is_some()
        {
            return Err(RegistrationError::Sequence);
        }
    }
    Ok(records)
}

fn require_events(
    source: &State,
    records: &BTreeMap<Sequence, (&OperationResult, Decoded)>,
) -> Result<()> {
    let mut pending = records.values().map(|(receipt, _)| {
        (receipt.operation_id, *receipt)
    }).collect::<BTreeMap<_, _>>();
    let selected = pending.keys().copied().collect::<BTreeSet<_>>();
    for event in source.events.iter().filter(|event| selected.contains(&event.operation_id)) {
        let receipt = pending.remove(&event.operation_id)
            .ok_or(RegistrationError::Event(event.operation_id))?;
        if event.sequence != receipt.sequence || event.kind != receipt.kind
            || event.object_id != receipt.operation_id.to_string()
        {
            return Err(RegistrationError::Event(event.operation_id));
        }
    }
    if let Some(operation) = pending.keys().next() {
        return Err(RegistrationError::Event(*operation));
    }
    Ok(())
}
