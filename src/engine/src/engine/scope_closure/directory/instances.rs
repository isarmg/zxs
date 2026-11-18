//! Pending names and settled sparse entities are not interchangeable. These
//! rows cover only independently captured creations and exact original empty
//! Genesis checkpoints; edited/moving/hiding instances have other obligations.
use super::{record::*, *};
use crate::directory_worktree::{ledger::{LedgerError, ScopedGenesis}, lifecycle::Mount};
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
struct Finished { source: SnapshotNodeId, pin: ScopedGenesis, material: Material<WorktreeId>, operation: OperationId }
pub(super) struct Instances {
    names: BTreeMap<SnapshotNodeId, Names>, settled: BTreeMap<WorktreeId, Finished>, empty: Option<OrdinalPages>,
}
fn ledger_error(e: LedgerError) -> crate::Error {
    crate::business::file_draft::worktree::failure(RegistrationError::Ledger(e))
}
impl Instances {
    pub(super) fn prepare(rows: &mut KeyedBuilder, nodes: &BTreeMap<SnapshotNodeId, ScopedNodeFiles>,
        instances: BTreeMap<WorktreeId, LocalInstance<'_>>) -> Result<Self> {
        let mut out = Self { names: BTreeMap::new(), settled: BTreeMap::new(), empty: None };
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
                    let pin = ScopedGenesis::open(node.source(), &p.request, c.binding, c.checkpoint).map_err(ledger_error)?;
                    let value = Instance { handle: WorktreeHandle { publication: original.operation_id, worktree_id: id },
                        reservation: reserve, source: p.request.source, mount: pin.mount(), instance: pin.sparse(),
                        ledger: c.binding, checkpoint: c.checkpoint };
                    rows.push(&Key::Instance { worktree_id: id }, &Row { source: origin(original)?, value })?;
                    // A Genesis frame commits its native source/branch, mount,
                    // bound sparse instance and zero-delta state. The exact raw
                    // frame is already stored at its native ledger address.
                    if out.empty.is_none() { out.empty = Some(OrdinalBuilder::new(0).finish()?); }
                    let material = Material { scope: Scope::Worktree, identity: id, logical_root: c.checkpoint.commitment,
                        representation: Representation::Native { root_hash: c.checkpoint.commitment },
                        body_inventory: out.empty.as_ref().ok_or_else(reject)?.root().clone() };
                    out.settled.insert(id, Finished { source: p.request.source.node_id, pin, material, operation: original.operation_id });
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
    pub(super) fn bytes(&self) -> usize { self.empty.as_ref().map_or(0, OrdinalPages::bytes) }
    pub(super) fn stage(&self, host: &BoundHost) -> Result<()> {
        if let Some(empty) = &self.empty { empty.install(&host.disk, &host.handle.root)?; }
        Ok(())
    }
    pub(super) fn verify(&self, host: &BoundHost) -> Result<()> {
        if let Some(empty) = &self.empty { empty.verify(&host.disk, &host.handle.root, empty.root())?; }
        Ok(())
    }
}
