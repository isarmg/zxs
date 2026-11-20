//! An original registered ordinary root, selected by VolumeId, not a
//! FILE/account permission. Never routes a recycling root into normal I/O.
use super::{Catalog,InitialReader,check,smb::ordinary_directory::{self,Route},zxs_sql_named};
use crate::{Role,VolumeId,fs::{ComponentLimit,ControlRead,HeldDir,invalid,directory_birth::{DirectoryHold,DirectoryParent}}};
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
    parent:DirectoryParent,ancestors:Vec<DirectoryHold>,
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
    /// The volume claim still proves the enrolled F. A second exact
    /// ancestry selects the internal parent; lock only that directory.
    pub(super) fn at_parent(source:&'a InitialReader,c:&Catalog,route:&Route,lock:Option<bool>)->io::Result<Self>{
        let parent=route.parent();
        let mut f=Self::selected(source,c,route.volume(),if parent==DirectoryParent::Root{lock}else{None})?;
        if parent!=DirectoryParent::Root{
            f.ancestors=route.hold(&f.root)?;f.parent=parent;
            match lock{Some(true)=>f.folder().exclusive()?,Some(false)=>f.folder().share()?,None=>{}}
            f.require_streaming()?;
        }Ok(f)
    }
    pub(super) fn folder(&self)->&HeldDir{self.ancestors.last().map_or(&self.root,|child|child.directory())}
    pub(super) fn parent(&self)->DirectoryParent{self.parent}
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
        let b=Self{source,root,mark,expected,volume:id,ordinal:row.ordinal,parent:DirectoryParent::Root,ancestors:Vec::new()};b.require()?;Ok(b)
    }
    pub(super) fn require(&self)->io::Result<()>{
        self.source.dir.require()?;check(unsafe{zxs_sql_named(self.source.conn.0)})?;
        self.source.marker.require(&self.source.dir.root)?;
        let now=HeldDir::trusted_storage(&self.root.path)?;
        if now.entity!=self.root.entity||!now.same_mount(&self.root)?||
            self.mark.bytes(&self.root)?.as_slice()!=self.expected.as_slice(){
            return Err(invalid("original SQL declaration does not match this current ordinary root"));
        }
        ordinary_directory::keep_chain(&self.root,&self.ancestors)
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
        crate::fs::WitnessRead::require_parent(&self.root)?;self.mark.require(&self.root)?;
        ordinary_directory::keep_chain(&self.root,&self.ancestors)
    }
    pub(super) fn for_tree(&self,source:&InitialReader,volume:VolumeId)->bool{
        std::ptr::eq(self.source,source)&&self.volume==volume
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

/// The native R role has its own selected proof. It cannot be passed into
/// the ordinary volume writer or obtained by sending a path/volume to SMB.
pub(super) struct RecycleRoot<'a>{
    source:&'a InitialReader,root:HeldDir,mark:ControlRead,expected:[u8;101],volume:VolumeId,
}
impl<'a> RecycleRoot<'a>{
    // Call after the logical SQL selection and outside any read transaction.
    // A missing physical root is not an empty retained namespace.
    pub(super) fn at(source:&'a InitialReader,c:&Catalog)->io::Result<Self>{
        if c.birth.pool!=source.pool{return Err(invalid("recycle role belongs to another pool"));}
        let r=c.rows().get(1).filter(|r|r.ordinal==1&&r.role==Role::Recycle)
            .ok_or_else(||invalid("missing original recycle role"))?;
        let expected=c.expected_marker(1).ok_or_else(||invalid("missing original recycle marker"))?;
        let root=HeldDir::trusted_storage(&r.path).map_err(|e|
            if e.kind()==io::ErrorKind::NotFound{io::Error::new(io::ErrorKind::NotConnected,"registered recycle volume unavailable")}else{e})?;
        if root.entity.device==source.dir.root.entity.device||root.same_mount(&source.dir.root)?||
            ComponentLimit::held(&root)?!=r.limit{
            return Err(invalid("recycle root is not the independently registered filesystem or geometry"));
        }
        let mark=source.dir.hold_marker(&root)?;
        let held=Self{source,root,mark,expected,volume:r.volume};
        if held.mark.bytes(&held.root)?.as_slice()!=held.expected.as_slice(){
            return Err(invalid("wrong original recycle root marker"));
        }
        held.require()?;Ok(held)
    }
    pub(super) fn volume(&self)->VolumeId{self.volume}
    pub(super) fn root_for_read(&self)->&HeldDir{&self.root}
    pub(super) fn require(&self)->io::Result<()>{
        crate::fs::WitnessRead::require_parent(&self.source.dir.root)?;
        check(unsafe{zxs_sql_named(self.source.conn.0)})?;
        self.source.marker.require(&self.source.dir.root)?;
        crate::fs::WitnessRead::require_parent(&self.root)?;
        self.mark.require(&self.root)
    }
    /// No assumption that two mount IDs imply different disks. Reject
    /// known shared devices/mounts, without probing unrelated data volumes.
    pub(super) fn separate_from(&self,f:&OrdinaryRoot<'_>)->io::Result<()>{
        if !std::ptr::eq(self.source,f.source)||self.root.entity.device==f.root.entity.device||
            self.root.same_mount(&f.root)?{return Err(invalid("the selected ordinary and retained roles overlap"));}
        Ok(())
    }
}
