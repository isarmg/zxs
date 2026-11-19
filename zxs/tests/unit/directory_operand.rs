use super::*;
use crate::{EntryId,OperationId,VolumeId,identity::DirectoryId,pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use crate::ordinary::directory::types::Basis;
use std::path::Path;

fn fresh()->OperationId{OperationId::new().unwrap()}
fn object(d:&HeldDir)->NativeObject{NativeObject{entity:d.entity,birth:fs::Birth::read(&d.file).unwrap()}}
fn read(holder:&HeldDir,pool:PoolId)->io::Result<Operand>{
    let(stamp,bytes)=holder.record(allocation::NAME,MAX)?.ok_or_else(||invalid("expected fixture record"))?;
    Operand::from_bytes(holder,pool,stamp,bytes)
}
#[test]
#[ignore="two explicit private filesystems with original native handles and stage xattrs"]
fn a_directory_operand_binds_its_issuing_record_then_one_real_child_without_admitting_a_namespace_decision(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let data=t.child("data");let info=crate::initialize(fresh(),&sys,&r.child("recycle"),std::slice::from_ref(&data)).unwrap();
    let id:VolumeId=info.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
    let(p,_,j)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let v=p.ordinary(id).unwrap();
    let parent=Basis::registered(&v).unwrap();let origin=Origin{op:fresh(),uid:fs::local_uid(),parent,
        name:"folder".into(),entry:EntryId::new().unwrap(),id:DirectoryId::new().unwrap(),ticket:fresh()};
    let(seq,previous)=j.frontier();assert_eq!(seq,0);
    // The operands below are real native records in a separate fixture
    // holder. Neither they nor the child directory are installed business data.
    let holder=HeldDir::open(&t.child("fixture-operands")).unwrap();
    let(fd,entity)=holder.anonymous_record().unwrap();let issuer=NativeObject{entity,birth:fs::Birth::read(&fd).unwrap()};
    assert!(Operand::image(p.id(),&issuer,&origin,0,previous).is_err());
    let(bytes,sum)=Operand::image(p.id(),&issuer,&origin,seq+1,previous).unwrap();
    let empty=allocation::binding(sum,None).unwrap();fs::initial_attempt_binding(&fd,&empty).unwrap();
    holder.add_record(fd,allocation::NAME,&bytes).unwrap();
    let a=read(&holder,p.id()).unwrap();assert_eq!(a.bytes,bytes);assert_eq!(a.digest,sum);assert!(a.stage.is_none());
    assert!(read(&holder,PoolId::new().unwrap()).is_err());
    let stage=v.root.new_child_directory(c"unissued-fixture").unwrap();let other=v.root.new_child_directory(c"other-fixture").unwrap();
    let actual=object(&stage);let original=Created::new(p.id(),seq+1,previous,origin.clone(),sum,2,actual.clone()).unwrap();
    assert!(!a.matches(&original));
    let bound=allocation::binding(sum,Some(&Stage{slot:2,object:actual.clone()})).unwrap();
    holder.bind_attempt(allocation::NAME,&a.stamp,MAX,&empty,&bound).unwrap();
    let a=read(&holder,p.id()).unwrap();assert!(a.matches(&original));
    assert!(holder.bind_attempt(allocation::NAME,&a.stamp,MAX,&empty,&bound).is_err());
    let wrong=|o:Origin,i,child|Created::new(p.id(),seq+1,previous,o,sum,i,child).unwrap();
    assert!(!a.matches(&wrong(origin.clone(),3,actual.clone())));
    assert!(!a.matches(&wrong(origin.clone(),2,object(&other))));
    assert!(!a.matches(&wrong(Origin{entry:EntryId::new().unwrap(),..origin.clone()},2,actual)));
    let second=HeldDir::open(&t.child("copied-operands")).unwrap();
    let(copy,_)=second.anonymous_record().unwrap();fs::initial_attempt_binding(&copy,&bound).unwrap();
    second.add_record(copy,allocation::NAME,&bytes).unwrap();
    assert!(read(&second,p.id()).is_err()); // identical bytes + binding, different birth
    assert!(p.system.record(allocation::NAME,MAX).unwrap().is_none());assert_eq!(p.snapshot().unwrap().1.frontier(),(seq,previous));
}
