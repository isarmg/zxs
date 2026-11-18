//! Pending names and settled sparse entities are not interchangeable. These
//! rows join captured creations and the complete Active edit chain.
//! Moving, hidden or otherwise unhandled phases are never inferred from files.
use super::{record::*, *};
use crate::directory_worktree::{ledger::{LedgerError, ScopedActiveLedger, ScopedEdit}, lifecycle::Mount};
use crate::engine::scope_closure::{OrdinalBuilder, ordinal::KeyedBuilder};
use crate::persist::{layout::worktree as disk_layout, snapshot_worktree as native};
use crate::snapshot_worktree::{LocalInstance, RegistrationError};
use crate::working_tree::snapshot::ScopedNodeFiles;
use platform_io::fs_safety::EntryName;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeId};
use storage_contract::working_tree::SnapshotNodeId;

#[derive(Default)]
struct Names { settled: BTreeSet<EntryName>, reserved: BTreeSet<EntryName> }
struct Finished { source: SnapshotNodeId, pin: ScopedActiveLedger, body: OrdinalPages, material: Material<WorktreeId>, operation: OperationId }
pub(super) struct Instances {
    names: BTreeMap<SnapshotNodeId, Names>, settled: BTreeMap<WorktreeId, Finished>, bytes: usize,
}
fn ledger_error(e: LedgerError) -> crate::Error {
    crate::business::file_draft::worktree::failure(RegistrationError::Ledger(e))
}
impl Instances {
    pub(super) fn prepare(rows: &mut KeyedBuilder, repository_bodies: &mut OrdinalBuilder, nodes: &BTreeMap<SnapshotNodeId, ScopedNodeFiles>,
        instances: BTreeMap<WorktreeId, LocalInstance<'_>>) -> Result<Self> {
        let mut out = Self { names: BTreeMap::new(), settled: BTreeMap::new(), bytes: 0 };
        for (id, i) in instances {
            let p = i.plan;
            let reserve = i.reserve.operation_id;
            let node = nodes.get(&p.request.source.node_id).ok_or_else(reject)?;
            check(id == p.allocated.worktree && reserve == p.request.operation_id && p.material == node.manifest().reference())?;
            let names = out.names.entry(p.request.source.node_id).or_default();
            let creation = match i.reserve.kind.as_str() { native::WORKTREE_RESERVE => native::WORKTREE_RESERVE,
                native::WORKTREE_ALLOCATE => native::WORKTREE_ALLOCATE, _ => return Err(reject()) };
            let add = match i.complete {
                None => {
                    let value = Pending { record_kind: creation, commitment: i.commitment,
                        target: Target::Worktree { allocated: p.allocated, source: p.request.source, material: p.material,
                            mount: Mount { parent: p.request.parent_directory_id, name: (&p.request.name).into() } },
                        binding: LocalBinding::NotBound {} };
                    rows.push(&Key::Pending { reserve_operation: reserve }, &Row { source: origin(i.reserve)?, value })?;
                    &mut names.reserved
                }
                Some((c, original)) => {
                    check(original.kind == native::WORKTREE_COMPLETE && original.operation_id == p.allocated.completion
                        && c.reserve == reserve && c.reservation == i.commitment && c.binding.worktree() == id)?;
                    let native: Vec<_> = i.edits.iter().map(|e| ScopedEdit { operation: e.reserve.operation_id,
                        expected: e.plan.request.expected, before: e.plan.before, after: e.result.checkpoint,
                        record: &e.result.edit }).collect();
                    let pin = ScopedActiveLedger::open(node.source(), &p.request, c.binding, c.checkpoint, &native)
                        .map_err(ledger_error)?;
                    let (checkpoint, selecting) = i.edits.last()
                        .map_or((c.checkpoint, original), |e| (e.result.checkpoint, e.publication));
                    let handle = WorktreeHandle { publication: original.operation_id, worktree_id: id };
                    let value = Instance { handle, reservation: reserve, source: p.request.source, mount: pin.mount(),
                        instance: pin.sparse(), ledger: c.binding, checkpoint };
                    rows.push(&Key::Instance { worktree_id: id }, &Row { source: origin(selecting)?, value })?;
                    if let Some(pending) = i.pending {
                        let p0 = pending.plan;
                        let src = origin(pending.reserve)?;
                        let op = src.operation_id;
                        check(pending.reserve.kind == native::EDIT_RESERVE && p0.request.operation_id == op
                            && p0.request.target == handle && p0.binding == c.binding && p0.before == checkpoint)?;
                        let planned = Pending { record_kind: native::EDIT_RESERVE, commitment: pending.commitment,
                            target: Target::Edit { worktree: handle, source: p.request.source, completion: p0.completion,
                                version: p0.version }, binding: LocalBinding::Bound {
                                    binding: BoundInstance { instance: pin.sparse(), ledger: c.binding } } };
                        rows.push(&Key::Pending { reserve_operation: op }, &Row { source: src, value: planned })?;
                        let key = |role| Key::PendingItem { reserve_operation: op, role,
                            source_ordinal: Sequence::new(0), item_ordinal: Sequence::new(0) };
                        rows.push(&key(ItemRole::Edit), &Row { source: src, value: ReservedEdit {
                            path: &p0.request.path, original: p0.original, selector: p0.original_source,
                            leaf: crate::directory_worktree::ledger::EditLeaf::for_version(p0.version, &p0.request.path),
                            bytes: p0.request.size, content_hash: p0.request.content_hash } })?;
                        rows.push(&key(ItemRole::Checkpoint), &Row { source: src, value: ReservedCheckpoint {
                            ledger: c.binding, before: p0.before, revision: p0.request.expected } })?;
                        // The future leaf/entity and next ledger checksum were
                        // NOT selected yet. An actual unknown suffix still
                        // refuses full admission; the checker never repairs it.
                    }
                    let mut own = OrdinalBuilder::new(i.edits.len());
                    for e in &i.edits {
                        let edit = &e.result.edit;
                        check(e.plan.request.target == handle && e.plan.request.path == edit.path
                            && e.plan.original.file_id == edit.material.file_id && e.plan.version == edit.material.version_id)?;
                        let item = Delta::WorktreeDelta { source: DeltaSource { worktree: handle,
                            snapshot: p.request.source, path: &edit.path, file_id: edit.material.file_id,
                            original_version: e.plan.original.version_id, version: e.plan.version,
                            selector: e.plan.original_source, reservation: e.reserve.operation_id,
                            publication: e.publication.operation_id, entity: edit.entity },
                            relative_name: edit.leaf.as_str(), bytes: edit.material.size,
                            content_hash: edit.material.content_hash };
                        own.push(&item)?;
                        repository_bodies.push(&item)?;
                    }
                    // The last independently selected native frame binds the
                    // complete chain backward to the original physical Genesis.
                    let body = own.finish()?;
                    out.bytes = out.bytes.checked_add(body.bytes()).filter(|n| *n <= 64 * 1024 * 1024)
                        .ok_or(Failure::Budget)?;
                    let material = Material { scope: Scope::Worktree, identity: id, logical_root: checkpoint.commitment,
                        representation: Representation::Native { root_hash: checkpoint.commitment },
                        body_inventory: body.root().clone() };
                    out.settled.insert(id, Finished { source: p.request.source.node_id, pin, body, material,
                        operation: original.operation_id });
                    &mut names.settled
                }
            };
            check(add.insert(disk_layout::instance(id)) && add.insert(disk_layout::ledger(id)))?;
        }
        Ok(out)
    }
    pub(super) fn require(&self, nodes: &BTreeMap<SnapshotNodeId, ScopedNodeFiles>) -> Result<()> {
        let none = Names::default();
        for (id, n) in nodes {
            let expected = self.names.get(id).unwrap_or(&none);
            n.require_instance_names(&expected.settled, &expected.reserved)?;
        }
        for f in self.settled.values() {
            f.pin.require(nodes.get(&f.source).ok_or_else(reject)?.source()).map_err(ledger_error)?;
        }
        Ok(())
    }
    pub(super) fn append(&self, host: &BoundHost, rows: &mut OrdinalBuilder) -> Result<()> {
        for (&id, f) in &self.settled {
            let h = &host.native;
            let m = f.pin.mount();
            rows.push(&InventoryRow { object: Object::Worktree { worktree_id: id, material: &f.material },
                ownership: Ownership::Original { registration: &h.registration },
                location: Location::Entry { root_id: h.root_id, parent_directory_id: m.parent, name: m.name.as_str() },
                lifecycle: Lifecycle::Current {}, entity: f.pin.entity().map_err(ledger_error)?, operation_id: f.operation })?;
        }
        Ok(())
    }
    pub(super) fn bytes(&self) -> usize { self.bytes }
    pub(super) fn stage(&self, host: &BoundHost) -> Result<()> {
        for f in self.settled.values() { f.body.install(&host.disk, &host.handle.root)?; }
        Ok(())
    }
    pub(super) fn verify(&self, host: &BoundHost) -> Result<()> {
        for f in self.settled.values() {
            f.body.verify(&host.disk, &host.handle.root, &f.material.body_inventory)?;
        }
        Ok(())
    }
}
