use super::*;
use crate::{pool::lease::LocalPool,test_support::PrivateTemp,ordinary::directory::types::{Parent,request}};
use std::{io,path::Path};
fn op()->OperationId{OperationId::new().unwrap()}
#[test]
#[ignore="original native SYSTEM and separate recycle filesystem; no directory decision or public API"]
fn real_directory_reservation_precedes_first_mkdir_and_only_a_newly_created_slot_can_bind(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let root_dir=t.child("one");let other=t.child("two");
    let pool=crate::initialize(op(),&sys,&r.child("recycle"),&[root_dir.clone(),other.clone()]).unwrap();
    let id=|path:&Path|pool.volumes.iter().find(|row|row.volume.path==path).unwrap().id;
    let v=id(&root_dir);let address=Parent::Root(v);let original=op();let unrelated=op();let file=crate::create_empty(&sys,op(),v,"file").unwrap();
    let req=request(pool.id,original,fs::local_uid(),address,"outer").unwrap();
    let(ticket,first,old_frontier)={
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();
        let parent=HeldParent::open(&volume,address).unwrap();issue(&parent,original,"outer").unwrap();
        let(_,now)=p.snapshot().unwrap();let a=now.making.unwrap();assert_eq!(a.origin.op,original);assert!(a.stage.is_none());
        assert_eq!(parent.directory().names().unwrap(),[b"file".to_vec()]);
        assert!(stage(&parent,original,req,|c|if c==Cut::UnboundMkdir{Err(io::Error::other("unbound primary mkdir"))}else{Ok(())}).is_err());
        let zero=parent.directory().child_directory(&material::staging(a.origin.ticket,0)).unwrap().unwrap();
        (a.origin.ticket,NativeObject{entity:zero.entity,birth:fs::Birth::read(&zero.file).unwrap()},s.frontier())
    };
    assert_eq!(crate::read_initial_file(&sys,file.file).unwrap().0.file,file.file);
    let listed=crate::list_initial_files(&sys,v).unwrap();assert_eq!(listed.files[0].file,file.file);assert_eq!(listed.reserved_private_entries,1);
    assert!(crate::create_empty(&sys,unrelated,id(&other),"blocked").is_err());assert!(crate::creation_receipt(&sys,unrelated).unwrap().is_none());
    {let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();let parent=HeldParent::open(&volume,address).unwrap();
        assert_eq!(s.frontier(),old_frontier);assert!(s.pending.is_none()&&s.making.as_ref().unwrap().stage.is_none());
        assert!(stage(&parent,original,[0;32],|_|Ok(())).is_err());
        let bound=stage(&parent,original,req,|_|Ok(())).unwrap();let own=bound.stage.as_ref().unwrap();
        assert_eq!(own.slot,1);assert_ne!(own.object,first);assert!(!parent.directory().entry_present(c"outer").unwrap());
        let zero=parent.directory().child_directory(&material::staging(ticket,0)).unwrap().unwrap();same_child(&zero,&first).unwrap();
        let one=material::staging(ticket,1);let src=root_dir.join(one.to_str().unwrap());let parked=t.0.join("parked");
        std::fs::rename(&src,&parked).unwrap();let replacement=parent.directory().new_child_directory(&one).unwrap();
        assert!(stage(&parent,original,req,|_|Ok(())).is_err());assert_ne!(replacement.entity,own.object.entity);
        assert!(p.snapshot().unwrap().1.pending.is_none());assert!(crate::fs::Birth::read(&replacement.file).is_ok());
        std::fs::rename(&src,t.0.join("foreign")).unwrap();std::fs::rename(parked,&src).unwrap();
        assert_eq!(stage(&parent,original,req,|_|Ok(())).unwrap().stage.as_ref(),Some(own));
        assert_eq!(p.snapshot().unwrap().1.frontier(),old_frontier);
    }
}

#[test]
#[ignore="two original native filesystems; genuine FILE residue is counted but never adopted or overwritten"]
fn new_directory_does_not_issue_an_original_ticket_over_a_nonempty_lower_file_slot(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let root=t.child("ordinary");let reg=crate::initialize(op(),&sys,&r.child("recycle"),std::slice::from_ref(&root)).unwrap();
    let v=reg.volumes.iter().find(|x|x.volume.path==root).unwrap().id;let create=op();let mkdir=op();
    assert!(crate::ordinary::create_inner(&sys,create,v,"first",|c|if c==Cut::UnboundMkdir{Err(io::Error::other("unbound FILE private slot"))}else{Ok(())}).is_err());
    let f=crate::create_empty(&sys,create,v,"first").unwrap();
    let (private,prior)={let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let c=&s.file(f.file).unwrap().create;
        assert_eq!(c.slot,1);let name=material::staging(c.attempt,0);
        let h=p.ordinary(v).unwrap().root.child_directory(&name).unwrap().unwrap();(h.path,s.frontier())};
    let foreign=private.join("untouched");std::fs::write(&foreign,b"not an original mkdir operand").unwrap();
    {let(p,_,_)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();let parent=HeldParent::open(&volume,Parent::Root(v)).unwrap();
        assert!(issue(&parent,mkdir,"new").is_err());let(_,s)=p.snapshot().unwrap();
        assert!(s.making.is_none()&&s.pending.is_none()&&s.decided(mkdir).is_none());assert_eq!(s.frontier(),prior);
        assert!(!p.system.entry_present(allocation::NAME).unwrap());assert!(!volume.root.entry_present(c"new").unwrap());}
    let old_list=crate::list_initial_files(&sys,v).unwrap();assert_eq!(old_list.files[0].file,f.file);
    assert_eq!(old_list.reserved_private_entries,1);assert_eq!(std::fs::read(&foreign).unwrap(),b"not an original mkdir operand");
    std::fs::rename(foreign,t.0.join("preserved")).unwrap();
    let(p,_,_)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();let parent=HeldParent::open(&volume,Parent::Root(v)).unwrap();
    issue(&parent,mkdir,"new").unwrap();assert_eq!(p.snapshot().unwrap().1.making.unwrap().origin.op,mkdir);
    assert!(private.is_dir());assert_eq!(std::fs::read(t.0.join("preserved")).unwrap(),b"not an original mkdir operand");
}
