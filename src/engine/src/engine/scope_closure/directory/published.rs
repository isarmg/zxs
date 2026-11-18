//! Complete original branch, node history and supported sparse creations.
//! Later ledger phases have different owners and are NOT skipped or
//! certified as empty by this producer.
use super::{record::*, *};
use crate::engine::scope_closure::{OrdinalBuilder, ordinal::KeyedBuilder, snapshot_bodies::{BodyLeaf, NodeBodies, OriginalLeaf}};
use crate::persist::{layout::snapshot as layout, snapshot_worktree::NodeCompletion};
use crate::snapshot_worktree::NodeDirectoryMaterial;
use crate::working_tree::snapshot::ScopedNodeFiles;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{Principal, working_tree::{LocalEntityIdentity, SnapshotNodeId}};

pub(in crate::engine) struct PublishedRepository {
    host: BoundHost, frozen: FrozenDirectory, nodes: BTreeMap<SnapshotNodeId, ScopedNodeFiles>,
    entity: LocalEntityIdentity, installed_by: OperationId, instances: super::instances::Instances,
}
impl PublishedRepository {
    pub(in crate::engine) fn original(ctx: &PlanningContext<'_>, repo: SnapshotRepositoryId) -> Result<Self> {
        let ix = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let facts = ix.original_node_directory(repo).map_err(crate::business::file_draft::worktree::failure)?;
        Self::from_facts(ctx, facts)
    }
    pub(in crate::engine) fn planned_initial_completion(ctx: &PlanningContext<'_>, next: &crate::State,
        receipt: &crate::OperationResult, principal: &Principal, completion: &NodeCompletion) -> Result<Self> {
        check(receipt.sequence == ctx.next_slot)?;
        ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?
            .with_planned_node_directory(next, receipt, principal, completion, |facts| Self::from_facts(ctx, facts))
            .map_err(crate::business::file_draft::worktree::failure)?
    }
    pub(in crate::engine) fn planned_instance(ctx: &PlanningContext<'_>, next: &crate::State,
        receipt: &crate::OperationResult, principal: &Principal, record: &crate::persist::snapshot_worktree::Record) -> Result<Self> {
        check(receipt.sequence == ctx.next_slot)?;
        ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?
            .with_planned_instance_directory(next, receipt, principal, record, |facts| Self::from_facts(ctx, facts))
            .map_err(crate::business::file_draft::worktree::failure)?
    }
    pub(super) fn from_facts(ctx: &PlanningContext<'_>, facts: NodeDirectoryMaterial<'_>) -> Result<Self> {
        let initial = &facts.initialization;
        let repo = initial.allocated.repository;
        let reserve = initial.original.operation_id;
        let first = facts.nodes.get(&initial.allocated.node).ok_or_else(reject)?;
        check(first.handle.publication == initial.allocated.completion && first.parent.is_none()
            && first.reservation == reserve && first.manifest.bytes() == initial.manifest.bytes())?;
        let entity = first.binding.repository_entity();
        let installed_by = first.handle.publication;
        let host = BoundHost::initial(ctx, initial.request.host, initial.host)?;
        let mut rows = KeyedBuilder::new(MAX_ROWS);
        host.append_steps(origin(initial.original)?, &mut rows)?;
        let mut body = OrdinalBuilder::new(MAX_ROWS);
        let here = facts.nodes.get(&facts.head.node_id).ok_or_else(reject)?;
        check(here.handle == facts.head)?;
        rows.push(&Key::Branch { branch_id: facts.head.branch_id }, &Row { source: origin(here.publication)?,
            value: Branch { branch_id: facts.head.branch_id, name: initial.branch_name,
                creation_operation: reserve, state: BranchState::Head { handle: facts.head } } })?;
        let mut nodes = BTreeMap::new();
        let mut distinct_bodies = BTreeSet::new();
        // Stable native node ID, then original manifest order. Equal bytes on
        // unrelated paths/nodes never collapse the logical occurrences.
        for (id, n) in facts.nodes {
            check(n.handle.node_id == id && n.handle.repository_id == repo && n.handle.publication == n.publication.operation_id
                && n.binding.repository_entity() == entity && n.binding.reference() == n.manifest.reference()
                && n.manifest.operation_id() == n.reservation)?;
            rows.push(&Key::Node { node_id: id }, &Row { source: origin(n.publication)?,
                value: Node { handle: n.handle, reservation: n.reservation, binding: n.binding,
                    parent: n.parent.map_or(Parent::Initial {}, |handle| Parent::Node { handle }) } })?;
            let pin = ScopedNodeFiles::open(host.held(), n.binding, n.manifest)?;
            let manifest = pin.manifest();
            let mut leaves = BTreeMap::new();
            for (id, file) in manifest.files() {
                let leaf_entity = pin.body_entity(*id)?;
                check(distinct_bodies.insert(leaf_entity))?;
                leaves.insert(*id, BodyLeaf { logical: *file, operation: n.reservation,
                    name: layout::file(&manifest.payloads()[id]), physical: OriginalLeaf::Present { entity: leaf_entity } });
            }
            NodeBodies::append_rows(manifest, &leaves, &mut body)?;
            nodes.insert(id, pin);
        }
        let instances = super::instances::Instances::prepare(&mut rows, &mut body, &nodes, facts.instances)?;
        let frozen = FrozenDirectory::new(repo, &host, reserve, rows.finish()?, body.finish()?)?;
        frozen.bytes()?.checked_add(instances.bytes()).filter(|n| *n <= 64 * 1024 * 1024)
            .ok_or(Failure::Budget)?;
        let value = Self { host, frozen, nodes, entity, installed_by, instances };
        value.require(ctx.config)?;
        Ok(value)
    }
    /// No single CURRENT row may survive a replaced inode/extra node or
    /// unaccounted sparse instance. A check never adopts the extra names.
    pub(in crate::engine) fn require(&self, config: &ValidatedConfig) -> Result<()> {
        self.host.check(config)?;
        ScopedNodeFiles::require_repository_names(self.host.held(), self.entity, self.nodes.keys().copied())?;
        for node in self.nodes.values() {
            node.require(self.host.held())?;
        }
        self.instances.require(&self.nodes)?;
        self.host.check(config)
    }
    pub(in crate::engine) fn bytes(&self) -> Result<usize> {
        self.frozen.bytes()?.checked_add(self.instances.bytes()).ok_or_else(|| Failure::Budget.into())
    }
    pub(in crate::engine) fn root(&self) -> MaterialCommitment { self.frozen.descriptor.hash() }
    pub(in crate::engine) fn counts(&self) -> (usize, usize) { (self.nodes.len(), self.frozen.body.entries()) }
    pub(in crate::engine) fn entity(&self) -> LocalEntityIdentity { self.entity }
    pub(in crate::engine) fn append_to(&self, config: &ValidatedConfig, rows: &mut OrdinalBuilder) -> Result<()> {
        self.require(config)?;
        let h = &self.host.native;
        let name = crate::persist::layout::SNAPSHOT_REPOSITORY.as_os_str().to_str().ok_or_else(reject)?;
        rows.push(&InventoryRow { object: Object::DirectoryRepository { repository_id: self.frozen.header.repository_id,
            directory_id: h.directory_id, material: &self.frozen.material },
            ownership: Ownership::Original { registration: &h.registration },
            location: Location::Entry { root_id: h.root_id, parent_directory_id: h.directory_id, name },
            lifecycle: Lifecycle::Current {}, entity: self.entity, operation_id: self.installed_by })?;
        self.instances.append(&self.host, rows)
    }
    pub(in crate::engine) fn stage_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.require(config)?;
        self.instances.stage(&self.host)?;
        self.frozen.stage(&self.host, config)?;
        self.require(config)
    }
    pub(in crate::engine) fn verify_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.require(config)?;
        self.instances.verify(&self.host)?;
        self.frozen.verify(&self.host, config)?;
        self.require(config)
    }
}
