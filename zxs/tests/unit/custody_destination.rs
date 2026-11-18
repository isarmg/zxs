use super::*;
use super::super::source::Clean;
use crate::{DraftClaim,LengthPolicy,OperationId,Role,SpanEdit,commit_working,create_empty,initialize,save_first,save_interval,test_support::PrivateTemp};
use std::{fs,io::Cursor,os::unix::fs::MetadataExt,path::PathBuf};
struct Pair{_a:PrivateTemp,_b:PrivateTemp,_r:PrivateTemp,system:PathBuf,b:PathBuf,src:Verified,dest:VolumeId,recycle:VolumeId}
fn make(separate:bool)->Pair{
    let a=PrivateTemp::under(&std::env::temp_dir());let system=a.child("system");let old=a.child("owner");
    let b=if separate{PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()))}
        else{PrivateTemp::under(&std::env::temp_dir())};let new=b.child("target");
    let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let recycle=r.child("recycle");
    let reg=initialize(OperationId::new().unwrap(),&system,&recycle,&[old.clone(),new.clone()]).unwrap();
    let one=reg.volumes.iter().find(|v|v.volume.path==old).unwrap().id;let dest=reg.volumes.iter().find(|v|v.volume.path==new).unwrap().id;
    let recycle=reg.volumes.iter().find(|v|v.volume.role==Role::Recycle).unwrap().id;
    let f=create_empty(&system,OperationId::new().unwrap(),one,"name").unwrap();let data=b"original logical history";
    let claim=|b:&[u8]|DraftClaim::new(b.len() as u64,crate::hash::digest(b).unwrap()).unwrap();
    save_first(&system,OperationId::new().unwrap(),f.file,claim(data),Cursor::new(data)).unwrap();commit_working(&system,OperationId::new().unwrap(),f.file,1).unwrap();
    save_interval(&system,OperationId::new().unwrap(),f.file,1,SpanEdit::new(0,claim(b"X"),LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"X")).unwrap();
    commit_working(&system,OperationId::new().unwrap(),f.file,2).unwrap();
    let src=Clean::capture(&system,f.file).unwrap().verify().unwrap();
    Pair{_a:a,_b:b,_r:r,system,b:new,src,dest,recycle}
}
#[test]
#[ignore="two private original filesystems"]
fn distinct_ordinary_roots_on_one_held_mount_require_true_rename_no_duplicated_bytes(){
    let t=make(false);let before=fs::read(t.system.join("REGISTRATION")).unwrap();
    let dest=Destination::select(&t.system,&t.src,t.dest).unwrap();assert_eq!(dest.mode,Method::Rename);
    let done=dest.prepare().unwrap();assert!(done.files.is_empty());assert_eq!(fs::read_dir(&t.b).unwrap().count(),0);
    assert_eq!(fs::read(t.system.join("REGISTRATION")).unwrap(),before);
    assert!(Destination::select(&t.system,&t.src,t.src.volume()).is_err());assert!(Destination::select(&t.system,&t.src,t.recycle).is_err());
    fs::create_dir(t.b.join("name")).unwrap();assert!(Destination::select(&t.system,&t.src,t.dest).is_err());assert!(t.b.join("name").is_dir());
}
#[test]
#[ignore="requires distinct real source, destination and recycle filesystems; no simulated EXDEV"]
fn actual_cross_filesystem_target_is_complete_preallocated_unnamed_independent_and_not_published(){
    let t=make(true);let before=fs::read(t.system.join("REGISTRATION")).unwrap();
    let d=Destination::select(&t.system,&t.src,t.dest).unwrap();assert_eq!(d.mode,Method::Transfer);
    assert_ne!(d.root.birth.filesystem(),t.src.source_root().birth.filesystem());
    let prepared=d.prepare().unwrap();assert_eq!(prepared.files.len(),t.src.objects().len());
    for original in t.src.objects(){
        let copy=prepared.part(original.kind).unwrap();let m=copy.file.metadata().unwrap();
        assert_eq!(m.nlink(),0);assert_eq!(m.len(),original.len);
        assert_ne!(original.original.birth.filesystem(),copy.stamp.birth.filesystem());assert_eq!(copy.stamp.birth.filesystem(),prepared.dest.root.birth.filesystem());
        assert_eq!(hash_unnamed(&copy.file,&copy.stamp).unwrap(),original.digest);
    }
    prepared.at_vacant_cut(Access::Read,|_,s,old,new|{
        assert_eq!(old.id(),t.src.volume());assert_eq!(new.id(),t.dest);assert!(s.named(t.dest,t.src.name()).is_none());Ok(())
    }).unwrap();
    assert_eq!(fs::read_dir(&t.b).unwrap().count(),0);assert_eq!(fs::read(t.system.join("REGISTRATION")).unwrap(),before);
    drop(prepared);assert_eq!(fs::read_dir(&t.b).unwrap().count(),0);
}

