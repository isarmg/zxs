//! Selected SYSTEM, shared REGISTRATION OFD for the lifetime of this
//! observer, short directory cuts when sampling. No write/sync, automatic
//! continuation or media discovery, including on a complete INITIALIZING.
use super::*;
use crate::{Entity, fs::{ControlRead, HeldDir}, pool::{READY, portable_claim::ROOT_NAME}};
use std::ffi::CStr;

const CONFIRMED: &CStr = c"CONFIRMED";

pub(in crate::pool) struct Empty {
    system: HeldDir,
    reg: ControlRead,
    floor: ControlRead,
    marker: ControlRead,
    genesis: Genesis,
}
pub(in crate::pool) struct LocalRow {
    pub(in crate::pool) id: VolumeId,
    pub(in crate::pool) role: Role,
    /// Only the current held mount observation: never an on-disk identity.
    pub(in crate::pool) entity: Entity,
}
fn only(dir: &HeldDir, expected: &[&CStr]) -> io::Result<()> {
    let actual = dir.names()?;
    if actual.len() != expected.len() || !expected.iter().all(|s| actual.iter().any(|n| n.as_slice()==s.to_bytes())) {
        return Err(invalid("unknown or missing portable root entry at this empty cut"));
    }
    Ok(())
}
fn row_at(root: &HeldDir, row: &Row) -> io::Result<()> {
    if root.path.as_os_str() != row.path.as_os_str() || ComponentLimit::held(root)? != row.limit {
        return Err(invalid("portable root spelling or observed component limit disagrees with its original row"));
    }
    Ok(())
}
fn marker(dir: &HeldDir, expected: Declaration) -> io::Result<ControlRead> {
    let held = ControlRead::open(dir, ROOT_NAME, MARK_SIZE)?;
    if Declaration::decode(&held.bytes(dir)?)? != expected {
        return Err(invalid("portable root marker disagrees with its original SYSTEM claim"));
    }
    Ok(held)
}
impl Empty {
    pub(in crate::pool) fn open(selected: &Path) -> io::Result<Self> {
        let system = HeldDir::open(selected)?;
        // No bytes interpreted, no registry path followed before this SH.
        // Neither the directory's short SH nor a pathname is a substitute.
        let reg = ControlRead::shared_registration(&system, MAX_GENESIS)?;
        system.share()?;
        only(&system, &[READY, CONFIRMED, ROOT_NAME])?;
        let genesis = Genesis::only(&reg.bytes(&system)?)?;
        if genesis.uid != fs::local_uid() { return Err(invalid("wrong original portable storage owner")); }
        row_at(&system, &genesis.rows[0])?;
        let s_mark = marker(&system, genesis.mark(&genesis.rows[0]))?;
        let floor = ControlRead::open(&system, CONFIRMED, FLOOR_SIZE)?;
        genesis.require_zero(&floor.bytes(&system)?)?;
        let ready = Self { system, reg, floor, marker: s_mark, genesis };
        ready.require(&ready.system)?;
        // All lookups above used the real protected S. No R/F needs opening
        // in order to report that this S has exactly the empty confirmed cut.
        ready.system.file.unlock()?;
        Ok(ready)
    }

    fn require(&self, cut: &HeldDir) -> io::Result<()> {
        cut.require_named()?;
        if cut.entity != self.system.entity || !cut.same_mount(&self.system)? {
            return Err(invalid("portable empty cut selected a different SYSTEM"));
        }
        only(cut, &[READY, CONFIRMED, ROOT_NAME])?;
        self.reg.require(cut)?; self.floor.require(cut)?; self.marker.require(cut)?;
        cut.require_named()
    }

    /// A current observation of one claimed role, not an I/O handle, proof
    /// of an empty disk, recycler independence, ONLINE health or authority to
    /// use a registration path. A missing unrelated volume does not hide S.
    pub(in crate::pool) fn observe(&self, requested: VolumeId) -> io::Result<LocalRow> {
        let cut = HeldDir::open(&self.system.path)?;
        cut.share()?;
        self.require(&cut)?;
        let row = self.genesis.rows.iter().find(|r| r.volume == requested)
            .ok_or_else(|| invalid("volume is not in the original portable claim"))?;
        if row.role == Role::System {
            return Ok(LocalRow { id:row.volume, role:row.role, entity:cut.entity });
        }
        let root = HeldDir::open(&row.path)?;
        root.share()?;
        row_at(&root, row)?;
        if root.entity == cut.entity { return Err(invalid("data role aliases this actual SYSTEM root")); }
        // These are two LIVE descriptors, not permanent disk IDs. Either
        // positive equality contradicts a dedicated recycler. Inequality
        // is not a full proof of separation from every registered root.
        if row.role == Role::Recycle && (root.same_mount(&cut)? || root.entity.device == cut.entity.device) {
            return Err(invalid("selected recycler shares the actual SYSTEM filesystem"));
        }
        only(&root, &[ROOT_NAME])?;
        let observed = marker(&root, self.genesis.mark(row))?;
        self.require(&cut)?;
        observed.require(&root)?;
        only(&root, &[ROOT_NAME])?;
        root.require_named()?;
        Ok(LocalRow { id:row.volume, role:row.role, entity:root.entity })
    }
}
