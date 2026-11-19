//! Selected SYSTEM, shared REGISTRATION OFD for the lifetime of this
//! observer, short directory cuts when sampling. No write/sync, automatic
//! continuation or media discovery, including on a complete INITIALIZING.
use super::*;
use crate::{Entity, fs::{ControlRead, HeldDir}, pool::{READY, portable_claim::ROOT_NAME}};
use std::ffi::CStr;
use super::shares::ShareUse;
#[path="members.rs"]
mod members;

const CONFIRMED: &CStr = c"CONFIRMED";

struct Source {
    system: HeldDir,
    reg: ControlRead,
    floor: ControlRead,
    marker: ControlRead,
    genesis: Genesis,
}
pub(in crate::pool) struct Empty {source:Source}
pub(in crate::pool) struct Recorded {source:Source,snapshot:std::sync::Mutex<Snapshot>}
struct Snapshot {decisions:decision::Decisions, newer:Option<(ControlRead,ControlRead)>}
pub(in crate::pool) struct Session<'r>{source:&'r Recorded,account:crate::identity::AccountId,credential:[u8;32]}
pub(in crate::pool) struct Selected<'a,'r>{session:&'a Session<'r>,original:shares::Selection}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"not authorized")}
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

    fn require_root(&self, cut: &HeldDir) -> io::Result<()> {
        cut.require_named()?;
        if cut.entity != self.system.entity || !cut.same_mount(&self.system)? {
            return Err(invalid("portable cut selected a different SYSTEM"));
        }
        only(cut, &[READY, CONFIRMED, ROOT_NAME])?;
        self.marker.require(cut)?;
        cut.require_named()
    }
    fn require(&self,cut:&HeldDir)->io::Result<()>{
        self.require_root(cut)?;self.reg.require(cut)?;self.floor.require(cut)?;
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
        Ok(Self{source,snapshot:std::sync::Mutex::new(Snapshot{decisions,newer:None})})
    }
    /// Original regular SH stays on the FIRST OFD even as ordinary decisions
    /// append. This cache is bounded to one current cut (not one per read).
    /// Only trusted, short selection code runs in f; never KDF, slow output
    /// or an external callback. All grants/subjects are selected together.
    fn at<T>(&self,f:impl FnOnce(&decision::Decisions)->io::Result<T>)->io::Result<T>{
        let cut=HeldDir::open(&self.source.system.path)?;cut.share()?;
        self.source.require_root(&cut)?;
        let mut snap=self.snapshot.lock().map_err(|_|invalid("portable decision observer unavailable"))?;
        let(reg,floor)=snap.newer.as_ref().map(|(r,f)|(r,f)).unwrap_or((&self.source.reg,&self.source.floor));
        if reg.require(&cut).is_err()||floor.require(&cut).is_err(){
            // The real lock is on source.reg, NOT on the floor, directory or
            // this temporary new OFD. Can't follow a copied/replaced register.
            let new_reg=ControlRead::open(&cut,READY,MAX_GENESIS+decision::MAX_JOURNAL)?;
            if !self.source.reg.same_registration(&new_reg){return Err(invalid("portable original registration replaced"));}
            let bytes=new_reg.bytes(&cut)?;
            let(g,tail)=Genesis::prefix(&bytes)?;
            if g.digest!=self.source.genesis.digest{return Err(invalid("portable original genesis changed"));}
            let new_floor=ControlRead::open(&cut,CONFIRMED,FLOOR_SIZE)?;
            if !new_floor.replaces_confirmed(floor)||!new_floor.replaces_confirmed(&self.source.floor){return Err(invalid("higher portable floor was not atomically replaced"));}
            let old=snap.decisions.head;
            let current=decision::Decisions::extending(&g,tail,&new_floor.bytes(&cut)?,Some(old))?;
            if current.head.sequence()<=old.sequence(){return Err(invalid("changed control file without a true extending decision"));}
            self.source.require_root(&cut)?;new_reg.require(&cut)?;new_floor.require(&cut)?;
            snap.newer=Some((new_reg,new_floor));snap.decisions=current;
        }
        f(&snap.decisions)
    }
    pub(in crate::pool) fn first_result(&self,op:OperationId)->io::Result<Option<(crate::identity::AccountId,[u8;32])>>{
        self.at(|s|Ok(s.accounts.first().filter(|f|f.operation==op).map(|f|(f.account,f.decision))))
    }
    pub(in crate::pool) fn authenticate(&self,login:&str,password:&str)->io::Result<Session<'_>>{
        self.authenticate_at(login,password,||Ok(()))
    }
    // After positive KDF verification but before a session can be admitted.
    pub(super) fn authenticate_at(&self,login:&str,password:&str,after:impl FnOnce()->io::Result<()>)->io::Result<Session<'_>>{
        // An unknown valid-length login still takes the bounded verifier path.
        // An exact login match on the same credential is required after it.
        let candidate=self.at(|s|s.accounts.login(login).or_else(||s.accounts.first()).cloned().ok_or_else(denied))?;
        if !candidate.password_matches(login,password)?{return Err(denied());}
        after()?;
        self.at(|s|{
            let now=s.accounts.login(login).filter(|a|a.account==candidate.account&&a.decision==candidate.decision).ok_or_else(denied)?;
            Ok(Session{source:self,account:now.account,credential:now.decision})
        })
    }
}
impl<'r> Session<'r>{
    fn at_state<T>(&self,share:shares::ShareId,action:ShareUse,f:impl FnOnce(&decision::Decisions,shares::Selection)->io::Result<T>)->io::Result<T>{
        self.source.at(|state|{
            let a=state.accounts.get(self.account).filter(|a|a.decision==self.credential).ok_or_else(denied)?;
            f(state,state.shares.selected(self.source.source.genesis.pool,a,share,action)?)
        })
    }
    pub(in crate::pool) fn select(&self,share:shares::ShareId,action:shares::ShareUse)->io::Result<Selected<'_,'r>>{
        self.at_state(share,action,|_,original|Ok(Selected{session:self,original}))
    }
}
impl Selected<'_,'_>{
    /// Non-secret, scoped management observation only. It is not a pathname
    /// or a grant usable by another component to open a FILE later.
    pub(in crate::pool) fn root(&self)->io::Result<VolumeId>{
        self.checked(|_,v|Ok(v))
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
