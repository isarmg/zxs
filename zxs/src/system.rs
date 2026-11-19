//! SQLite pool catalog. These helpers describe/validate inputs and immutable
//! results. Nothing here opens a DB, accepts an authority token, or issues a
//! volume claim. SQL callers use the one DDL; there is no application log.
use crate::{OperationId,PoolId,Role,VolumeId,fs::{self,invalid,ComponentLimit,NameBudget},hash::Sha256};
use std::{collections::BTreeSet,ffi::CStr,io,os::unix::ffi::OsStrExt,path::{Path,PathBuf}};

mod lease;
pub(crate) mod reader;

pub(crate) const SCHEMA:&str=include_str!("system/schema.sql");
pub(crate) const CATALOG:&str=include_str!("system/initial-catalog.sql");
pub(crate) const ORIGINAL_RESULT:&str=include_str!("system/original-init.sql");
pub(crate) const COMPLETE_INIT:&str=include_str!("system/complete-initial.sql");
pub(crate) const DB:&CStr=c"SYSTEM.sqlite3";
const JOURNAL:&CStr=c"SYSTEM.sqlite3-journal";
pub(crate) const ROOT_MARK:&CStr=c".zxs-new-ffffffffffffffffffffffffffffffff-0";
const MARK_DOMAIN:&[u8;20]=b"ZXS-SQLITE-1/VOLUME\0";
const MAX_ROOTS:usize=64;

fn role_tag(role:Role)->u8{match role {Role::System=>0,Role::Recycle=>1,Role::File=>2}}
fn role_at(i:usize)->Role{match i {0=>Role::System,1=>Role::Recycle,_=>Role::File}}
fn storage_uid(uid:u32)->io::Result<()>{
    if uid==0||uid==u32::MAX{return Err(invalid("expected a real non-root storage service uid"));}Ok(())
}
/// No authorization, I/O, normalization or filesystem discovery. Only the
/// caller's exact canonical roots enter the digest; never newly issued results.
pub(crate) fn init_request(op:OperationId,actor:[u8;16],approval:[u8;16],uid:u32,roots:&[&Path])->io::Result<[u8;32]>{
    if op.0==[0;16] || actor==[0;16] || approval==[0;16] || !(3..=MAX_ROOTS).contains(&roots.len()){
        return Err(invalid("missing initial identities or bounded selection"));
    }
    storage_uid(uid)?;
    let mut h=Sha256::new(); h.update(b"ZXS-SQLITE-1/INIT\0")?; h.update(&op.0)?; h.update(&actor)?;
    h.update(&approval)?;h.update(&uid.to_be_bytes())?;h.update(&(roots.len() as u16).to_be_bytes())?;
    for (i,path) in roots.iter().copied().enumerate(){
        fs::check_path(path)?;
        let raw=path.as_os_str().as_bytes();
        if (i==0&&path.to_str().is_none()) || path.iter().any(|p|p.as_bytes().len()>255) ||
            (i>2&&roots[i-1].as_os_str().as_bytes()>=raw) ||
            roots[..i].iter().any(|p|path.starts_with(p)||p.starts_with(path)){
            return Err(invalid("invalid SYSTEM spelling, unsorted, nested or duplicate original root"));
        }
        h.update(&[role_tag(role_at(i))])?;h.update(&(raw.len() as u16).to_be_bytes())?;h.update(raw)?;
    }
    Ok(h.finish())
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum InitState {PendingIo,Committed}
#[derive(Clone,Debug)]
pub(crate) struct ClaimRow {
    pub(crate) ordinal:u8,pub(crate) role:Role,pub(crate) volume:VolumeId,
    pub(crate) path:PathBuf,pub(crate) limit:ComponentLimit,pub(crate) token:[u8;32],
}
#[derive(Clone,Copy)]
pub(crate) struct BirthRow {
    pub(crate) pool:PoolId,pub(crate) op:OperationId,pub(crate) actor:[u8;16],
    pub(crate) request:[u8;32],pub(crate) state:InitState,pub(crate) approval:[u8;16],
    pub(crate) uid:u32,pub(crate) count:u8,
}
/// A checked relational *declaration*. Does not grant a write or prove a disk
/// is present. The VFS observation and authorization are separate boundaries.
pub(crate) struct Catalog {birth:BirthRow,rows:Vec<ClaimRow>}
impl Catalog {
    pub(crate) fn checked(expect:PoolId,birth:BirthRow,rows:Vec<ClaimRow>)->io::Result<Self>{
        if expect.0==[0;16] || expect!=birth.pool || rows.len()!=usize::from(birth.count) ||
            !(3..=MAX_ROOTS).contains(&rows.len()){return Err(invalid("wrong or incomplete initial pool catalog"));}
        let mut volumes=BTreeSet::new();let mut tokens=BTreeSet::new();
        for(i,r)in rows.iter().enumerate(){
            if usize::from(r.ordinal)!=i || r.role!=role_at(i) || r.volume.0==[0;16] || !volumes.insert(r.volume) ||
                r.token==[0;32] || !tokens.insert(r.token){return Err(invalid("duplicate or out-of-order initial claim"));}
            let budget=NameBudget::from_original(fs::path_budget(&r.path)?,r.limit);budget.child(ROOT_MARK)?;
            if i==0{budget.child(DB)?;budget.child(JOURNAL)?;}
        }
        let paths:Vec<_>=rows.iter().map(|r|r.path.as_path()).collect();
        if init_request(birth.op,birth.actor,birth.approval,birth.uid,&paths)?!=birth.request{
            return Err(invalid("original INIT fields disagree with stored request"));
        }
        Ok(Self{birth,rows})
    }
    pub(crate) fn state(&self)->InitState{self.birth.state}
    pub(crate) fn rows(&self)->&[ClaimRow]{&self.rows}
    /// Compare one ordinary marker with the expected SQL row. No marker can
    /// select a pool, substitute for a missing DB or move PendingIo to Committed.
    fn expected_marker(&self,ordinal:usize)->Option<[u8;101]>{
        let r=self.rows.get(ordinal)?;let mut b=[0;101];
        b[..20].copy_from_slice(MARK_DOMAIN);b[20]=role_tag(r.role);b[21..37].copy_from_slice(&self.birth.pool.0);
        b[37..53].copy_from_slice(&r.volume.0);b[53..69].copy_from_slice(&self.birth.op.0);b[69..].copy_from_slice(&r.token);
        Some(b)
    }
    pub(crate) fn marker_matches(&self,ordinal:usize,bytes:&[u8])->bool{
        self.expected_marker(ordinal).is_some_and(|mark|mark.as_slice()==bytes)
    }
}

#[cfg(test)]
#[path="../tests/unit/sqlite_init.rs"]
mod tests;
