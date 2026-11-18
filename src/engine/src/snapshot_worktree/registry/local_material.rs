//! Only the independent receipt index may identify a held/reserved original.
//! These are borrowed projections of the existing native records, not a second
//! registration or a handle minted from a disk scan.
use super::*;
mod group;
mod ordinary;
pub(crate) use ordinary::OrdinaryOriginal;
use group::Group;
use crate::working_tree::snapshot::AdmittedSnapshotManifest;
use storage_contract::{BranchName, snapshot_worktree::InitializeSnapshot};

pub(crate) struct InitialDirectoryMaterial<'a> {
    pub(crate) original: &'a crate::OperationResult,
    pub(crate) request: &'a InitializeSnapshot,
    pub(crate) host: &'a HostEvidence,
    pub(crate) allocated: NodeAllocation,
    pub(crate) branch_name: &'a BranchName,
    pub(crate) manifest: &'a AdmittedSnapshotManifest,
    pub(crate) commitment: MaterialCommitment,
}
impl<'s> ReceiptIndex<'s> {
    /// Before the first native node is bound nothing can derive a worktree
    /// from it. A head, selected completion or an already-created instance
    /// contradicts this particular proof rather than widening its scope.
    pub(crate) fn initial_pending_material(
        &self, repository: SnapshotRepositoryId,
    ) -> Result<InitialDirectoryMaterial<'_>> {
        let mut groups = self.scope_groups(|n| n.plan.allocated.repository == repository)?;
        let g = groups.remove(&repository).ok_or(RegistrationError::Source)?;
        self.initial_pending_group(&g)
    }
    fn initial_pending_group<'r>(&'r self, g: &Group<'r,'s>) -> Result<InitialDirectoryMaterial<'r>> {
        let node = g.initial;
        if !matches!(node.publication, Publication::Reserved) || g.locked_branch || !g.heads.is_empty()
            || !g.trees.is_empty() || !g.edits.is_empty() || !g.working.is_empty()
            || self.source.operations.contains_key(&node.plan.allocated.completion)
        { return Err(RegistrationError::Source); }
        Ok(InitialDirectoryMaterial { original: node.receipt.original, request: &node.plan.request,
            host: &node.plan.host, allocated: node.plan.allocated, branch_name: &node.plan.branch_name,
            manifest: &node.manifest, commitment: node.commitment })
    }
}

