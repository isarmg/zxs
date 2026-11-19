//! The only OS file reservation for the fresh SQLite path. Once closed, the raw
//! DB is NEVER reopened for inspection. Only metadata and the SQLite VFS are used.
use super::{bad,engine,originals::Group,request::Request,DATABASE};
use crate::pinned::{Entity,Mount};
use rustix::fs::{AtFlags,Mode,OFlags,StatxFlags,makedev,openat,statx};
use std::{fs::File,io,os::unix::fs::MetadataExt,path::PathBuf};

pub(super) struct Reserved<'g>(DbName<'g>);
pub(super) struct Existing<'g>(DbName<'g>);
struct DbName<'g>{group:&'g Group,path:PathBuf,entity:Entity,mount:u64}
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
        let me=Self(DbName{group,path,entity:Entity::of(&meta)?,mount});
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
    pub(super) fn path(&self)->&std::path::Path{&self.0.path}
    pub(super) fn same_existing(&self,other:&Existing<'_>)->bool{
        std::ptr::eq(self.0.group,other.0.group)&&self.0.entity==other.0.entity&&self.0.mount==other.0.mount
    }
    pub(super) fn pristine(&self)->io::Result<()>{
        self.0.group.scan_names(0,&[DATABASE.as_bytes()])?;self.require_name(true)
    }
    pub(super) fn require_name(&self,empty:bool)->io::Result<()>{self.0.check(empty).map(|_|())}
    pub(super) fn sync_parent(&self)->io::Result<()>{self.0.group.system().sync_all()}
}
impl<'g> Existing<'g>{
    pub(super) fn observe(group:&'g Group,r:&Request)->io::Result<Self>{
        if group.uid()!=r.uid()||!group.matches(r){return Err(bad("selected group does not match original INIT"));}
        let path=r.database(engine::path_limit()?)?;
        if !group.controls(0,&[DATABASE.as_bytes(),super::MARKER])?[0]{return Err(bad("existing original database is missing, not a new installation"));}
        let s=statx(group.system(),c"SYSTEM.sqlite3",AtFlags::SYMLINK_NOFOLLOW|AtFlags::NO_AUTOMOUNT,StatxFlags::BASIC_STATS|StatxFlags::MNT_ID)?;
        let x=Self(DbName{group,path,entity:Entity{dev:makedev(s.stx_dev_major,s.stx_dev_minor),ino:s.stx_ino},mount:s.stx_mnt_id});
        x.require()?;Ok(x)
    }
    pub(super) fn path(&self)->&std::path::Path{&self.0.path}
    pub(super) fn require(&self)->io::Result<()>{
        let me=&self.0;let s=me.group.system();
        if me.entity.ino==0||me.entity.dev!=s.metadata()?.dev()||Mount::on(s)?.is_none_or(|x|x.id!=me.mount)
            ||me.check(false)?==0 {return Err(bad("expected a positive-sized original database on SYSTEM"));}
        Ok(())
    }
    pub(super) fn sidecars_absent(&self)->io::Result<()>{
        // This narrow action cannot repair a hot journal and never silently
        // opens its unknown source RW in order to identify that source.
        if !self.0.group.controls(0,&[DATABASE.as_bytes(),super::MARKER])?[0]{return Err(bad("original database not present"));}
        self.require()
    }
}
impl DbName<'_>{
    fn check(&self,empty:bool)->io::Result<u64>{
        // The SQLite Unix VFS resolves an absolute canonical pathname. Match
        // the *current* named chain and mount to our already-held original S,
        // not just "SYSTEM.sqlite3" inside a now-detached directory.
        self.group.require_system()?;
        let need=StatxFlags::TYPE|StatxFlags::MODE|StatxFlags::NLINK|StatxFlags::UID|StatxFlags::INO|StatxFlags::SIZE|StatxFlags::MNT_ID;
        let s=statx(self.group.system(),c"SYSTEM.sqlite3",AtFlags::SYMLINK_NOFOLLOW|AtFlags::NO_AUTOMOUNT,need)?;
        if s.stx_mask&need.bits()!=need.bits()|| s.stx_mode!=0o100600||s.stx_nlink!=1||s.stx_uid!=self.group.uid()
            || s.stx_ino!=self.entity.ino||makedev(s.stx_dev_major,s.stx_dev_minor)!=self.entity.dev||s.stx_mnt_id!=self.mount
            || empty&&s.stx_size!=0{return Err(bad("original database reservation changed"));}
        Ok(s.stx_size)
    }
}
