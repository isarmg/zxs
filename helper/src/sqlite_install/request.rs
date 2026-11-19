//! The original INIT request never includes randomly issued results, a password
//! or a claim inferred from looking at a disk. All paths are unaltered Unix bytes.
use super::{bad,unsupported,DATABASE};
use crate::path_rules::Kind;
use sha2::{Digest,Sha256};
use std::{io,os::unix::ffi::OsStrExt,path::{Path,PathBuf}};

pub(super) type Id=[u8;16];
#[derive(Clone)]
pub(super) struct Request{
    op:Id,actor:Id,approval:Id,uid:u32,paths:Vec<PathBuf>,sha:[u8;32],
}

impl Request{
    pub(super) fn new(op:Id,actor:Id,approval:Id,uid:u32,paths:Vec<PathBuf>)->io::Result<Self>{
        if [op,actor,approval].contains(&[0;16]) || uid==0 || uid==u32::MAX || !(3..=64).contains(&paths.len()){
            return Err(bad("missing INIT identity, invalid storage UID, or wrong number of roots"));
        }
        let mut h=Sha256::new();
        h.update(b"ZXS-SQLITE-1/INIT\0");h.update(op);h.update(actor);h.update(approval);
        h.update(uid.to_be_bytes());h.update((paths.len() as u16).to_be_bytes());
        for (i,path) in paths.iter().enumerate(){
            let raw=path.as_os_str().as_bytes();Kind::StorageRoot.check_path(raw).map_err(bad)?;
            if i==0 && path.to_str().is_none(){return Err(unsupported("SYSTEM must be losslessly representable by the Unix SQLite VFS"));}
            if (i>2 && paths[i-1].as_os_str().as_bytes()>=raw)
                || paths[..i].iter().any(|p|p.starts_with(path)||path.starts_with(p)){
                return Err(bad("overlapping roots or ordinary roots not in exact raw byte order"));
            }
            h.update([role(i)]);h.update((raw.len() as u16).to_be_bytes());h.update(raw);
        }
        Ok(Self{op,actor,approval,uid,paths,sha:h.finalize().into()})
    }
    pub(super) fn sha(&self)->[u8;32]{self.sha}
    pub(super) fn op(&self)->Id{self.op}
    pub(super) fn actor(&self)->Id{self.actor}
    pub(super) fn approval(&self)->Id{self.approval}
    pub(super) fn uid(&self)->u32{self.uid}
    pub(super) fn paths(&self)->&[PathBuf]{&self.paths}
    pub(super) fn database(&self,limit:usize)->io::Result<PathBuf>{database_path(&self.paths[0],limit)}
}
pub(super) fn role(i:usize)->u8{match i {0=>0,1=>1,_=>2}}

fn database_path(system:&Path,limit:usize)->io::Result<PathBuf>{
    let p=system.join(DATABASE);
    if p.to_str().is_none() || p.as_os_str().len().checked_add(b"-journal\0".len()).is_none_or(|n|n>limit){
        return Err(unsupported("SYSTEM database and rollback-journal name do not both fit the qualified Unix VFS"));
    }
    Ok(p)
}
#[cfg(test)]
#[path="../../tests/sqlite/request.rs"] mod tests;
