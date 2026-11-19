//! Original proposal, reservation, physical installation, and public replay
//! are distinct cuts of the same child FILE operation. Native fixtures only.
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


#[test]
#[ignore="two real original filesystems; private unbound child creation, no public child FILE"]
fn one_consumed_proposal_freezes_its_parent_and_file_ids_before_any_data_name(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("ordinary");let bin=r.child("recycle");
    let reg=crate::initialize(op(),&sys,&bin,&[a.clone()]).unwrap();
    let v=reg.volumes.iter().find(|x|x.volume.path==a).unwrap().id;
    let l=crate::make_directory(&sys,op(),Parent::Root(v),"left").unwrap();
    let rr=crate::make_directory(&sys,op(),Parent::Root(v),"right").unwrap();
    let sibling=crate::create_empty(&sys,op(),v,"Report.xlsx").unwrap();let id=op();
    let address=Parent::Child(v,l.directory);let(frontier,file,issuer,image,digest,request)={
        let(p,_,before)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();
        let proposal=Unissued::select(&volume,l.directory,id,"Report.xlsx").unwrap();let obj=proposal.issuer.clone();
        let img=proposal.bytes.clone();let f=proposal.claim.file;let d=proposal.digest;
        let a=proposal.issue().unwrap();assert_eq!(NativeObject::stamp(&a.stamp),obj);assert_eq!(a.bytes,img);assert!(a.stage.is_none());
        (before.frontier(),f,obj,img,d,a.request)
    };
    assert!(crate::create_empty(&sys,id,v,"Report.xlsx").is_err());
    let old=t.0.join("detached-original");std::fs::rename(a.join("left"),&old).unwrap();
    let h=crate::creation_receipt(&sys,id).unwrap().unwrap();assert_eq!(h.status,crate::CreationStatus::Reserved);
    assert_eq!((h.file.parent,h.file.file,h.file.decision_sequence),(address,file,None));
    assert!(crate::list_initial_files(&sys,v).is_err()); // original settled directory is genuinely absent
    assert_eq!(crate::read_initial_file(&sys,sibling.file).unwrap().0,sibling);
    {
        let(p,_,cold)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=p.ordinary(v).unwrap();
        let original=cold.allocated.as_ref().unwrap();assert_eq!(cold.frontier(),frontier);assert!(cold.decided(id).is_none()&&cold.pending.is_none());
        assert_eq!((original.bytes.as_slice(),original.digest,NativeObject::stamp(&original.stamp)),(image.as_slice(),digest,issuer));
        assert_eq!(request,crate::ordinary::file_parent::request(p.id(),id,fs::local_uid(),address,"Report.xlsx").unwrap());
        assert_ne!(request,crate::ordinary::file_parent::request(p.id(),id,fs::local_uid(),Parent::Child(v,rr.directory),"Report.xlsx").unwrap());
        assert!(Unissued::select(&volume,rr.directory,id,"Report.xlsx").is_err());
        assert!(p.system.entry_present(allocation::NAME).unwrap());assert!(!p.system.entry_present(journal::INTENT).unwrap());
    }
    std::fs::rename(old,a.join("left")).unwrap();
    assert!(std::fs::read_dir(a.join("left")).unwrap().next().is_none());assert!(std::fs::read_dir(a.join("right")).unwrap().next().is_none());
    assert_eq!(crate::list_initial_files(&sys,v).unwrap().files,vec![sibling]);
}

