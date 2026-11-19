//! Real opt-in native fixture sources. No mounting, device administration or
//! production roots. Each Library call reopens the original SYSTEM.
use super::*;
use crate::{pool::lease::LocalPool,ordinary::{allocation,material},test_support::PrivateTemp,VolumeId};
use std::{fs as host,os::unix::{ffi::OsStrExt,fs::DirBuilderExt},path::PathBuf};
fn op()->OperationId{OperationId::new().unwrap()}
struct F{_r:PrivateTemp,t:PrivateTemp,sys:PathBuf,a:PathBuf,aid:VolumeId,bid:VolumeId}
impl F{
    fn new()->Self{
        let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        let sys=t.child("system");let a=t.child("a");let b=t.child("b");
        let info=crate::initialize(op(),&sys,&r.child("recycle"),&[a.clone(),b.clone()]).unwrap();
        let id=|path:&Path|info.volumes.iter().find(|v|v.volume.path==path).unwrap().id;
        let(aid,bid)=(id(&a),id(&b));Self{_r:r,t,sys,a,aid,bid}
    }
    fn top(&self)->Parent{Parent::Root(self.aid)}
}
#[test]
#[ignore="registered native filesystems; cold retry at every original mkdir durability boundary"]
fn each_real_cut_preserves_one_parent_one_directory_and_the_historical_receipt(){
    let cuts=[Cut::Reservation,Cut::UnboundMkdir,Cut::DirectoryBoundBeforeSync,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,
        Cut::MarkedBeforeSync,Cut::Decision,Cut::DirectoryRenamed,Cut::Installation,Cut::IntentReleased,Cut::Release];
    for stop in cuts{
        let f=F::new();let op1=op();let sibling=crate::create_empty(&f.sys,op(),f.aid,"sibling").unwrap();
        assert!(make(&f.sys,op1,f.top(),"outer",|c|if c==stop{Err(io::Error::other("interrupted original mkdir"))}else{Ok(())}).is_err());
        let(id,confirmed,pending)={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let decided=s.decided(op1).map(|d|d.directory().unwrap());
            (s.making.as_ref().map(|a|a.origin.id).or_else(||decided.map(|d|d.origin.id)).unwrap(),decided.is_some(),s.making.is_some())};
        if stop==Cut::FrameSynced{assert!(!confirmed);}
        assert_eq!(crate::read_initial_file(&f.sys,sibling.file).unwrap().0.file,sibling.file);
        if pending{
            let attempted=op();assert!(make_directory(&f.sys,attempted,Parent::Child(f.aid,id),"early").is_err());
            assert!(list_directory(&f.sys,Parent::Child(f.aid,id)).is_err());
            let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert!(s.decided(attempted).is_none());
            if confirmed{assert!(list_directory(&f.sys,f.top()).is_err());}else{
                assert_eq!(list_directory(&f.sys,f.top()).unwrap().entries,[DirectoryEntry::File(sibling.clone())]);}
        }
        let outer=make_directory(&f.sys,op1,f.top(),"outer").unwrap();assert_eq!(id,outer.directory);
        let child=make_directory(&f.sys,op(),Parent::Child(f.aid,id),"inner").unwrap();
        assert_eq!(list_directory(&f.sys,Parent::Child(f.aid,id)).unwrap().entries,[DirectoryEntry::Directory(child.clone())]);
        assert!(list_directory(&f.sys,Parent::Child(f.aid,child.directory)).unwrap().entries.is_empty());
        assert_eq!(make_directory(&f.sys,op1,f.top(),"outer").unwrap(),outer); // no requirement that it still be empty
        assert!(make_directory(&f.sys,op1,f.top(),"other").is_err());
        assert_eq!(host::read_dir(f.a.join("outer")).unwrap().count(),1); // no per-directory business metadata
        host::rename(&f.a,f.t.0.join("offline")).unwrap();
        assert_eq!(make_directory(&f.sys,op1,f.top(),"outer").unwrap(),outer);
        assert!(list_directory(&f.sys,f.top()).is_err());assert!(list_directory(&f.sys,Parent::Root(f.bid)).unwrap().entries.is_empty());
    }
}
#[test]
#[ignore="native original held through a late sibling and the rename-to-directory-sync gap"]
fn hot_and_cold_confirmation_refuse_foreign_final_names_and_postdecision_never_adopts_a_nonempty_target(){
    let f=F::new();let op1=op();let init={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();s.frontier()};
    let intrusion=f.a.join("outer");let parked=f.t.0.join("foreign");let mut once=false;
    assert!(make(&f.sys,op1,f.top(),"outer",|c|{if c==Cut::FrameSynced&&!once{once=true;
        host::DirBuilder::new().mode(0o700).create(&intrusion)?;host::write(intrusion.join("sentinel"),b"outsider")?;}Ok(())}).is_err());
    assert!(once);{let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert_eq!(s.frontier(),init);
        assert!(s.decided(op1).is_none()&&s.pending.is_some()&&s.tail>0);}
    assert!(make_directory(&f.sys,op1,f.top(),"outer").is_err());assert_eq!(host::read(intrusion.join("sentinel")).unwrap(),b"outsider");
    host::rename(&intrusion,&parked).unwrap();let mut installed=false;
    assert!(make(&f.sys,op1,f.top(),"outer",|c|{if c==Cut::DirectoryRenamed{installed=true;
        host::write(intrusion.join("late"),b"postdecision")?;return Err(io::Error::other("before parent sync"));}Ok(())}).is_err());
    assert!(installed);let original={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();s.decided(op1).unwrap().directory().unwrap().clone()};
    assert!(make_directory(&f.sys,op1,f.top(),"outer").is_err());assert!(list_directory(&f.sys,Parent::Child(f.aid,original.origin.id)).is_err());
    assert_eq!(host::read(intrusion.join("late")).unwrap(),b"postdecision");host::rename(intrusion.join("late"),f.t.0.join("preserved")).unwrap();
    assert_eq!(make_directory(&f.sys,op1,f.top(),"outer").unwrap(),DirectoryInfo::original(&original));
    assert_eq!(host::read(parked.join("sentinel")).unwrap(),b"outsider");
}
#[test]
#[ignore="native roots exactly at byte/depth boundaries, original loop is not executed by default"]
fn actual_public_mkdir_rejects_oversize_issued_paths_before_any_new_frame_or_reservation(){
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let sibling=t.child("short");
    let width=(0..allocation::SLOTS).map(|n|material::staging(op(),n).to_bytes().len()).max().unwrap();
    let exact=crate::test_support::native_root_length(&t.child("exact"),4096-1-width);
    let too_long=crate::test_support::native_root_length(&t.child("long"),4096-width);let mut depth=t.child("deep");
    while depth.components().count()-1<fs::MAX_PATH_DEPTH-1{depth.push("d");host::DirBuilder::new().mode(0o700).create(&depth).unwrap();}
    let config=crate::initialize(op(),&sys,&r.child("recycle"),&[sibling.clone(),exact.clone(),too_long.clone(),depth.clone()]).unwrap();
    let vid=|path:&Path|config.volumes.iter().find(|v|v.volume.path==path).unwrap().id;
    let guard=|address:Parent,name:&str,actual:&Path|{
        let before={let(_,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();s.frontier()};let failed=op();
        assert!(make_directory(&sys,failed,address,name).is_err());
        let(p,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();assert_eq!(before,s.frontier());
        assert!(s.decided(failed).is_none()&&s.making.is_none()&&s.pending.is_none());
        assert!(!p.system.entry_present(allocation::NAME).unwrap()&&!p.system.entry_present(journal::INTENT).unwrap());
        assert!(fs::HeldDir::open(actual).unwrap().names().unwrap().is_empty());
    };
    assert!(fs::check_path(&too_long.join("x")).is_ok());guard(Parent::Root(vid(&too_long)),"x",&too_long);
    let x=make_directory(&sys,op(),Parent::Root(vid(&exact)),"x").unwrap();assert_eq!(x.name,"x");
    {let(_,_,s)=LocalPool::with_snapshot(&sys,Access::Read).unwrap();let d=s.decided(x.created_by).unwrap().directory().unwrap();
        assert_eq!(exact.as_os_str().as_bytes().len()+1+material::staging(d.origin.ticket,d.slot).to_bytes().len(),4096);}
    let last=make_directory(&sys,op(),Parent::Root(vid(&depth)),"x").unwrap();
    assert_eq!(depth.join("x").components().count()-1,256);guard(Parent::Child(vid(&depth),last.directory),"y",&depth.join("x"));
    let peer=crate::create_empty(&sys,op(),vid(&sibling),"sibling").unwrap();assert_eq!(crate::list_initial_files(&sys,vid(&sibling)).unwrap().files,[peer]);
}
#[test]
#[ignore="settled directory private ticket cannot be consumed by a new FILE request"]
fn a_private_directory_ticket_cannot_start_file_create_save_or_another_mkdir(){
    let f=F::new();let create=op();let row=make_directory(&f.sys,create,f.top(),"folder").unwrap();
    let file=crate::create_empty(&f.sys,op(),f.aid,"report").unwrap();let(ticket,prior)={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
        (s.decided(create).unwrap().directory().unwrap().origin.ticket,s.frontier())};
    assert!(crate::create_empty(&f.sys,ticket,f.bid,"new").is_err());
    let claim=crate::DraftClaim::from_hex(3,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
    assert!(crate::save_first(&f.sys,ticket,file.file,claim,std::io::Cursor::new(b"abc")).is_err());
    assert!(make_directory(&f.sys,ticket,f.top(),"new").is_err());assert_eq!(make_directory(&f.sys,create,f.top(),"folder").unwrap(),row);
    let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert_eq!(s.frontier(),prior);
    assert!(s.making.is_none()&&s.saving.is_none()&&s.allocated.is_none()&&s.pending.is_none());
}

#[test]
#[ignore="real registered directory, replaced ancestor and same-operation re-entry on the original"]
fn same_spelled_substitute_cannot_supply_the_original_parent_for_public_list_or_mkdir(){
    let f=F::new();let parent_op=op();let outer=make_directory(&f.sys,parent_op,f.top(),"outer").unwrap();
    let address=Parent::Child(f.aid,outer.directory);
    let inner=make_directory(&f.sys,op(),address,"inner").unwrap();let deep=Parent::Child(f.aid,inner.directory);
    let issued=op();let before={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();s.frontier()};
    let old_name=f.a.join("outer");let parked=f.t.0.join("original-outer");host::rename(&old_name,&parked).unwrap();
    host::DirBuilder::new().mode(0o700).create(&old_name).unwrap();
    host::DirBuilder::new().mode(0o700).create(old_name.join("inner")).unwrap();
    host::write(old_name.join("inner/sentinel"),b"not the original parent").unwrap();
    assert!(list_directory(&f.sys,address).is_err());assert!(list_directory(&f.sys,deep).is_err());
    assert!(make_directory(&f.sys,issued,deep,"child").is_err());
    assert_eq!(make_directory(&f.sys,parent_op,f.top(),"outer").unwrap(),outer);
    {let(p,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();assert_eq!(s.frontier(),before);
        assert!(s.decided(issued).is_none()&&s.pending.is_none()&&s.making.is_none());
        assert!(!p.system.entry_present(allocation::NAME).unwrap()&&!p.system.entry_present(journal::INTENT).unwrap());}
    assert!(host::read_dir(parked.join("inner")).unwrap().next().is_none());
    let stranger=f.t.0.join("stranger");host::rename(&old_name,&stranger).unwrap();host::rename(&parked,&old_name).unwrap();
    let born=make_directory(&f.sys,issued,deep,"child").unwrap();
    assert_eq!(list_directory(&f.sys,address).unwrap().entries,[DirectoryEntry::Directory(inner)]);
    assert_eq!(list_directory(&f.sys,deep).unwrap().entries,[DirectoryEntry::Directory(born.clone())]);
    assert_eq!(make_directory(&f.sys,issued,deep,"child").unwrap(),born);
    assert_eq!(host::read(stranger.join("inner/sentinel")).unwrap(),b"not the original parent");
    assert_eq!(host::read_dir(stranger.join("inner")).unwrap().count(),1);
}
