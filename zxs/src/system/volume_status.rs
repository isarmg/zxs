//! A single authenticated, on-demand observation of one registered volume.
//! No filesystem information is used as ownership, health or an I/O grant.
use super::{WebAdmin,manual_work::{Site,Who}};
use super::super::{Catalog,InitialReader,volume::{OrdinaryRoot,RecycleRoot}};
use crate::{Role,VolumeId,fs::{HeldDir,invalid},system::ClaimRow};
use std::{fmt,io,os::{fd::AsRawFd,raw::c_int}};

unsafe extern "C"{
    fn zxs_storage_capacity_status(fd:c_int,total:*mut u64,available:*mut u64,read_only:*mut c_int)->c_int;
    fn zxs_storage_uuid(fd:c_int,uuid:*mut u8)->c_int;
}

/// Exact bytes returned by FS_IOC_GETFSUUID; a short identifier is not filled
/// out to a UUID or reinterpreted as a disk serial.
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct FilesystemId{bytes:[u8;16],len:u8}
impl FilesystemId{
    fn observed(mut bytes:[u8;16],len:c_int)->Option<Self>{
        let len=usize::try_from(len).ok()?;
        let value=bytes.get(..len)?;
        if !value.iter().any(|&b|b!=0){return None;}
        bytes[len..].fill(0); // outside the reported length is not part of the identity
        Some(Self{bytes,len:len as u8})
    }
    pub(crate) fn len(&self)->u8{self.len}
}
impl fmt::Display for FilesystemId{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        for (i,b) in self.bytes[..usize::from(self.len)].iter().enumerate(){
            if self.len==16&&matches!(i,4|6|8|10){f.write_str("-")?;}
            write!(f,"{b:02x}")?;
        }
        Ok(())
    }
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Reading{
    pub(crate) bytes:u64,
    pub(crate) available:u64,
    pub(crate) mount_read_only:bool,
    pub(crate) filesystem_id:Option<FilesystemId>,
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
    let(mut bytes,mut available,mut read_only,mut uuid)=(0,0,-1,[0;16]);
    let fd=root.file.as_raw_fd();
    // The actual proved held directory FD; no drive path, file body or fd from HTTP.
    if unsafe{zxs_storage_capacity_status(fd,&mut bytes,&mut available,&mut read_only)}<0{return Err(io::Error::last_os_error());}
    let mount_read_only=match read_only{0=>false,1=>true,_=>return Err(invalid("invalid kernel mount observation"))};
    let len=unsafe{zxs_storage_uuid(fd,uuid.as_mut_ptr())};
    Ok(Reading{bytes,available,mount_read_only,filesystem_id:FilesystemId::observed(uuid,len)})
}

#[cfg(test)]
mod tests{
    use super::FilesystemId;
    #[test]
    fn a_short_kernel_id_keeps_its_bytes_and_is_not_padded_to_a_uuid(){
        let kernel=[0x89,0,0x12,0xff,1,2,3,4,5,6,7,8,9,10,11,12];
        let short=FilesystemId::observed(kernel,4).unwrap();
        assert_eq!(short.len(),4);
        assert_eq!(short.to_string(),"890012ff");
        let full=FilesystemId::observed(kernel,16).unwrap();
        assert_eq!(full.to_string(),"890012ff-0102-0304-0506-0708090a0b0c");
        for n in [-1,0,17]{assert!(FilesystemId::observed(kernel,n).is_none());}
        assert!(FilesystemId::observed([0;16],16).is_none());
        let mut zero_prefix=kernel;zero_prefix[..4].fill(0);
        assert!(FilesystemId::observed(zero_prefix,4).is_none()); // no fallback to trailing data
    }
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