#[test]
#[ignore="two private registered native filesystems"]
fn a_target_chosen_for_a_cannot_exchange_its_held_file_for_b_or_accept_a_new_name_occupant(){
    let t=make(false);let other=create_empty(&t.system,OperationId::new().unwrap(),t.src.volume(),"other").unwrap();
    let bytes=b"original logical history";let c=DraftClaim::new(bytes.len() as u64,crate::hash::digest(bytes).unwrap()).unwrap();
    save_first(&t.system,OperationId::new().unwrap(),other.file,c,Cursor::new(bytes)).unwrap();
    commit_working(&t.system,OperationId::new().unwrap(),other.file,1).unwrap();
    let x=DraftClaim::new(1,crate::hash::digest(b"X").unwrap()).unwrap();
    save_interval(&t.system,OperationId::new().unwrap(),other.file,1,SpanEdit::new(0,x,LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"X")).unwrap();
    commit_working(&t.system,OperationId::new().unwrap(),other.file,2).unwrap();
    let b=Clean::capture(&t.system,other.file).unwrap().verify().unwrap();
    fs::create_dir(t.b.join("other")).unwrap();
    assert!(Destination::select(&t.system,&b,t.dest).is_err());
    // Neither prepare() nor at_vacant_cut() takes a substitute source argument.
    let a=Destination::select(&t.system,&t.src,t.dest).unwrap().prepare().unwrap();
    assert_eq!(a.source().file(),t.src.file());assert_ne!(a.source().file(),b.file());
    a.at_vacant_cut(Access::Read,|_,_,_,_|Ok(())).unwrap();
    fs::create_dir(t.b.join(a.source().name())).unwrap();
    assert!(a.at_vacant_cut(Access::Read,|_,_,_,_|->io::Result<()>{panic!("occupied name used")}).is_err());
}

#[test]
#[ignore="two private registered native filesystems"]
fn a_real_reserved_create_with_no_final_name_prevents_initial_and_delayed_transfer_admission(){
    let t=make(false);let planned=Destination::select(&t.system,&t.src,t.dest).unwrap();
    let op=OperationId::new().unwrap();
    assert!(crate::ordinary::create_inner(&t.system,op,t.dest,t.src.name(),|cut|if cut==crate::ordinary::Cut::Reservation{
        Err(io::Error::other("original name reserved, not yet present"))
    }else{Ok(())}).is_err());
    assert!(!t.b.join(t.src.name()).exists());
    assert!(Destination::select(&t.system,&t.src,t.dest).is_err());
    assert!(planned.prepare_after(||panic!("attempted anonymous preparation for an already reserved destination")).is_err());
    assert!(crate::creation_receipt(&t.system,op).unwrap().is_some());assert_eq!(fs::read_dir(&t.b).unwrap().count(),0);
}

