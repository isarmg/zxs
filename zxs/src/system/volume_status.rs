//! A single authenticated, on-demand observation of one registered volume.
//! No filesystem information is used as ownership, health or an I/O grant.
use super::{WebAdmin,manual_work::{Site,Who}};
use super::super::{Catalog,InitialReader,volume::{OrdinaryRoot,RecycleRoot}};
use crate::{Role,VolumeId,fs::{HeldDir,invalid},system::ClaimRow};
use std::{io,os::{fd::AsRawFd,raw::c_int}};

unsafe extern "C"{
    fn zxs_storage_capacity(fd:c_int,total:*mut u64,available:*mut u64)->c_int;
    fn zxs_storage_uuid(fd:c_int,uuid:*mut u8)->c_int;
}

#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Reading{
    pub(crate) bytes:u64,
    pub(crate) available:u64,
    pub(crate) uuid:Option<[u8;16]>,
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Report{
    pub(crate) volume:VolumeId,
    pub(crate) role:Role,
    pub(crate) current:Option<Reading>,
}
fn claim(c:&Catalog,id:VolumeId)->io::Result<&ClaimRow>{
    c.rows().iter().find(|p|p.volume==id).ok_or_else(||io::ErrorKind::NotFound.into())
}
enum Disk<'a>{System(&'a InitialReader),File(OrdinaryRoot<'a>),Recycle(RecycleRoot<'a>)}
impl<'a> Disk<'a>{
    fn open(src:&'a InitialReader,c:&Catalog,r:&ClaimRow)->io::Result<Self>{
        match r.role{
            Role::System=>Ok(Self::System(src)), // Already held and selected by the original S cut.
            Role::File=>Ok(Self::File(OrdinaryRoot::for_body(src,c,r.volume)?)), // No FILE or namespace lock.
            Role::Recycle=>Ok(Self::Recycle(RecycleRoot::at(src,c)?)),
        }
    }
    fn root(&self)->&HeldDir{
        match self{
            Self::System(s)=>&s.dir.root,
            Self::File(f)=>&f.root,
            Self::Recycle(r)=>r.root_for_read(),
        }
    }
    fn require(&self)->io::Result<()>{
        match self{
            Self::System(s)=>{s.dir.require()?;s.marker.require(&s.dir.root)},
            Self::File(f)=>f.require_streaming(),
            Self::Recycle(r)=>r.require(),
        }
    }
}
fn read(root:&HeldDir)->io::Result<Reading>{
    let(mut bytes,mut available,mut uuid)=(0,0,[0;16]);
    let fd=root.file.as_raw_fd();
    // The actual proved held directory FD; no drive path, file body or fd from HTTP.
    if unsafe{zxs_storage_capacity(fd,&mut bytes,&mut available)}<0{return Err(io::Error::last_os_error());}
    let found=unsafe{zxs_storage_uuid(fd,uuid.as_mut_ptr())}==0;
    Ok(Reading{bytes,available,uuid:(found&&uuid!=[0;16]).then_some(uuid)})
}
impl Site<'_>{
    pub(crate) fn observe(&self,who:Who,id:VolumeId,live:impl Fn()->io::Result<()>)->io::Result<Report>{
        live()?;
        self.with_admin(who,|admin|admin.observe_after(id,live,|_|{}))
    }
}
impl WebAdmin<'_>{
    pub(in crate::system::reader) fn observe_after(&self,id:VolumeId,live:impl Fn()->io::Result<()>,after:impl FnOnce(&HeldDir))->io::Result<Report>{
        let catalog=self.source.at(|db,c|{self.current(db,&c)?;claim(&c,id)?;Ok(c)})?;
        let original=claim(&catalog,id)?;
        live()?;
        // Neither the SQLite cut nor a lock on a data directory is held during F/R I/O.
        let disk=Disk::open(self.source,&catalog,original);
        let mut current=None;
        if let Ok(held)=&disk{
            live()?;
            if let Ok(value)=read(held.root()){
                after(held.root());
                if held.require().is_ok(){current=Some(value);}
            }
        }
        // Hold that same physical sample through the final short S cut. A false/
        // missing physical observation is reported only to this same current admin.
        self.source.at(|db,c|{
            self.current(db,&c)?;
            if claim(&c,id)?!=original{return Err(invalid("registered volume changed during the observation"));}
            live()
        })?;
        live()?;
        let _held=disk;
        Ok(Report{volume:id,role:original.role,current})
    }
}
