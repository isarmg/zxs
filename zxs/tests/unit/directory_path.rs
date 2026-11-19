use super::*;
use crate::{EntryId,OperationId,identity::DirectoryId,ordinary::directory::types::Origin,pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::{fs as osfs,path::Path};

#[test]
#[ignore="two native private filesystems with exportable original identities"]
fn real_registered_root_is_a_parent_but_a_real_ordinary_directory_plus_its_detached_frame_is_not(){
    let op=||OperationId::new().unwrap();let t=PrivateTemp::under(&std::env::temp_dir());
    let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let data=t.child("data");let reg=crate::initialize(op(),&sys,&r.child("recycle"),std::slice::from_ref(&data)).unwrap();
    let id=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
    let(p,_,j)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let v=p.ordinary(id).unwrap();
    let root=HeldParent::open(&v,Parent::Root(id)).unwrap();assert_eq!(root.directory().entity,v.root.entity);root.require().unwrap();
    let original=v.root.new_child_directory(c"fixture").unwrap();
    let o=Origin{op:op(),uid:fs::local_uid(),parent:root.basis().clone(),name:"fixture".into(),entry:EntryId::new().unwrap(),id:DirectoryId::new().unwrap(),ticket:op()};
    let object=crate::ordinary::wire::NativeObject{entity:original.entity,birth:fs::Birth::read(&original.file).unwrap()};
    let d=Created::new(p.id(),1,j.frontier().1,o,[14;32],1,object).unwrap();d.relation(&p.journal_registration(),None).unwrap();
    assert_eq!(installed(&v.root,&d).unwrap().entity,original.entity); // one named physical row, not SYSTEM authority
    assert!(HeldParent::open(&v,Parent::Child(id,d.origin.id)).is_err());
    let slot=material::staging(d.origin.ticket,d.slot);let stray=v.root.new_child_directory(&slot).unwrap();assert!(installed(&v.root,&d).is_err());
    assert_eq!(fs::Birth::read(&stray.file).unwrap(),fs::Birth::read(&v.root.child_directory(&slot).unwrap().unwrap().file).unwrap());
    osfs::rename(&stray.path,data.join("unrelated")).unwrap();
    osfs::rename(&original.path,data.join("detached")).unwrap();let replacement=v.root.new_child_directory(c"fixture").unwrap();
    assert_ne!(replacement.entity,original.entity);assert!(installed(&v.root,&d).is_err());
    assert!(HeldParent::open(&v,Parent::Child(id,d.origin.id)).is_err());assert_eq!(p.snapshot().unwrap().1.frontier(),j.frontier());
}
