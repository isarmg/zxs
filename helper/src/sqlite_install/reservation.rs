//! The only OS file reservation for the fresh SQLite path. Once closed, the raw
//! DB is NEVER reopened for inspection. Only metadata and the SQLite VFS are used.
use super::{bad,engine,originals::Group,request::Request,DATABASE};
use crate::pinned::{Entity,Mount};
use rustix::fs::{AtFlags,Mode,OFlags,StatxFlags,makedev,openat,statx};
use std::{fs::File,io,os::unix::fs::MetadataExt,path::PathBuf};

pub(super) struct Reserved<'g>{group:&'g Group,path:PathBuf,entity:Entity,mount:u64}
impl<'g> Reserved<'g>{
    pub(super) fn create(group:&'g Group,request:&Request)->io::Result<Self>{
        let path=request.database(engine::path_limit()?)?;
        if group.uid()!=request.uid() || !group.matches(request){return Err(bad("wrong approved group for original INIT"));}
        group.recheck()?;
        for i in 0..group.dirs().len(){group.scan_names(i,&[])?;}
        group.recheck()?;
        let s=group.system();
        // Only this open can CREATE the DB. Never truncate/adopt an existing
        // regular file, and never repair a half-built database or an unknown name.
        let new=File::from(openat(s,c"SYSTEM.sqlite3",OFlags::RDWR|OFlags::CREATE|OFlags::EXCL|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR)?);
        let meta=new.metadata()?;let mount=Mount::on(&new)?.ok_or_else(||bad("new database has no mount identity"))?.id;
        let me=Self{group,path,entity:Entity::of(&meta)?,mount};
        if !meta.is_file()||meta.uid()!=request.uid()||meta.mode()&0o7777!=0o600||meta.nlink()!=1||meta.len()!=0
            || !Mount::on(s)?.is_some_and(|m|m.id==mount)||s.metadata()?.dev()!=meta.dev(){
            return Err(bad("exclusive creation did not yield a new private database on the original SYSTEM"));
        }
        me.require_name(true)?;new.sync_all()?;s.sync_all()?;me.require_name(true)?;
        // A raw close on an inode used by SQLite elsewhere in this process
        // would cancel SQLite POSIX locks. No connection exists in this stage.
        drop(new);group.scan_names(0,&[DATABASE.as_bytes()])?;group.recheck()?;me.require_name(true)?;
        Ok(me)
    }
    pub(super) fn path(&self)->&std::path::Path{&self.path}
    pub(super) fn pristine(&self)->io::Result<()>{
        self.group.scan_names(0,&[DATABASE.as_bytes()])?;self.require_name(true)
    }
    pub(super) fn require_name(&self,empty:bool)->io::Result<()>{
        // The SQLite Unix VFS resolves an absolute canonical pathname. Match
        // the *current* named chain and mount to our already-held original S,
        // not just "SYSTEM.sqlite3" inside a now-detached directory.
        self.group.require_system()?;
        let need=StatxFlags::TYPE|StatxFlags::MODE|StatxFlags::NLINK|StatxFlags::UID|StatxFlags::INO|StatxFlags::SIZE|StatxFlags::MNT_ID;
        let s=statx(self.group.system(),c"SYSTEM.sqlite3",AtFlags::SYMLINK_NOFOLLOW|AtFlags::NO_AUTOMOUNT,need)?;
        if s.stx_mask&need.bits()!=need.bits()|| s.stx_mode!=0o100600||s.stx_nlink!=1||s.stx_uid!=self.group.uid()
            || s.stx_ino!=self.entity.ino||makedev(s.stx_dev_major,s.stx_dev_minor)!=self.entity.dev||s.stx_mnt_id!=self.mount
            || empty&&s.stx_size!=0{return Err(bad("original database reservation changed"));}
        Ok(())
    }
    pub(super) fn sync_parent(&self)->io::Result<()>{self.group.system().sync_all()}
}