#[test]
#[ignore="private registered original filesystems; original local decisions, not synthetic identities"]
fn move_frame_keeps_the_original_file_history_requires_every_distinct_role_and_exact_original_request(){
    use super::super::transfer::{Moved,Origin,Sites};
    use crate::ordinary::wire;
    let t=make(false);let prep=Destination::select(&t.system,&t.src,t.dest).unwrap().prepare().unwrap();
    let op=OperationId::new().unwrap();let ticket=OperationId::new().unwrap();
    let original=Origin::select(op,&prep,ticket).unwrap();let(c,f,d)=t.src.originals();let sites=Sites::birthplace(c,f,d);
    let (n,prev)=prep.under_original_writer(|_,s,_,_|Ok(s.frontier())).unwrap();
    let m=Moved::seal(t.src.pool(),n+1,prev,original.clone(),[7;32],None,sites.clone()).unwrap();
    let round=Moved::read(t.src.pool(),&m.bytes,wire::envelope(t.src.pool(),&m.bytes).unwrap()).unwrap();
    assert_eq!(m,round);round.require_history(c,f,d).unwrap();
    assert!(round.require_history(c,f,crate::ordinary::custody::transfer::ChildRoles::None).is_err());
    let mut other=round.clone();other.original.entry=crate::EntryId::new().unwrap();assert!(other.require_history(c,f,d).is_err());
    let mut other=round.clone();other.target.raw=other.target.node.clone();assert!(other.require_history(c,f,d).is_err());
    let mut other=round.clone();other.original.parent=[1;32];assert!(other.require_history(c,f,d).is_err());
    let mut other=round.clone();other.original.uid=other.original.uid.wrapping_add(1);assert!(other.require_history(c,f,d).is_err());
    assert!(Moved::seal(t.src.pool(),n+1,prev,original.clone(),[7;32],Some(255),sites.clone()).is_err());
    assert!(Moved::seal(t.src.pool(),n+1,prev,original.clone(),[7;32],Some(0),sites.clone()).is_err());
    let mut no_child=sites;no_child.children=crate::ordinary::custody::transfer::ChildRoles::None;
    assert!(Moved::seal(t.src.pool(),n+1,prev,original,[7;32],None,no_child).is_err());
    assert!(fs::read_dir(&t.b).unwrap().next().is_none());
}

#[test]
#[ignore="private registered original filesystems; fixture source only"]
fn original_move_reservation_cold_reopens_one_pending_slot_never_grants_serving_or_retirement(){
    use super::super::{allocation::MoveAllocation,transfer::{Sites,Moved}};
    let t=make(false);let op=OperationId::new().unwrap();
    let p=Destination::select(&t.system,&t.src,t.dest).unwrap().prepare().unwrap();
    let o=MoveAllocation::reserve(&p,op).unwrap();drop(p);
    let (pool,_,s)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();
    let a=s.moving.as_ref().unwrap();assert_eq!(a.original,o);
    assert!(!a.opened());assert!(!a.retiring());assert!(a.stage().is_none());
    assert!(s.decided(op).is_none());assert!(s.named(t.src.volume(),t.src.name()).is_some());assert!(s.named(t.dest,t.src.name()).is_none());
    let(c,f,d)=t.src.originals();let m=Moved::seal(pool.id(),a.seq,a.previous,o.clone(),a.digest,None,Sites::birthplace(c,f,d)).unwrap();
    assert!(a.matches(&m));assert!(s.confirmed_handoff(op).is_err());
    let pending_inode=fs::metadata(t.system.join("FILE-ALLOCATION")).unwrap();
    assert!(!a.opened());assert_eq!(fs::metadata(t.system.join("FILE-ALLOCATION")).unwrap().ctime_nsec(),pending_inode.ctime_nsec());
    let mut wrong=m.clone();wrong.original.ticket=OperationId::new().unwrap();assert!(!a.matches(&wrong));
    drop(s);drop(pool);
    assert!(create_empty(&t.system,OperationId::new().unwrap(),t.dest,"unrelated").is_err());
    assert_eq!(crate::current_info(&t.system,t.src.file()).unwrap().file.volume,t.src.volume());
    assert!(!t.b.join("name").exists());
}

