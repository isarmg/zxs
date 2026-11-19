//! Selected SYSTEM, shared REGISTRATION OFD for the lifetime of this
//! observer, short directory cuts when sampling. No write/sync, automatic
//! continuation or media discovery, including on a complete INITIALIZING.
use super::*;
use crate::{Entity, fs::{ControlRead, HeldDir}, pool::{READY, portable_claim::ROOT_NAME}};
use std::ffi::CStr;

const CONFIRMED: &CStr = c"CONFIRMED";

struct Source {
    system: HeldDir,
    reg: ControlRead,
    floor: ControlRead,
    marker: ControlRead,
    genesis: Genesis,
}
pub(in crate::pool) struct Empty {source:Source}
pub(in crate::pool) struct Recorded {source:Source,decisions:super::decision::Decisions}
pub(in crate::pool) struct LocalRow {
    pub(in crate::pool) id: VolumeId,
    pub(in crate::pool) role: Role,
    /// Only the current held mount observation: never an on-disk identity.
    pub(in crate::pool) entity: Entity,
}
fn only(dir: &HeldDir, expected: &[&CStr]) -> io::Result<()> {
    let actual = dir.names()?;
    if actual.len() != expected.len() || !expected.iter().all(|s| actual.iter().any(|n| n.as_slice()==s.to_bytes())) {
        return Err(invalid("unknown or missing portable root entry at this selected cut"));
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
impl Source {
    fn open<T>(selected:&Path,max_suffix:usize,admit:impl FnOnce(&Genesis,&[u8],&[u8])->io::Result<T>)->io::Result<(Self,T)> {
        let system = HeldDir::open(selected)?;
        // No bytes interpreted, no registry path followed before this SH.
        // Neither the directory's short SH nor a pathname is a substitute.
        let reg = ControlRead::shared_registration(&system,MAX_GENESIS+max_suffix)?;
        system.share()?;
        only(&system, &[READY, CONFIRMED, ROOT_NAME])?;
        let bytes=reg.bytes(&system)?;
        let (genesis,tail)=Genesis::prefix(&bytes)?;
        if tail.len()>max_suffix{return Err(invalid("unknown or oversized portable register suffix"));}
        if genesis.uid != fs::local_uid() { return Err(invalid("wrong original portable storage owner")); }
        row_at(&system, &genesis.rows[0])?;
        let s_mark = marker(&system, genesis.mark(&genesis.rows[0]))?;
        let floor = ControlRead::open(&system, CONFIRMED, FLOOR_SIZE)?;
        let state=admit(&genesis,tail,&floor.bytes(&system)?)?;
        let ready = Self { system, reg, floor, marker: s_mark, genesis };
        ready.require(&ready.system)?;
        // All lookups above used the real protected S. No R/F needs opening
        // to verify a confirmed empty/account cut in this S.
        ready.system.file.unlock()?;
        Ok((ready,state))
    }

    fn require(&self, cut: &HeldDir) -> io::Result<()> {
        cut.require_named()?;
        if cut.entity != self.system.entity || !cut.same_mount(&self.system)? {
            return Err(invalid("portable cut selected a different SYSTEM"));
        }
        only(cut, &[READY, CONFIRMED, ROOT_NAME])?;
        self.reg.require(cut)?; self.floor.require(cut)?; self.marker.require(cut)?;
        cut.require_named()
    }

}
impl Recorded {
    /// Trusted configuration selects the expected pool. No user-supplied
    /// pathname, R/F probing or old-family fallback. This status cannot
    /// create a task, publish FILEs, grant a share or tell a caller a password.
    pub(in crate::pool) fn open(selected:&Path,pool:PoolId)->io::Result<Self>{
        let(source,decisions)=Source::open(selected,super::decision::MAX_JOURNAL,|g,tail,floor|{
            if g.pool!=pool{return Err(invalid("different registered portable pool"));}
            super::decision::Decisions::read(g,tail,floor)
        })?;
        Ok(Self{source,decisions})
    }
    pub(in crate::pool) fn first_result(&self,op:OperationId)->io::Result<Option<(crate::identity::AccountId,[u8;32])>>{
        let cut=HeldDir::open(&self.source.system.path)?;cut.share()?;
        self.source.require(&cut)?;
        Ok(self.decisions.first.as_ref().filter(|f|f.operation==op).map(|f|(f.account,f.decision)))
    }
}
impl Empty {
    pub(in crate::pool) fn open(selected:&Path)->io::Result<Self>{
        let(source,())=Source::open(selected,0,|g,_,floor|g.require_zero(floor))?;
        Ok(Self{source})
    }
    /// A current observation of one claimed role, not an I/O handle, proof
    /// of an empty disk, recycler independence, ONLINE health or authority to
    /// use a registration path. A missing unrelated volume does not hide S.
    pub(in crate::pool) fn observe(&self, requested: VolumeId) -> io::Result<LocalRow> {
        let cut = HeldDir::open(&self.source.system.path)?;
        cut.share()?;
        self.source.require(&cut)?;
        let row = self.source.genesis.rows.iter().find(|r| r.volume == requested)
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
        let observed = marker(&root, self.source.genesis.mark(row))?;
        self.source.require(&cut)?;
        observed.require(&root)?;
        only(&root, &[ROOT_NAME])?;
        root.require_named()?;
        Ok(LocalRow { id:row.volume, role:row.role, entity:root.entity })
    }
}