#[test]
#[ignore="original SYSTEM and eligible ordinary/recycle filesystems; private nested birth materials"]
fn original_child_ticket_only_binds_a_fresh_reachable_stage_and_reopens_its_own_file_material(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("regular");let bin=r.child("recycle");
    let reg=crate::initialize(op(),&sys,&bin,&[a.clone()]).unwrap();let id=reg.volumes.iter().find(|x|x.volume.path==a).unwrap().id;
    let l=crate::make_directory(&sys,op(),Parent::Root(id),"left").unwrap().directory;
    let rr=crate::make_directory(&sys,op(),Parent::Root(id),"right").unwrap().directory;
    let address=Parent::Child(id,l);let request=op();
    let(file,ticket,digest,frontier)={
        let(p,_,cut)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let vol=p.ordinary(id).unwrap();
        let a=Unissued::select(&vol,l,request,"Report.xlsx").unwrap().issue().unwrap();
        let wrong=HeldParent::open(&vol,Parent::Child(id,rr)).unwrap();
        assert!(a.bind_fresh(&wrong,||panic!("wrote under wrong directory")).is_err());wrong.directory().only(None).unwrap();drop(wrong);
        let root=HeldParent::open(&vol,Parent::Root(id)).unwrap();
        assert!(a.bind_fresh(&root,||panic!("wrote at volume root")).is_err());drop(root);
        let h=HeldParent::open(&vol,address).unwrap();
        assert!(a.bind_fresh(&h,||Err(io::Error::other("fresh mkdir synced; no binding"))).is_err());
        assert!(p.snapshot().unwrap().1.allocated.unwrap().stage.is_none());
        let private=material::staging(a.attempt,0);
        assert!(!vol.root.entry_present(&private).unwrap());let first=h.directory().exact_directory(&private).unwrap().unwrap();first.only(None).unwrap();
        (a.file,a.attempt,a.digest,cut.frontier())
    };
    let stage0=material::staging(ticket,0);let foreign=a.join("left").join(stage0.to_str().unwrap()).join("keep");
    std::fs::write(&foreign,b"unrelated").unwrap();
    {
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let vol=p.ordinary(id).unwrap();
        let h=HeldParent::at_cut(&vol,address,&s).unwrap();let a=s.allocated.as_ref().unwrap();
        assert!(a.bind_fresh(&h,||panic!("crossed nonempty unresolved slot")).is_err());
        assert!(!h.directory().entry_present(&material::staging(ticket,1)).unwrap());assert_eq!(std::fs::read(&foreign).unwrap(),b"unrelated");
        assert!(p.snapshot().unwrap().1.allocated.unwrap().stage.is_none());
    }
    let saved=t.0.join("untouched");std::fs::rename(&foreign,&saved).unwrap();
    let material={
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let vol=p.ordinary(id).unwrap();
        let h=HeldParent::at_cut(&vol,address,&s).unwrap();let a=s.allocated.as_ref().unwrap();assert_eq!((a.file,a.digest),(file,digest));
        a.bind_fresh(&h,||Ok(())).unwrap();let a=p.snapshot().unwrap().1.allocated.unwrap();assert_eq!(a.bound().unwrap().slot,1);
        let c=material::prepare(&h,&a).unwrap();assert_eq!(c.parent,a.parent);
        assert_eq!(c.directory,a.bound().unwrap().object);
        assert!(!vol.root.entry_present(&material::staging(ticket,1)).unwrap());assert!(!p.system.entry_present(journal::INTENT).unwrap());
        let f=crate::ordinary::wire::Frame::new(p.id(),a.seq,a.previous,c.clone()).unwrap();assert!(a.matches(&f));
        assert_eq!(journal::Decision::read(p.id(),&f.bytes).unwrap().creation().unwrap(),&f);
        assert!(p.snapshot().unwrap().1.decided(request).is_none());
        assert!(!p.system.entry_present(journal::INTENT).unwrap()); // parsing alone never seals or decides
        c
    };
    {
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let vol=p.ordinary(id).unwrap();
        let h=HeldParent::at_cut(&vol,address,&s).unwrap();let a=s.allocated.as_ref().unwrap();assert_eq!(s.frontier(),frontier);assert!(s.file(file).is_none());
        assert_eq!(material::prepare(&h,a).unwrap(),material);
        assert!(material::install_decided(&h,&material).is_err());
        assert!(!h.directory().entry_present(&crate::ordinary::storage_name::file_leaf("Report.xlsx",file).unwrap()).unwrap());
        assert!(!vol.root.entry_present(&crate::ordinary::storage_name::file_leaf("Report.xlsx",file).unwrap()).unwrap());
    }
    assert_eq!(std::fs::read(&saved).unwrap(),b"unrelated");
}

