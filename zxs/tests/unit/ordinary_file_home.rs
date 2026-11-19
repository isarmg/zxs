//! Files and system mutations described in these tests are local fixture data.
use crate::test_support::physical as native_leaf;
use super::*;
use crate::{OperationId,ordinary::{Cut,create_inner},pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::{os::unix::fs::MetadataExt,path::Path};

fn op()->OperationId{OperationId::new().unwrap()}

#[test]
#[ignore="two private registered filesystems; real FILE and two installed ordinary parents"]
fn held_root_birth_cannot_be_relabelled_as_a_matching_real_container_under_a_child(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let other=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=tmp.child("s");let data=tmp.child("data");let bin=other.child("bin");
    let reg=crate::initialize(op(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let id=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
    let left=crate::make_directory(&sys,op(),Parent::Root(id),"left").unwrap();
    let right=crate::make_directory(&sys,op(),Parent::Root(id),"right").unwrap();
    let file=crate::create_empty(&sys,op(),id,"report").unwrap();
    let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let v=p.ordinary(id).unwrap();
    let real=s.file(file.file).unwrap();let original=open(&v,&s,real).unwrap();let root=data.join(native_leaf("report",file.file));
    assert_eq!(original.entity.inode,std::fs::metadata(&root).unwrap().ino());drop(original);
    assert_eq!(s.on(id).map(|f|f.create.file).collect::<Vec<_>>(),[file.file]);
    for directory in [left.directory,right.directory]{
        let child=Parent::Child(id,directory);assert_eq!(s.at_parent(child).count(),0);
        let mut made=real.create.clone();made.parent=s.file_parent_at(child,real.create.uid,&p.journal_registration()).unwrap();
        let detached=Frame::new(p.id(),real.seq,real.previous,made).unwrap();assert_ne!(detached.digest,real.digest);
        let at=data.join(if directory==left.directory{"left"}else{"right"}).join(native_leaf("report",file.file));
        std::fs::rename(&root,&at).unwrap();
        // Every FILE byte and both native births still exist; only the occurrence is wrong.
        assert_eq!(std::fs::metadata(&at).unwrap().ino(),real.create.directory.entity.inode);
        assert!(open(&v,&s,real).is_err());assert!(open(&v,&s,&detached).is_err());
        assert_eq!(p.snapshot().unwrap().1.frontier(),s.frontier());
        assert!(!p.system.entry_present(crate::ordinary::allocation::NAME).unwrap());
        assert!(!p.system.entry_present(crate::ordinary::journal::INTENT).unwrap());
        std::fs::rename(&at,&root).unwrap();assert_eq!(open(&v,&s,real).unwrap().entity,real.create.directory.entity);
    }
}

#[test]
#[ignore="two private original filesystems; complete unconfirmed frame versus advanced durable floor"]
fn unchanged_birth_uses_the_confirmed_floor_not_the_register_length_or_a_stale_journal(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let other=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=tmp.child("s");let data=tmp.child("data");let bin=other.child("bin");
    let reg=crate::initialize(op(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let id=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
    let f=crate::create_empty(&sys,op(),id,"original").unwrap();
    let old=LocalPool::with_snapshot(&sys,Access::Read).unwrap().2;
    let birth=old.file(f.file).unwrap();let second=op();let mut hit=false;
    assert!(create_inner(&sys,second,id,"sibling",|c|{
        if c==Cut::FrameSynced{hit=true;Err(io::Error::other("append and fsync, before confirmation"))}else{Ok(())}
    }).is_err());assert!(hit);
    {
        let(p,_,now)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let v=p.ordinary(id).unwrap();
        assert_eq!(old.frontier(),now.frontier());assert!(now.decided(second).is_none());
        assert!(p.system.entry_present(crate::ordinary::journal::INTENT).unwrap());
        assert_eq!(open(&v,&old,birth).unwrap().entity,birth.create.directory.entity);
    }
    let made=crate::create_empty(&sys,second,id,"sibling").unwrap();assert_ne!(made.file,f.file);
    let(p,_,now)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let v=p.ordinary(id).unwrap();
    assert_ne!(old.frontier(),now.frontier());assert_eq!(now.file(f.file),Some(birth));
    assert!(open(&v,&old,birth).is_err()); // even though both original native objects remain
    assert_eq!(open(&v,&now,birth).unwrap().entity,birth.create.directory.entity);
}

#[test]
#[ignore="two private original filesystems; already-held cut and corrupted original register"]
fn a_floor_attribute_and_healthy_file_do_not_substitute_for_lost_or_damaged_confirmed_frames(){
    use std::time::{Duration,UNIX_EPOCH};
    for shorten in [true,false]{
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let other=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        let sys=tmp.child("s");let data=tmp.child("data");let bin=other.child("bin");
        let reg=crate::initialize(op(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
        let id=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
        let f=crate::create_empty(&sys,op(),id,"original").unwrap();let last=op();
        crate::create_empty(&sys,last,id,"other").unwrap();
        let(p,stamp,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let v=p.ordinary(id).unwrap();
        let file=s.file(f.file).unwrap();assert!(open(&v,&s,file).is_ok());
        let named=sys.join("REGISTRATION");let bytes=std::fs::read(&named).unwrap();let budget=bytes.len();
        let first=budget-s.decided(last).unwrap().bytes().len();let floor=s.confirmed_floor();
        assert_eq!(p.system.read_frontier(crate::pool::READY,&stamp,budget).unwrap(),floor);
        let mut changed=bytes.clone();
        if shorten{changed.truncate(first);}else{changed[budget-1]^=1;} // the other, still-confirmed decision
        std::fs::write(&named,&changed).unwrap();
        let raw=std::fs::OpenOptions::new().write(true).open(&named).unwrap();
        // Distinguish the equal-sized write even on a coarse timestamp fixture filesystem.
        raw.set_modified(UNIX_EPOCH+Duration::from_secs(123456789)).unwrap();raw.sync_all().unwrap();
        let(now,seen)=p.system.record(crate::pool::READY,budget).unwrap().unwrap();
        assert_eq!(now.stat.entity(),stamp.stat.entity());assert_eq!(now.birth,stamp.birth);assert_eq!(seen,changed);
        assert_eq!(p.system.read_frontier(crate::pool::READY,&now,budget).unwrap(),floor);
        assert!(open(&v,&s,file).is_err());assert!(p.snapshot().is_err());
        // The reader changes neither suspect register nor healthy native FILE.
        assert_eq!(std::fs::read(&named).unwrap(),changed);assert_eq!(std::fs::metadata(data.join(native_leaf("original",f.file))).unwrap().ino(),file.create.directory.entity.inode);
        std::fs::write(&named,&bytes).unwrap();raw.sync_all().unwrap();
        assert!(open(&v,&s,file).is_ok());
    }
}
