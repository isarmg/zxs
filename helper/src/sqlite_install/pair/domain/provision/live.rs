//! Immutable public bytes on the one original private installation mount.
use crate::{pinned::{Entity,Mount},sqlite_install::bad};
use std::{fs::File,io,os::unix::fs::MetadataExt};

#[derive(Copy,Clone,PartialEq,Eq)]
pub(super) struct Stamp{pub(super) entity:Entity,pub(super) mount:u64,pub(super) mode:u32,pub(super) uid:u32,
    pub(super) links:u64,pub(super) len:u64,pub(super) mtime:(i64,i64),pub(super) ctime:(i64,i64)}
impl Stamp{
    pub(super) fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{entity:Entity::of(&m)?,
        mount:Mount::on(f)?.ok_or_else(||bad("missing installed product record mount"))?.id,
        mode:m.mode(),uid:m.uid(),links:m.nlink(),len:m.len(),
        mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}
    pub(super) fn at(&self,parent:&Stamp,length:u64,mode:u32,uid:u32)->bool{
        self.mode==(0o100_000|mode)&&self.uid==uid&&self.links==1&&self.len==length
            &&self.mount==parent.mount&&self.entity.dev==parent.entity.dev&&self.entity!=parent.entity
    }
}
