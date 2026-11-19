//! A real confirmed parent and real native children, but NOT a published
//! nested FILE. The active SYSTEM/intent parser must still reject that kind.
use super::*;
use crate::{EntryId,FileId,HistoryId,Role,fs,ordinary::{allocation,journal::{self,Decision},wire::{Creation,Frame}},pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::path::Path;
fn op()->OperationId{OperationId::new().unwrap()}

#[test]
#[ignore="real registered ordinary roots, installed directories and separate recycle filesystem"]
fn child_birth_bytes_bind_a_real_parent_at_one_prefix_without_publishing_a_nested_file(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let a=t.child("a");let b=t.child("b");let system=t.child("system");
    let info=crate::initialize(op(),&system,&r.child("recycle"),&[a.clone(),b.clone()]).unwrap();
    let vid=|path:&Path|info.volumes.iter().find(|v|v.volume.path==path).unwrap().id;
    let(av,bv)=(vid(&a),vid(&b));let rv=info.volumes.iter().find(|v|v.volume.role==Role::Recycle).unwrap().id;
    let outer=crate::make_directory(&system,op(),Parent::Root(av),"outer").unwrap();
    let above=Parent::Child(av,outer.directory);
    let left=crate::make_directory(&system,op(),above,"left").unwrap();
    let right=crate::make_directory(&system,op(),above,"right").unwrap();
    let root_file=crate::create_empty(&system,op(),av,"report").unwrap();
    let left_at=Parent::Child(av,left.directory);let right_at=Parent::Child(av,right.directory);
    let(p,_,s)=LocalPool::with_snapshot(&system,Access::Write).unwrap();let reg=p.journal_registration();
    let uid=fs::local_uid();let v=p.ordinary(av).unwrap();let(seq,head)=s.frontier();
    let orig=s.file(root_file.file).unwrap();assert_eq!(orig.create.parent,BirthParent::Root);
    assert_eq!(wire::envelope(p.id(),&orig.bytes).unwrap().kind,ROOT);
    assert_eq!(Decision::read(p.id(),&orig.bytes).unwrap().creation().unwrap(),orig);
    assert_eq!(Frame::new(p.id(),orig.seq,orig.previous,orig.create.clone()).unwrap().bytes,orig.bytes);
    assert_eq!(orig.request,request(p.id(),orig.create.op,uid,Parent::Root(av),"report").unwrap());
    let scope=s.file_parent_at(left_at,uid,&reg).unwrap();
    let other=s.file_parent_at(right_at,uid,&reg).unwrap();assert_ne!(scope,other);
    assert!(s.file_parent_at(Parent::Root(rv),uid,&reg).is_err());
    assert!(s.file_parent_at(Parent::Child(bv,left.directory),uid,&reg).is_err());
    assert!(s.file_parent_at(Parent::Child(av,DirectoryId(root_file.file.0)),uid,&reg).is_err());
    assert!(s.file_parent_at(left_at,uid^1,&reg).is_err());
    let outer_native=v.root.child_directory(c"outer").unwrap().unwrap();
    let left_native=outer_native.child_directory(c"left").unwrap().unwrap();
    // These are unissued test-only material, not an ALLOC or an already
    // authenticated FILE creation. Same leaf as an existing ROOT FILE.
    let dir=left_native.new_child_directory(c"report").unwrap();let op1=op();let file=FileId::new().unwrap();let history=HistoryId::new().unwrap();
    let bytes=wire::empty_material(file,history,op1,"report");let(local,entity)=dir.anonymous_record().unwrap();
    let obj=NativeObject{entity,birth:fs::Birth::read(&local).unwrap()};dir.add_record(local,c"FILE",&bytes).unwrap();
    let c=Creation{parent:scope.clone(),op:op1,uid,volume:av,entry:EntryId::new().unwrap(),file,history,name:"report".into(),
        attempt:op(),slot:0,allocation:[9;32],directory:NativeObject{entity:dir.entity,birth:fs::Birth::read(&dir.file).unwrap()},
        local:obj,local_digest:crate::hash::digest(&bytes).unwrap()};
    let frame=Frame::new(p.id(),seq+1,head,c.clone()).unwrap();
    assert_eq!(Frame::read(p.id(),&frame.bytes).unwrap(),frame);s.check_next_file_parent(&reg,&frame).unwrap();
    assert!(Decision::read(p.id(),&frame.bytes).is_err());assert!(s.decided(op1).is_none());
    assert_eq!(frame.request,request(p.id(),op1,uid,left_at,"report").unwrap());
    for where_ in [Parent::Root(av),right_at,Parent::Child(bv,left.directory)]{
        assert_ne!(frame.request,request(p.id(),op1,uid,where_,"report").unwrap());
    }
    // An in-memory claim is not authenticated merely because it contains
    // the right directory spelling or the same native filesystem.
    let BirthParent::Child{decision,object,..}=&scope else{panic!("real child expected")};
    for bad_parent in [BirthParent::Child{directory:right.directory,decision:*decision,object:object.clone()},
        BirthParent::Child{directory:left.directory,decision:[3;32],object:object.clone()},
        BirthParent::Child{directory:left.directory,decision:*decision,object:orig.create.directory.clone()}]{
        let mut changed=c.clone();changed.parent=bad_parent;
        let f=Frame::new(p.id(),seq+1,head,changed).unwrap();assert!(s.check_next_file_parent(&reg,&f).is_err());
    }
    let mut alias=c.clone();alias.parent=BirthParent::Child{directory:left.directory,decision:*decision,object:c.directory.clone()};
    assert!(Frame::new(p.id(),seq+1,head,alias).is_err());
    let foreign=PoolId::new().unwrap();let f=Frame::new(foreign,seq+1,head,c.clone()).unwrap();
    assert!(s.check_next_file_parent(&reg,&f).is_err());assert!(s.check_next_file_parent(&reg,&Frame::new(p.id(),seq+2,head,c.clone()).unwrap()).is_err());
    let nested=wire::envelope(p.id(),&frame.bytes).unwrap();assert_eq!(nested.kind,CHILD);
    let root_equivalent=Frame::new(p.id(),seq+1,head,Creation{parent:BirthParent::Root,..c}).unwrap();
    let root=wire::envelope(p.id(),&root_equivalent.bytes).unwrap();let mut extra=nested.body.to_vec();extra.push(0);
    for (tag,claim,body) in [(ROOT,root.request,nested.body),(CHILD,nested.request,root.body),(CHILD,root.request,nested.body),
        (CHILD,nested.request,extra.as_slice())]{
        let(bad,_)=wire::encode(p.id(),seq+1,head,tag,claim,body).unwrap();assert!(Frame::read(p.id(),&bad).is_err());
    }
    assert_eq!(p.snapshot().unwrap().1.frontier(),(seq,head));
    assert!(!p.system.entry_present(allocation::NAME).unwrap()&&!p.system.entry_present(journal::INTENT).unwrap());
    drop(v);drop(p);
    // A real root-only ALLOC still cannot match a detached value whose
    // serialized root bytes and original request were copied unchanged but
    // whose claimed semantic parent was replaced in memory.
    let root_op=op();assert!(crate::ordinary::create_inner(&system,root_op,bv,"sealed",|cut|
        if cut==crate::ordinary::Cut::Intent{Err(io::Error::other("retain the root-only seal"))}else{Ok(())}).is_err());
    let(_,_,pending)=LocalPool::with_snapshot(&system,Access::Read).unwrap();
    let allocation=pending.allocated.as_ref().unwrap();let mut altered=pending.pending.as_ref().unwrap().frame.creation().unwrap().clone();
    assert!(allocation.matches(&altered));altered.create.parent=scope;assert!(!allocation.matches(&altered));
}
