//! An original registered ordinary root, selected by VolumeId, not a
//! FILE/account permission. Never routes a recycling root into normal I/O.
use super::{Catalog,InitialReader,check,zxs_sql_named};
use crate::{Role,VolumeId,fs::{ComponentLimit,ControlRead,HeldDir,invalid}};
use std::{io,os::{fd::AsRawFd,raw::c_int}};

unsafe extern "C"{fn zxs_storage_capacity(fd:c_int,total:*mut u64,available:*mut u64)->c_int;}

#[derive(Debug,Eq,PartialEq)]
pub(crate) struct DiskSpace{
    pub(crate) volume:VolumeId,
    /// fstatvfs on the actual held directory. Two logical roots on one FS
    /// may report the same total; neither this nor a marker is disk health.
    pub(crate) filesystem_bytes:u64,pub(crate) available_bytes:u64,
}
pub(super) struct OrdinaryRoot<'a>{
    source:&'a InitialReader,pub(super) root:HeldDir,mark:ControlRead,expected:[u8;101],volume:VolumeId,pub(super) ordinal:u8,
}
impl InitialReader{
    /// A fresh SQL selection for exactly one mounted-FS capacity sample.
    /// No holder for taking another sample with yesterday's permission/cut escapes.
    pub(crate) fn ordinary_space(&self,id:VolumeId)->io::Result<DiskSpace>{
        self.ordinary_space_after(id,||Ok(()))
    }
    pub(super) fn ordinary_space_after(&self,id:VolumeId,after_open:impl FnOnce()->io::Result<()>)->io::Result<DiskSpace>{
        let catalog=self.snapshot()?; // Actual short SQL cut ended before any target I/O.
        let b=OrdinaryRoot::at(self,&catalog,id,false)?;
        after_open()?;b.space()
    }
}
impl<'a> OrdinaryRoot<'a>{
    // Physical proof only. The caller supplies the catalog from its own
    // authorization cut. A capacity sample cannot be converted into a write.
    pub(super) fn at(source:&'a InitialReader,catalog:&Catalog,id:VolumeId,namespace_write:bool)->io::Result<Self>{
        Self::selected(source,catalog,id,Some(namespace_write))
    }
    /// The body phase takes the actual FILE lock, not a volume-wide lock;
    /// still retain the actual registered root, marker, same mount and S.
    pub(super) fn for_body(source:&'a InitialReader,catalog:&Catalog,id:VolumeId)->io::Result<Self>{
        Self::selected(source,catalog,id,None)
    }
    fn selected(source:&'a InitialReader,catalog:&Catalog,id:VolumeId,lock:Option<bool>)->io::Result<Self>{
        let (i,row)=catalog.rows().iter().enumerate().find(|(_,r)|r.volume==id&&r.role==Role::File)
             .ok_or_else(||invalid("not an ordinary VolumeId in this original pool"))?;
        let expected=catalog.expected_marker(i).ok_or_else(||invalid("missing original SQL volume claim"))?;
        let root=HeldDir::trusted_storage(&row.path)?;
        match lock{Some(true)=>root.exclusive()?,Some(false)=>root.share()?,None=>{}} // only this F is opened
        if root.entity==source.dir.root.entity || ComponentLimit::held(&root)?!=row.limit{
            return Err(invalid("selected original F aliases S or has different native geometry"));
        }
        let mark=source.dir.hold_marker(&root)?;
        let b=Self{source,root,mark,expected,volume:id,ordinal:row.ordinal};b.require()?;Ok(b)
    }
    pub(super) fn require(&self)->io::Result<()>{
        self.source.dir.require()?;check(unsafe{zxs_sql_named(self.source.conn.0)})?;
        self.source.marker.require(&self.source.dir.root)?;
        let now=HeldDir::trusted_storage(&self.root.path)?;
        if now.entity!=self.root.entity||!now.same_mount(&self.root)?||
            self.mark.bytes(&self.root)?.as_slice()!=self.expected.as_slice(){
            return Err(invalid("original SQL declaration does not match this current ordinary root"));
        }
        Ok(())
    }
    /// A stream already selected this F from a verified short SQL snapshot.
    /// Check its held names, both exact private parents and marker stamps
    /// without re-enumerating S, re-reading its SQL schema or probing R.
    pub(super) fn require_streaming(&self)->io::Result<()>{
        self.require_streaming_at(||Ok(()))
    }
    /// The caller's physical phase. No target-root lookup belongs inside a
    /// SYSTEM rollback-journal transaction; a blocked F must not pin it.
    pub(super) fn require_streaming_at(&self,at_target:impl FnOnce()->io::Result<()>)->io::Result<()>{
        crate::fs::WitnessRead::require_parent(&self.source.dir.root)?;
        check(unsafe{zxs_sql_named(self.source.conn.0)})?;
        self.source.marker.require(&self.source.dir.root)?;
        at_target()?;
        crate::fs::WitnessRead::require_parent(&self.root)?;self.mark.require(&self.root)
    }
    pub(super) fn space(&self)->io::Result<DiskSpace>{
        self.require()?;let(mut total,mut available)=(0,0);
        // SAFETY: actual already-held ordinary root FD, two live output integers.
        if unsafe{zxs_storage_capacity(self.root.file.as_raw_fd(),&mut total,&mut available)}<0{
            return Err(io::Error::last_os_error());
        }
        self.require()?;Ok(DiskSpace{volume:self.volume,filesystem_bytes:total,available_bytes:available})
    }
}
