use super::*;
use crate::{Role,initialize,ordinary::journal::Decision,pool::lease::{Access,LocalPool},test_support::PrivateTemp};
use std::path::Path;
fn id()->OperationId{OperationId::new().unwrap()}
fn native(d:&fs::HeldDir)->NativeObject{NativeObject{entity:d.entity,birth:fs::Birth::read(&d.file).unwrap()}}
fn origin(p:Basis,name:&str)->Origin{Origin{op:id(),uid:fs::local_uid(),parent:p,name:name.into(),entry:EntryId::new().unwrap(),id:DirectoryId::new().unwrap(),ticket:id()}}

#[test]
#[ignore="real ordinary roots and separate registered recycle filesystem"]
fn directory_frame_decodes_against_the_exact_root_or_prior_directory_without_proving_installation(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let s=t.child("system");let a=t.child("ordinary");let b=t.child("another");let recycle=r.child("recycle");
    let info=initialize(id(),&s,&recycle,&[a.clone(),b.clone()]).unwrap();
    let aid=info.volumes.iter().find(|v|v.volume.path==a).unwrap().id;let bid=info.volumes.iter().find(|v|v.volume.path==b).unwrap().id;
    let rid=info.volumes.iter().find(|v|v.volume.role==Role::Recycle).unwrap().id;
    let(p,_,j)=LocalPool::with_snapshot(&s,Access::Write).unwrap();assert!(p.ordinary(rid).is_err());
    let(v,w)=(p.ordinary(aid).unwrap(),p.ordinary(bid).unwrap());let root=Basis::registered(&v).unwrap();
    let reg=p.journal_registration();let(seq,prev)=j.frontier();assert_eq!(seq,0);root.require(&reg,fs::local_uid(),None).unwrap();
    let mut wrong=root.clone();wrong.address=Parent::Root(bid);assert!(wrong.require(&reg,fs::local_uid(),None).is_err());
    wrong=root.clone();wrong.anchor=[17;32];assert!(wrong.require(&reg,fs::local_uid(),None).is_err());
    // Fixture-only native originals. No real business allocation, decision
    // or installation has been issued for any of these private directories.
    let plain=v.root.new_child_directory(c"fixture-parent").unwrap();
    let top=Created::new(p.id(),1,prev,origin(root.clone(),"parent"),[9;32],0,native(&plain)).unwrap();
    top.relation(&reg,None).unwrap();assert_eq!(Created::read(p.id(),&top.bytes).unwrap(),top);
    let physical=plain.new_child_directory(c"fixture-child").unwrap();
    let parent=Basis{address:Parent::Child(aid,top.origin.id),anchor:top.digest,object:top.object.clone()};
    let sub=Created::new(p.id(),2,top.digest,origin(parent.clone(),"child"),[10;32],1,native(&physical)).unwrap();
    sub.relation(&reg,Some(&top)).unwrap();assert!(sub.relation(&reg,None).is_err());
    let alias_root=Created::new(p.id(),1,prev,origin(root.clone(),"wrong"),[11;32],0,native(&w.root)).unwrap();
    assert!(alias_root.relation(&reg,None).is_err());
    for altered in [Basis{address:Parent::Child(bid,top.origin.id),..parent.clone()},
        Basis{anchor:[12;32],..parent.clone()},Basis{address:Parent::Child(aid,DirectoryId::new().unwrap()),..parent.clone()},
        Basis{object:native(&physical),..parent.clone()}]{
        let o=origin(altered,"child");let child=Created::new(p.id(),2,top.digest,o,[11;32],0,native(&physical));
        assert!(child.is_err()||child.unwrap().relation(&reg,Some(&top)).is_err());
    }
    // Real different pool, deliberately reusing A's known parent identity.
    // Parsing under B does not make an A-root frame joinable to A's register.
    let foreign=initialize(id(),&t.child("other-system"),&r.child("other-recycle"),&[t.child("other-file")]).unwrap().id;
    let other_top=Created::new(foreign,1,prev,top.origin.clone(),[9;32],0,native(&plain)).unwrap();
    assert!(other_top.relation(&reg,None).is_err());assert!(Created::read(p.id(),&other_top.bytes).is_err());
    let wrong_pool_child=Created::new(foreign,2,top.digest,origin(parent.clone(),"child"),[11;32],0,native(&physical)).unwrap();
    assert!(wrong_pool_child.relation(&reg,Some(&top)).is_err());
    let a_child_of_foreign=Created::new(p.id(),2,other_top.digest,origin(Basis{anchor:other_top.digest,..parent.clone()},"child"),
        [11;32],0,native(&physical)).unwrap();assert!(a_child_of_foreign.relation(&reg,Some(&other_top)).is_err());
    assert!(a_child_of_foreign.origin.parent.require(&reg,fs::local_uid(),Some(&other_top)).is_err());
    let too_soon=Created::new(p.id(),1,prev,origin(parent,"child"),[11;32],0,native(&physical)).unwrap();
    assert!(too_soon.relation(&reg,Some(&top)).is_err());
    assert!(Created::new(p.id(),1,prev,origin(root.clone(),"self"),[9;32],0,root.object.clone()).is_err());
    assert!(Created::new(p.id(),1,prev,origin(root,"slot"),[9;32],SLOTS,native(&plain)).is_err());
    let e=wire::envelope(p.id(),&sub.bytes).unwrap();let mut body=e.body.to_vec();body.push(0);
    for(kind,req,data)in [(KIND,e.request,body.as_slice()),(KIND,[0;32],e.body),(KIND+1,e.request,e.body)]{
        let(raw,_)=wire::encode(p.id(),e.seq,e.previous,kind,req,data).unwrap();
        assert!(Created::read(p.id(),&raw).is_err());
    }
    assert_eq!(Decision::read(p.id(),&top.bytes).unwrap().directory().unwrap(),&top);assert!(p.snapshot().unwrap().1.frontier()==(seq,prev));
}