#[test]
#[ignore="two private original filesystems; refuse without borrowing the only SYSTEM slot"]
fn dirty_first_full_or_used_operation_never_reserves_handoff_or_blocks_sibling(){
    use super::super::allocation::MoveAllocation;
    let t=make(false);let f=create_empty(&t.system,OperationId::new().unwrap(),t.src.volume(),"single").unwrap();
    let r=save_first(&t.system,OperationId::new().unwrap(),f.file,DraftClaim::new(1,crate::hash::digest(b"R").unwrap()).unwrap(),Cursor::new(b"R")).unwrap();
    commit_working(&t.system,OperationId::new().unwrap(),f.file,1).unwrap();
    let e=SpanEdit::new(0,DraftClaim::new(1,crate::hash::digest(b"D").unwrap()).unwrap(),LengthPolicy::KeepOrGrow).unwrap();
    save_interval(&t.system,OperationId::new().unwrap(),f.file,1,e,Cursor::new(b"D")).unwrap();
    let rejected=OperationId::new().unwrap();
    assert!(crate::move_file(&t.system,rejected,f.file,t.src.volume(),t.dest,0).is_err());
    assert!(crate::move_receipt(&t.system,rejected,f.file,t.src.volume(),t.dest,0).unwrap().is_none());
    assert!(Clean::capture(&t.system,f.file).is_err());
    let p=Destination::select(&t.system,&t.src,t.dest).unwrap().prepare().unwrap();
    assert!(MoveAllocation::reserve(&p,r.operation).is_err());
    assert!(!t.system.join("FILE-ALLOCATION").exists());assert!(!t.system.join("FILE-INTENT").exists());
    assert!(t.b.read_dir().unwrap().next().is_none());drop(p);
    create_empty(&t.system,OperationId::new().unwrap(),t.dest,"sibling").unwrap();
}

#[test]
#[ignore="three genuine private filesystems; actual named partial source and destination"]
fn unfinished_cross_mount_stage_reopens_only_issued_roles_and_does_not_allocate_an_already_copied_role(){
    use super::super::{allocation::MoveAllocation,placed::Reopened};
    let t=make(true);let op=OperationId::new().unwrap();
    let p=Destination::select(&t.system,&t.src,t.dest).unwrap().for_operation(op).unwrap();
    let original=MoveAllocation::reserve(&p,op).unwrap();
    assert!(p.staged_decision(op,&mut |c|if c==crate::ordinary::Cut::MoveLeafLinked{Err(io::Error::other("after first private leaf"))}else{Ok(())}).is_err());
    drop(p);assert!(!t.b.join("name").exists());
    let p=Destination::select(&t.system,&t.src,t.dest).unwrap().for_operation(op).unwrap();
    assert_eq!(p.existing.as_ref().unwrap().len(),1);assert_eq!(p.files.len()+1,t.src.objects().len());
    let(m,stage)=p.staged_decision(op,&mut |_|Ok(())).unwrap();
    let (input,name)=stage.unwrap();let checked=Reopened::verify(input).unwrap();
    p.original_writer_after_naming(|_,s,_,new|{
        assert!(s.decided(op).is_none());assert_eq!(s.moving.as_ref().unwrap().original,original);
        checked.require(new,&name,&m)?;Ok(())
    }).unwrap();
    assert!(!t.b.join("name").exists());
}

#[test]
#[ignore="three genuinely independent filesystems"]
fn a_missing_earlier_synced_private_role_never_becomes_a_fresh_unlinked_replacement(){
    use super::super::allocation::MoveAllocation;
    let t=make(true);let op=OperationId::new().unwrap();
    let p=Destination::select(&t.system,&t.src,t.dest).unwrap().for_operation(op).unwrap();
    let o=MoveAllocation::reserve(&p,op).unwrap();let mut count=0;
    assert!(p.staged_decision(op,&mut |cut|{
        if cut==crate::ordinary::Cut::MoveLeafLinked{count+=1;if count==2{return Err(io::Error::other("two roles named"));}}Ok(())
    }).is_err());drop(p);
    let private=t.b.join(format!(".zxs-new-{}-0",o.ticket));let earlier=private.join("FILE");let saved=t.b.parent().unwrap().join("held-away-original");
    fs::rename(&earlier,&saved).unwrap();let remaining=fs::read_dir(&private).unwrap().next().unwrap().unwrap().path();
    let inode=fs::metadata(&remaining).unwrap().ino();let bytes=fs::read(&remaining).unwrap();
    assert!(Destination::select(&t.system,&t.src,t.dest).unwrap().for_operation(op).is_err());
    assert!(!earlier.exists());assert_eq!(fs::metadata(&remaining).unwrap().ino(),inode);assert_eq!(fs::read(&remaining).unwrap(),bytes);
    assert!(crate::current_info(&t.system,t.src.file()).is_ok());assert!(!t.b.join("name").exists());
}