#[test]
#[ignore="original native child, real private slot issuance and cold stage reopen; no public child commit"]
fn binding_only_its_last_observed_slot_and_preparing_only_an_unlabelled_original(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let path=t.child("regular");let reg=crate::initialize(op(),&sys,&r.child("recycle"),&[path.clone()]).unwrap();
    let v=reg.volumes.iter().find(|x|x.volume.path==path).unwrap().id;
    let dir=crate::make_directory(&sys,op(),Parent::Root(v),"p").unwrap().directory;let parent=Parent::Child(v,dir);let request=op();
    let(ticket,file)={
        let(pool,_,_s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=pool.ordinary(v).unwrap();
        let a=Unissued::select(&volume,dir,request,"A").unwrap().issue().unwrap();let held=HeldParent::open(&volume,parent).unwrap();
        let later=material::staging(a.attempt,1);assert!(a.bind_fresh(&held,||{
            held.directory().new_child_directory(&later)?.file.sync_all()?;held.directory().file.sync_all()
        }).is_err());
        assert!(pool.snapshot().unwrap().1.allocated.unwrap().stage.is_none());
        for i in [0,1]{held.directory().exact_directory(&material::staging(a.attempt,i)).unwrap().unwrap().only(None).unwrap();}
        // Neither previously observed native directory can be claimed.
        a.bind_fresh(&held,||Ok(())).unwrap();let original=pool.snapshot().unwrap().1.allocated.unwrap();
        assert_eq!(original.bound().unwrap().slot,2);let bound=held.directory().exact_directory(&material::staging(a.attempt,2)).unwrap().unwrap();
        bound.only(None).unwrap();bound.claim(&[]).unwrap();assert_eq!(bound.label().unwrap(),Some(vec![]));
        (a.attempt,a.file)
    };
    let(pool,_,s)=LocalPool::with_snapshot(&sys,Access::Write).unwrap();let volume=pool.ordinary(v).unwrap();
    let held=HeldParent::at_cut(&volume,parent,&s).unwrap();let a=s.allocated.as_ref().unwrap();assert_eq!((a.attempt,a.file),(ticket,file));
    assert!(material::prepare(&held,a).is_err());let b=held.directory().exact_directory(&material::staging(ticket,2)).unwrap().unwrap();
    assert!(b.label().unwrap().is_some());b.only(None).unwrap();assert!(!b.entry_present(c"FILE").unwrap());
    assert!(s.decided(request).is_none());assert!(!pool.system.entry_present(journal::INTENT).unwrap());
}

#[test]
#[ignore="real parent-specific CREATE retry and durable reservation before the original decision"]
fn the_same_child_creation_dispatch_resumes_its_original_and_never_falls_back_to_the_root_or_a_peer(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let path=t.child("regular");let reg=crate::initialize(op(),&sys,&r.child("recycle"),&[path.clone()]).unwrap();
    let v=reg.volumes.iter().find(|x|x.volume.path==path).unwrap().id;let root=Parent::Root(v);
    let left=Parent::Child(v,crate::make_directory(&sys,op(),root,"left").unwrap().directory);
    let right=Parent::Child(v,crate::make_directory(&sys,op(),root,"right").unwrap().directory);let cmd=op();
    crate::create_empty(&sys,op(),v,"book").unwrap();
    assert!(crate::ordinary::create_at_inner(&sys,cmd,left,"book",|s|if s==crate::ordinary::Cut::Reservation{
        Err(io::Error::other("original child reservation is durable"))}else{Ok(())}).is_err());
    let receipt=crate::creation_receipt(&sys,cmd).unwrap().unwrap();assert_eq!(receipt.file.parent,left);
    assert!(crate::ordinary::create_at_inner(&sys,cmd,right,"book",|_|panic!("different parent cannot take original")).is_err());
    assert!(crate::create_empty(&sys,cmd,v,"book").is_err());
    let mut reached=false;assert!(crate::ordinary::create_at_inner(&sys,cmd,left,"book",|c|if c==crate::ordinary::Cut::UnboundMkdir{
        reached=true;Err(io::Error::other("the true original selected its first private directory"))}else{Ok(())}).is_err());
    assert!(reached);let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();
    let a=s.allocated.as_ref().unwrap();assert_eq!((a.op,a.file),(cmd,receipt.file.file));
    let v=p.ordinary(v).unwrap();let l=HeldParent::at_cut(&v,left,&s).unwrap();
    l.directory().exact_directory(&material::staging(a.attempt,0)).unwrap().unwrap().only(None).unwrap();
    HeldParent::at_cut(&v,right,&s).unwrap().directory().only(None).unwrap();
    assert!(s.decided(cmd).is_none()&&s.pending.is_none());assert!(!p.system.entry_present(journal::INTENT).unwrap());
}


#[test]
#[ignore="two original filesystems: cold recovery of real child FILE creation and a replaced parent"]
fn child_birth_never_promotes_a_complete_frame_using_a_replacement_parent(){
    use crate::ordinary::{CreationStatus,Cut};
    use std::os::unix::fs::DirBuilderExt;
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("ordinary");let reg=crate::initialize(op(),&sys,&r.child("recycle"),&[a.clone()]).unwrap();
    let v=reg.volumes.iter().find(|x|x.volume.path==a).unwrap().id;let root=Parent::Root(v);
    let left=Parent::Child(v,crate::make_directory(&sys,op(),root,"left").unwrap().directory);
    let right=Parent::Child(v,crate::make_directory(&sys,op(),root,"right").unwrap().directory);
    let source=a.join("left");let saved=t.0.join("original-left");let outsider=t.0.join("untouched-foreign-dir");let cmd=op();
    let mut changed=false;
    assert!(crate::ordinary::create_at_inner(&sys,cmd,left,"Report.xlsx",|cut|{
        if cut==Cut::FrameSynced{
            std::fs::rename(&source,&saved)?;std::fs::DirBuilder::new().mode(0o700).create(&source)?;
            std::fs::write(source.join("keep"),b"not the original")?;changed=true;
        }Ok(())
    }).is_err());assert!(changed);
    let before=crate::creation_receipt(&sys,cmd).unwrap().unwrap();assert_eq!(before.status,CreationStatus::DecisionAwaitingConfirmation);
    assert_eq!(before.file.parent,left);assert!(crate::read_initial_file(&sys,before.file.file).is_err());
    for p in [root,right,left]{assert!(crate::create_empty_at(&sys,cmd,p,"Report.xlsx").is_err());}
    assert_eq!(crate::creation_receipt(&sys,cmd).unwrap().unwrap(),before);
    assert_eq!(std::fs::read(source.join("keep")).unwrap(),b"not the original");
    std::fs::rename(&source,&outsider).unwrap();std::fs::rename(&saved,&source).unwrap();
    let born=crate::create_empty_at(&sys,cmd,left,"Report.xlsx").unwrap();
    assert_eq!((born.file,born.entry,born.history,born.parent),(before.file.file,before.file.entry,before.file.history,left));
    assert_eq!(crate::read_initial_file(&sys,born.file).unwrap().1,Vec::<u8>::new());
    let offline=t.0.join("unavailable");std::fs::rename(&a,&offline).unwrap();
    assert_eq!(crate::create_empty_at(&sys,cmd,left,"Report.xlsx").unwrap(),born);
    assert!(crate::create_empty_at(&sys,cmd,right,"Report.xlsx").is_err());
    assert_eq!(crate::creation_receipt(&sys,cmd).unwrap().unwrap().status,CreationStatus::Published);
    assert_eq!(std::fs::read(outsider.join("keep")).unwrap(),b"not the original");
}
