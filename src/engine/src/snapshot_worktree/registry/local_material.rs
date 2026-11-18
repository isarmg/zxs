//! Only the independent receipt index may identify a held/reserved original.
//! These are borrowed projections of the existing native records, not a second
//! registration or a handle minted from a disk scan.
use super::*;
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
impl ReceiptIndex<'_> {
    /// Before the first native node is bound nothing can derive a worktree
    /// from it. A head, selected completion or an already-created instance
    /// contradicts this particular proof rather than widening its scope.
    pub(crate) fn initial_pending_material(
        &self, repository: SnapshotRepositoryId,
    ) -> Result<InitialDirectoryMaterial<'_>> {
        let mut nodes = self.nodes.values().filter(|n| n.plan.allocated.repository == repository);
        let node = nodes.next().ok_or(RegistrationError::Source)?;
        if nodes.next().is_some() || !matches!(node.publication, Publication::Reserved)
            || self.branch_heads.keys().any(|(r, _)| *r == repository)
            || self.worktrees.values().any(|w| w.plan.request.source.repository_id == repository)
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
impl ReceiptIndex<'_> {
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
            _ => return Err(RegistrationError::Source),
        };
        Ok(consume(index.node_directory(repo, Some(result))?))
    }
    fn node_directory<'r>(&'r self, repository: SnapshotRepositoryId, planned: Option<&'r OperationResult>)
        -> Result<NodeDirectoryMaterial<'r>> {
        let mut originals = self.nodes.values().filter(|n| n.plan.allocated.repository == repository);
        let initial = originals.next().ok_or(RegistrationError::Source)?;
        let Publication::Complete(binding) = &initial.publication else { return Err(RegistrationError::Source); };
        let branch = initial.plan.allocated.branch;
        let mut heads = self.branch_heads.iter().filter(|((r, _), _)| *r == repository);
        let Some((&(_, b), &head)) = heads.next() else { return Err(RegistrationError::Source); };
        if originals.next().is_some() || heads.next().is_some() || b != branch
            || self.pending_branches.keys().any(|(r, _)| *r == repository)
        { return Err(RegistrationError::Source); }
        let prefix = self.originals.reborrow(self.source).map_err(RegistrationError::OriginalReceipt)?;
        let published = |op| planned.filter(|p| p.operation_id == op)
            .or_else(|| prefix.receipt(op).map(|(r, _)| r)).ok_or(RegistrationError::Operation(op));
        let first = initial.plan.handle();
        let mut nodes = BTreeMap::from([(first.node_id, LocalNode {
            handle: first, parent: None, reservation: initial.receipt.original.operation_id,
            publication: published(first.publication)?, binding: binding.binding, manifest: &initial.manifest,
        })]);
        // Same-branch ordinary combined directory commits can create a real
        // successor with no sparse instance. Keep all of them, not only HEAD.
        for record in self.working.values() {
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
        let mut instances = BTreeMap::new();
        for wt in self.worktrees.values().filter(|wt| wt.plan.request.source.repository_id == repository) {
            let id = wt.plan.allocated.worktree;
            let source = wt.plan.request.source;
            if nodes.get(&source.node_id).is_none_or(|n| n.handle != source || n.manifest.reference() != wt.plan.material)
                || wt.plan.host != initial.plan.host || self.pending_edits.contains_key(&id)
                || self.added_files.get(&id).is_some_and(|v| !v.is_empty()) || instances.len() >= 65_536
            { return Err(RegistrationError::Source); }
            let complete = match &wt.publication {
                Publication::Reserved if !self.heads.contains_key(&id) => None,
                Publication::Complete(c) if self.heads.get(&id).is_some_and(|h|
                    h.checkpoint == c.checkpoint && c.checkpoint == c.binding.initial_checkpoint()
                    && h.revision.generation.get() == 0) =>
                        Some((*c, published(wt.plan.allocated.completion)?)),
                _ => return Err(RegistrationError::Source),
            };
            let row = LocalInstance { plan: &wt.plan, reserve: wt.receipt.original, commitment: wt.commitment, complete };
            if instances.insert(id, row).is_some() { return Err(RegistrationError::Identity); }
        }
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
}