pub(crate) struct LocalNode<'a> {
    pub(crate) handle: SnapshotHandle,
    pub(crate) parent: Option<SnapshotHandle>,
    pub(crate) reservation: OperationId,
    pub(crate) publication: &'a OperationResult,
    pub(crate) binding: SnapshotNodeBinding,
    pub(crate) manifest: &'a AdmittedSnapshotManifest,
}
pub(crate) struct NodeDirectoryMaterial<'a> {
    pub(crate) initialization: InitialDirectoryMaterial<'a>,
    pub(crate) nodes: BTreeMap<SnapshotNodeId, LocalNode<'a>>,
    pub(crate) head: SnapshotHandle,
    pub(crate) instances: BTreeMap<WorktreeId, LocalInstance<'a>>,
}
impl<'s> ReceiptIndex<'s> {
    /// No incomplete instance is skipped. All supported open allocations and
    /// settled genesis instances are retained. Pending edits and advanced
    /// ledgers require their actual native phase and refuse this subproducer.
    pub(crate) fn original_node_directory(&self, repository: SnapshotRepositoryId)
        -> Result<NodeDirectoryMaterial<'_>> {
        self.node_directory(repository, None)
    }
    /// A planned completion is never installed into the original cursor. Its
    /// one native result is available only inside this producer callback.
    pub(crate) fn with_planned_node_directory<T>(
        &self, next: &State, result: &OperationResult, principal: &Principal,
        completion: &NodeCompletion, consume: impl FnOnce(NodeDirectoryMaterial<'_>) -> T,
    ) -> Result<T> {
        let index = self.planned_index(next, result, principal, &Record::CompleteNode(*completion))?;
        Ok(consume(index.node_directory(completion.binding.reference().repository, Some(result))?))
    }
    pub(crate) fn with_planned_instance_directory<T>(
        &self, next: &State, result: &OperationResult, principal: &Principal, record: &Record,
        consume: impl FnOnce(NodeDirectoryMaterial<'_>) -> T,
    ) -> Result<T> {
        let index = self.planned_index(next, result, principal, record)?;
        let repo = match record {
            Record::ReserveWorktree(p) => p.request.source.repository_id,
            Record::ReserveAllocatedWorktree(p) => p.resolved.request.source.repository_id,
            Record::CompleteWorktree(p) => index.worktrees.get(&p.reserve)
                .ok_or(RegistrationError::Source)?.plan.request.source.repository_id,
            Record::ReserveEdit(p) => {
                let Some(CompletionOwner::Worktree(at)) = index.completions.get(&p.request.target.publication)
                    else { return Err(RegistrationError::Source); };
                index.worktrees.get(at).ok_or(RegistrationError::Source)?.plan.request.source.repository_id
            }
            Record::CompleteEdit(c) => {
                let e = index.edits.get(&c.reserve).ok_or(RegistrationError::Source)?;
                index.worktree(e.plan.request.target)?.reservation().request.source.repository_id
            }
            _ => return Err(RegistrationError::Source),
        };
        Ok(consume(index.node_directory(repo, Some(result))?))
    }
    fn node_directory<'r>(&'r self, repository: SnapshotRepositoryId, planned: Option<&'r OperationResult>)
        -> Result<NodeDirectoryMaterial<'r>> {
        let mut groups = self.scope_groups(|n| n.plan.allocated.repository == repository)?;
        let g = groups.remove(&repository).ok_or(RegistrationError::Source)?;
        self.node_group(repository, &g, planned)
    }
    fn node_group<'r>(&'r self, repository: SnapshotRepositoryId, g: &Group<'r,'s>,
                      planned: Option<&'r OperationResult>) -> Result<NodeDirectoryMaterial<'r>> {
        let initial = g.initial;
        let Publication::Complete(binding) = &initial.publication else { return Err(RegistrationError::Source); };
        let branch = initial.plan.allocated.branch;
        let [(b, head)] = g.heads.as_slice() else { return Err(RegistrationError::Source); };
        let head = *head;
        if *b != branch || g.locked_branch { return Err(RegistrationError::Source); }
        let published = |op| match planned.filter(|p| p.operation_id == op) {
            Some(p) => Ok(p),
            None => self.originals.receipt(self.source, op).map(|(r, _)| r).map_err(RegistrationError::OriginalReceipt),
        };
        let first = initial.plan.handle();
        let mut nodes = BTreeMap::from([(first.node_id, LocalNode {
            handle: first, parent: None, reservation: initial.receipt.original.operation_id,
            publication: published(first.publication)?, binding: binding.binding, manifest: &initial.manifest,
        })]);
        // Same-branch ordinary combined directory commits can create a real
        // successor with no sparse instance. Keep all of them, not only HEAD.
        for &record in &g.working {
            let Some((node, parent)) = record.history_node() else { continue };
            let here = node.handle();
            if here.repository_id != repository { continue; }
            if here.branch_id != branch || parent.repository_id != repository || parent.branch_id != branch
                || node.host() != &initial.plan.host || node.plan.branch_name != initial.plan.branch_name
                || node.manifest().operation_id() != here.publication || nodes.len() >= 65_536
            { return Err(RegistrationError::Source); }
            let value = LocalNode { handle: here, parent: Some(parent), reservation: here.publication,
                publication: published(here.publication)?, binding: node.binding, manifest: node.manifest };
            if nodes.insert(here.node_id, value).is_some() { return Err(RegistrationError::Identity); }
        }
        // Follow already authenticated parent *handles*, never infer them by
        // order. Every node in this one-branch scope must be on that chain.
        let mut at = head;
        for left in (0..nodes.len()).rev() {
            let row = nodes.get(&at.node_id).filter(|v| v.handle == at).ok_or(RegistrationError::Source)?;
            match row.parent {
                None if left == 0 && at == first => break,
                Some(p) if left > 0 => at = p,
                _ => return Err(RegistrationError::Source),
            }
        }
        // Walk only original edit receipts; a different operation kind cannot
        // masquerade as an ordinary edit with the same final revision.
        let mut edits = BTreeMap::<WorktreeId, BTreeMap<Sequence, LocalEdit<'_>>>::new();
        let mut pending_edits = BTreeMap::new();
        for &e in &g.edits {
            let target = e.plan.request.target;
            let Publication::Complete(c) = &e.publication else {
                if pending_edits.insert(target.worktree_id, LocalPendingEdit { plan: &e.plan, reserve: e.receipt.original,
                    commitment: e.commitment }).is_some() { return Err(RegistrationError::Source); }
                continue;
            };
            let row = LocalEdit { plan: &e.plan, reserve: e.receipt.original, result: c,
                publication: published(e.plan.completion)? };
            let group = edits.entry(target.worktree_id).or_default();
            if group.len() >= crate::directory_worktree::ledger::MAX_FRAMES as usize
                || group.insert(c.checkpoint.sequence, row).is_some() { return Err(RegistrationError::Source); }
        }
        let mut instances = BTreeMap::new();
        for &wt in &g.trees {
            let id = wt.plan.allocated.worktree;
            let source = wt.plan.request.source;
            let pending = pending_edits.remove(&id);
            if nodes.get(&source.node_id).is_none_or(|n| n.handle != source || n.manifest.reference() != wt.plan.material)
                || wt.plan.host != initial.plan.host || self.pending_edits.get(&id) != pending.as_ref().map(|e| &e.reserve.operation_id)
                || self.added_files.get(&id).is_some_and(|v| !v.is_empty()) || instances.len() >= 65_536
            { return Err(RegistrationError::Source); }
            let edits: Vec<_> = edits.remove(&id).unwrap_or_default().into_values().collect();
            let complete = match &wt.publication {
                Publication::Reserved if !self.heads.contains_key(&id) && edits.is_empty() && pending.is_none() => None,
                Publication::Complete(c) => {
                    if c.checkpoint != c.binding.initial_checkpoint() { return Err(RegistrationError::Source); }
                    let mut through = c.checkpoint;
                    let mut revision = None;
                    for e in &edits {
                        if e.plan.binding != c.binding || e.plan.before != through
                            || e.result.checkpoint.sequence.get() != through.sequence.get() + 1
                            || e.plan.request.operation_id != e.reserve.operation_id || e.result.reserve != e.reserve.operation_id
                            || revision.is_some_and(|rev| e.plan.request.expected != rev)
                        { return Err(RegistrationError::Source); }
                        through = e.result.checkpoint;
                        revision = Some(e.result.revision);
                    }
                    let h = self.heads.get(&id).ok_or(RegistrationError::Source)?;
                    if pending.as_ref().is_some_and(|e| e.plan.binding != c.binding
                        || e.plan.request.expected != h.revision || e.plan.before != through)
                        || h.checkpoint != through || revision.map_or(h.revision.generation.get() != 0, |r| r != h.revision) {
                        return Err(RegistrationError::Source);
                    }
                    Some((*c, published(wt.plan.allocated.completion)?))
                }
                _ => return Err(RegistrationError::Source),
            };
            let row = LocalInstance { plan: &wt.plan, reserve: wt.receipt.original, commitment: wt.commitment, complete, edits, pending };
            if instances.insert(id, row).is_some() { return Err(RegistrationError::Identity); }
        }
        if !edits.is_empty() || !pending_edits.is_empty() { return Err(RegistrationError::Source); }
        Ok(NodeDirectoryMaterial { initialization: InitialDirectoryMaterial {
            original: initial.receipt.original, request: &initial.plan.request, host: &initial.plan.host,
            allocated: initial.plan.allocated, branch_name: &initial.plan.branch_name, manifest: &initial.manifest,
            commitment: initial.commitment,
        }, nodes, head, instances })
    }
}

