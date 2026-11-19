//! A prospective nested FILE is not a creation, a named reservation, or an
//! installed container. All operations below are fixture-only.
use super::*;
use crate::{ordinary::{allocation,journal},test_support::{PrivateTemp,native_root_length},pool::lease::LocalPool};
use std::{ffi::CString,os::unix::ffi::OsStrExt,path::Path};

fn op()->OperationId{OperationId::new().unwrap()}
fn untouched(p:&LocalPool,frontier:(u64,[u8;32]),id:OperationId){
    let(_,s)=p.snapshot().unwrap();assert_eq!(s.frontier(),frontier);assert!(s.decided(id).is_none());
    assert!(s.allocated.is_none()&&s.making.is_none()&&s.pending.is_none());
    assert!(!p.system.entry_present(allocation::NAME).unwrap());assert!(!p.system.entry_present(journal::INTENT).unwrap());
}
#[test]
#[ignore="two original filesystems; private anonymous proposal only, no child FILE issuance"]
fn child_birth_selection_follows_its_held_real_lineage_not_an_equal_name_or_an_old_ticket(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("a");let b=t.child("b");let bin=r.child("recycle");
    let reg=crate::initialize(op(),&sys,&bin,&[a.clone(),b.clone()]).unwrap();
    let vol=|path:&Path|reg.volumes.iter().find(|v|v.volume.path==path).unwrap().id;let v=vol(&a);
    let l=crate::make_directory(&sys,op(),Parent::Root(v),"left").unwrap();
    let rr=crate::make_directory(&sys,op(),Parent::Root(v),"right").unwrap();
    let occupied=crate::make_directory(&sys,op(),Parent::Child(v,l.directory),"occupied").unwrap();
    let root_file=crate::create_empty(&sys,op(),v,"sibling").unwrap();
    let ri=crate::make_directory(&sys,op(),Parent::Root(vol(&b)),"left").unwrap();
    let other_bin=bin.with_file_name("offline");std::fs::rename(&bin,&other_bin).unwrap();
    let request=op();
    {
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();
        let f=s.frontier();let ticket=s.file(root_file.file).unwrap().create.attempt;
        assert!(Unissued::select(&volume,ri.directory,request,"report").is_err());
        assert!(Unissued::select(&volume,l.directory,ticket,"report").is_err());
        assert!(Unissued::select(&volume,l.directory,request,"occupied").is_err());
        assert!(Unissued::select(&volume,l.directory,root_file.created_by,"available").is_err());
        // The same leaf under the other parent is genuinely unoccupied.
        assert!(Unissued::select(&volume,rr.directory,request,"occupied").is_ok());
        let p1=Unissued::select(&volume,rr.directory,request,"report").unwrap();
        let first=p1.claim.terms(&p,v);let r1=crate::ordinary::file_parent::request(p.id(),request,first.uid,
            first.parent.address(v),first.name).unwrap();drop(p1);
        let p2=Unissued::select(&volume,l.directory,request,"report").unwrap();p2.require().unwrap();
        let first=p2.claim.terms(&p,v);
        assert_ne!(r1,crate::ordinary::file_parent::request(p.id(),request,first.uid,first.parent.address(v),first.name).unwrap());
        let orig_bytes=p2.bytes.clone();let id=p2.claim.file;
        let parked=a.with_file_name("original-left");let human=a.join("left");
        std::fs::rename(&human,&parked).unwrap();let foreign=volume.root.new_child_directory(c"left").unwrap();
        assert!(p2.require().is_err());assert_eq!(p2.bytes,orig_bytes);
        assert!(!foreign.entry_present(c"report").unwrap());untouched(&p,f,request);
        let replacement=a.with_file_name("foreign-left");std::fs::rename(&human,&replacement).unwrap();std::fs::rename(parked,&human).unwrap();
        p2.require().unwrap();assert_eq!(p2.claim.file,id);assert_eq!(p2.bytes,orig_bytes);
        assert!(!p2.parent.directory().entry_present(c"report").unwrap());assert!(replacement.is_dir());untouched(&p,f,request);
        assert_eq!(p2.inode.metadata().unwrap().nlink(),0);
    }
    {let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let volume=p.ordinary(v).unwrap();
        assert!(Unissued::select(&volume,l.directory,request,"report").is_err());untouched(&p,s.frontier(),request);}
    assert_eq!(crate::list_directory(&sys,Parent::Child(v,l.directory)).unwrap().entries,[crate::DirectoryEntry::Directory(occupied)]);
    let other_op=op();
    assert!(crate::ordinary::create_inner(&sys,other_op,vol(&b),"independent",|k|if k==crate::ordinary::Cut::Reservation{
        Err(io::Error::other("hold the genuine earlier FILE reservation"))}else{Ok(())}).is_err());
    {let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();
        let prior=s.allocated.unwrap();assert_eq!(prior.op,other_op);assert!(prior.stage.is_none());
        assert!(Unissued::select(&volume,l.directory,request,"report").is_err());
        let later=p.snapshot().unwrap().1;assert!(later.decided(request).is_none());assert_eq!(later.allocated.as_ref(),Some(&prior));}
    assert!(crate::create_empty(&sys,other_op,vol(&b),"independent").is_ok());
    assert!(crate::creation_receipt(&sys,request).unwrap().is_none());std::fs::rename(other_bin,bin).unwrap();
}

#[test]
#[ignore="genuine held ordinary child parents and exact 4049/4050 byte boundary, no publication"]
fn child_residence_must_fit_the_future_isolation_leaf_even_when_every_creation_slot_fits(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let short=t.child("short");let long=native_root_length(&t.child("long-path"),4000);
    let reg=crate::initialize(op(),&sys,&r.child("recycle"),&[short.clone(),long.clone()]).unwrap();
    let vol=|path:&Path|reg.volumes.iter().find(|v|v.volume.path==path).unwrap().id;let id=vol(&long);
    let n49="e".repeat(48);let n50="f".repeat(49);
    let ok=crate::make_directory(&sys,op(),Parent::Root(id),&n49).unwrap();
    let no=crate::make_directory(&sys,op(),Parent::Root(id),&n50).unwrap();
    let proposed=op();
    {let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let v=p.ordinary(id).unwrap();
        let before=s.frontier();let ticket=op();let q=crate::ordinary::custody::transfer::isolation_name(ticket);assert_eq!(q.to_bytes().len(),46);
        for(n,expected)in[(&n49,4049usize),(&n50,4050usize)]{
            let leaf=CString::new(n.as_bytes()).unwrap();let h=v.root.child_directory(&leaf).unwrap().unwrap();
            assert_eq!(h.path.as_os_str().as_bytes().len(),expected);
            material::check_ticket_paths(&h,"x",ticket).unwrap();
            assert_eq!(expected+1+q.to_bytes().len(),if expected==4049{4096}else{4097});
            assert_eq!(material::held_residence(&v,&h,"x",crate::FileId([17;16]),ticket).is_ok(),expected==4049);
            h.only(None).unwrap();
        }
        assert!(Unissued::select(&v,no.directory,proposed,"x").is_err());untouched(&p,before,proposed);
        let admitted=Unissued::select(&v,ok.directory,proposed,"x").unwrap();admitted.require().unwrap();
        admitted.parent.directory().only(None).unwrap();untouched(&p,before,proposed);
    }
    assert!(crate::create_empty(&sys,op(),vol(&short),"unblocked").is_ok());
}
