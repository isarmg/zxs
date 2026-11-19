//! Original, bounded operands in private fixtures. The real cold SYSTEM
//! reader recognizes an exact unbound child ticket but cannot promote it to a FILE.
use super::*;
use crate::{DirectoryId,DirectoryParent as Parent,ordinary::{material,journal,wire::{self,Creation}},pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::path::Path;
fn op()->OperationId{OperationId::new().unwrap()}
fn native(d:&HeldDir)->NativeObject{NativeObject{entity:d.entity,birth:fs::Birth::read(&d.file).unwrap()}}

#[test]
fn maximal_child_file_allocation_is_one_bounded_original_record(){
    // Format width only; syntactically valid handles are not real native proof.
    let item=|n:u8|{let mut raw=[1;fs::MAX_BIRTH];raw[fs::MAX_BIRTH-1]=n;
        NativeObject{entity:crate::Entity{device:1,inode:u64::from(n)},birth:fs::Birth::decode(&raw).unwrap()}};
    let root=item(2);let owner=item(1);let scope=BirthParent::Child{directory:DirectoryId([4;16]),decision:[9;32],object:item(3)};
    let name="a".repeat(wire::MAX_HUMAN);let terms=Terms{pool:PoolId([1;16]),op:OperationId([2;16]),uid:0,volume:VolumeId([3;16]),
        parent:&scope,name:&name,seq:1,previous:[8;32],entry:EntryId([5;16]),file:FileId([6;16]),history:HistoryId([7;16]),attempt:OperationId([8;16]),root:&root};
    let(bytes,_)=terms.image(&owner).unwrap();assert_eq!(bytes.len(),MAX_CHILD_IMAGE);assert!(bytes.len()<=MAX);
    assert!(terms.image(&root).is_err());
}

#[test]
#[ignore="actual original SYSTEM holder, ordinary directory, separate recycle filesystem: a detached child operand cannot perform root I/O"]
fn genuine_child_operand_is_inode_scoped_and_cannot_be_mutated_by_a_root_adapter(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("ordinary");let recycle=r.child("recycle");
    let pool=crate::initialize(op(),&sys,&recycle,&[a.clone()]).unwrap();
    let vid=pool.volumes.iter().find(|v|v.volume.path==a).unwrap().id;
    let left=crate::make_directory(&sys,op(),Parent::Root(vid),"left").unwrap();
    let right=crate::make_directory(&sys,op(),Parent::Root(vid),"right").unwrap();
    let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let v=p.ordinary(vid).unwrap();let uid=fs::local_uid();
    let root_parent=crate::ordinary::directory::path::HeldParent::at_cut(&v,Parent::Root(vid),&s).unwrap();
    let addr=Parent::Child(vid,left.directory);let pool=p.id();let scope=s.file_parent_at(addr,uid,&p.journal_registration()).unwrap();
    let other=s.file_parent_at(Parent::Child(vid,right.directory),uid,&p.journal_registration()).unwrap();
    let root=native(&v.root);let h=v.root.child_directory(c"left").unwrap().unwrap();
    assert!(matches!(&scope,BirthParent::Child{object,..} if *object==native(&h)));
    let (seq,prev)=s.frontier();let issued=op();let attempt=op();
    let terms=Terms{pool,op:issued,uid,volume:vid,parent:&scope,name:"report",seq:seq+1,previous:prev,entry:EntryId::new().unwrap(),
        file:FileId::new().unwrap(),history:HistoryId::new().unwrap(),attempt,root:&root};
    let(record,entity)=p.system.anonymous_record().unwrap();let issuer=NativeObject{entity,birth:fs::Birth::read(&record).unwrap()};
    let(image,digest)=terms.image(&issuer).unwrap();assert!(!root_image(&image));
    assert_ne!(Terms{parent:&other,..terms}.image(&issuer).unwrap().0,image);
    assert!(Terms{root:&native(&h),..terms}.image(&issuer).is_err());
    fs::initial_attempt_binding(&record,&binding(digest,None).unwrap()).unwrap();p.system.add_record(record,NAME,&image).unwrap();
    let decode=||{let(st,b)=p.system.record(NAME,MAX).unwrap().unwrap();Allocation::from_bytes(&p.system,pool,st,b).unwrap()};
    let root_closed=||{assert!(Allocation::read(&p.system,pool).is_err());assert!(!p.system.entry_present(journal::INTENT).unwrap());};
    let unbound=decode();assert_eq!(unbound.parent,scope);assert!(unbound.stage.is_none());root_closed();
    assert_eq!(p.snapshot().unwrap().1.allocated.as_ref(),Some(&unbound));
    // A detached child reservation must not even create/sync an empty private
    // slot at the volume root. Decoding the child opcode does not issue it at a different parent.
    assert!(unbound.bind_fresh(&root_parent,||panic!("child operand created a root directory")).is_err());
    for slot in 0..SLOTS{assert!(!v.root.entry_present(&material::staging(attempt,slot)).unwrap());}
    h.only(None).unwrap();root_closed();
    let name=material::staging(attempt,0);let leaf_name=name.to_str().unwrap();
    let dir=h.new_child_directory(&name).unwrap();let st=Stage{slot:0,object:native(&dir)};
    assert!(unbound.bind_fresh(&root_parent,||panic!("detached child could name a root slot")).is_err());
    // An incorrect in-memory tag is not the original issued command. Check
    // the persisted child operand again before the test changes its xattr.
    let mut relabelled=decode();relabelled.parent=BirthParent::Root;
    assert!(relabelled.bind_fresh(&root_parent,||panic!("relabelled value crossed the original slot")).is_err());assert!(decode().stage.is_none());
    assert_eq!(decode().bytes,image);root_closed();
    // Install only the test xattr on the real SYSTEM operand. Moving the exact
    // staged inode to the spelling expected by the wrong adapter must not
    // let that adapter populate it or rename it to a business FILE.
    let real=binding(digest,Some(&st)).unwrap();
    p.system.bind_attempt(NAME,&unbound.stamp,MAX,&binding(digest,None).unwrap(),&real).unwrap();
    let bound=decode();assert_eq!(bound.stage.as_ref(),Some(&st));root_closed();
    assert_eq!(p.snapshot().unwrap().1.allocated.as_ref(),Some(&bound)); // a native stage claim is not publication
    let old=h.path.join(leaf_name);let wrong=v.root.path.join(leaf_name);
    std::fs::rename(&old,&wrong).unwrap();
    let same=v.root.child_directory(&name).unwrap().unwrap();assert_eq!(native(&same),st.object);
    assert!(material::prepare(&root_parent,&bound).is_err());same.only(None).unwrap();
    assert!(!same.entry_present(c"FILE").unwrap());h.only(None).unwrap();
    let (leaf,entity)=same.anonymous_record().unwrap();let local=NativeObject{entity,birth:fs::Birth::read(&leaf).unwrap()};
    let origin=wire::empty_material(terms.file,terms.history,issued,"report");same.add_record(leaf,c"FILE",&origin).unwrap();
    let c=Creation{parent:scope.clone(),op:issued,uid,volume:vid,entry:terms.entry,file:terms.file,history:terms.history,name:"report".into(),
        attempt,slot:0,allocation:digest,directory:st.object.clone(),local,local_digest:crate::hash::digest(&origin).unwrap()};
    let f=Frame::new(pool,seq+1,prev,c.clone()).unwrap();assert!(!unbound.matches(&f));assert!(bound.matches(&f));
    assert!(material::install_decided(&root_parent,&c).is_err());
    assert!(material::install_decided(&root_parent,&Creation{parent:BirthParent::Root,..c.clone()}).is_err());
    assert!(!v.root.entry_present(&std::ffi::CString::new(crate::test_support::physical("report",c.file)).unwrap()).unwrap());assert_eq!(native(&v.root.child_directory(&name).unwrap().unwrap()),st.object);
    assert_eq!(same.record(c"FILE",512).unwrap().unwrap().1,origin);
    assert!(!bound.matches(&Frame::new(pool,seq+1,prev,Creation{parent:other,..c.clone()}).unwrap()));
    assert!(!bound.matches(&Frame::new(PoolId::new().unwrap(),seq+1,prev,c).unwrap()));
    std::fs::rename(wrong,old).unwrap();assert_eq!(native(&h.child_directory(&name).unwrap().unwrap()),st.object);
    root_closed();
    let holder=HeldDir::open(&t.child("unrelated-holder")).unwrap();let(copy,_)=holder.anonymous_record().unwrap();
    fs::initial_attempt_binding(&copy,&real).unwrap();holder.add_record(copy,NAME,&image).unwrap();
    let(stamp,data)=holder.record(NAME,MAX).unwrap().unwrap();assert!(Allocation::from_bytes(&holder,pool,stamp,data).is_err());
    p.system.remove_original_record(NAME,&bound.stamp,&image).unwrap();
    assert_eq!(p.snapshot().unwrap().1.frontier(),(seq,prev)); // no command or parent was published
}