pub(crate) struct LocalInstance<'a> {
    pub(crate) plan: &'a WorktreeReservation, pub(crate) reserve: &'a OperationResult,
    pub(crate) commitment: MaterialCommitment,
    pub(crate) complete: Option<(WorktreeCompletion, &'a OperationResult)>,
    pub(crate) edits: Vec<LocalEdit<'a>>,
    pub(crate) pending: Option<LocalPendingEdit<'a>>,
}
pub(crate) struct LocalEdit<'a> {
    pub(crate) plan: &'a crate::persist::snapshot_worktree::EditReservation,
    pub(crate) reserve: &'a OperationResult,
    pub(crate) result: &'a crate::persist::snapshot_worktree::EditCompletion,
    pub(crate) publication: &'a OperationResult,
}
pub(crate) struct LocalPendingEdit<'a> {
    pub(crate) plan: &'a crate::persist::snapshot_worktree::EditReservation,
    pub(crate) reserve: &'a OperationResult, pub(crate) commitment: MaterialCommitment,
}


/// Complete list over the ORIGINAL initialization registrations on one
/// selected holder. Descendants cannot create an unrelated repository by
/// naming its ID. This is a catalogue of directory sources only.
pub(crate) enum LocalDirectory<'a> { Reserved(InitialDirectoryMaterial<'a>), Bound(NodeDirectoryMaterial<'a>) }
impl<'s> ReceiptIndex<'s> {
    pub(crate) fn original_directories(&self, registration: &storage_contract::working_tree::DiskRegistrationIdentity)
        -> Result<BTreeMap<SnapshotRepositoryId, LocalDirectory<'_>>> {
        self.project_directories(registration, None)
    }
    pub(crate) fn with_planned_directories<T>(&self, next: &State, result: &OperationResult, principal: &Principal,
        record: &Record, registration: &storage_contract::working_tree::DiskRegistrationIdentity,
        consume: impl FnOnce(BTreeMap<SnapshotRepositoryId, LocalDirectory<'_>>) -> T) -> Result<T> {
        let index = self.planned_index(next, result, principal, record)?;
        Ok(consume(index.project_directories(registration, Some(result))?))
    }
    fn project_directories<'a>(&'a self, registration: &storage_contract::working_tree::DiskRegistrationIdentity,
        planned: Option<&'a OperationResult>) -> Result<BTreeMap<SnapshotRepositoryId, LocalDirectory<'a>>> {
        let mut result = BTreeMap::new();
        let mut hosts = BTreeSet::new();
        for (repo, group) in self.scope_groups(|n| n.plan.host.registration == *registration)? {
            let node = group.initial;
            let host = node.plan.host.host();
            if result.len() >= 65_536 || host.directory_id != node.plan.request.host.directory_id
                || !hosts.insert((node.plan.request.host.root_id, host.directory_id)) || result.contains_key(&repo) {
                return Err(RegistrationError::Source);
            }
            let cut = match &node.publication {
                Publication::Reserved => LocalDirectory::Reserved(self.initial_pending_group(&group)?),
                Publication::Complete(_) => LocalDirectory::Bound(self.node_group(repo, &group, planned)?),
            };
            result.insert(repo, cut);
        }
        Ok(result)
    }
}
