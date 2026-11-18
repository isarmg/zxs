use super::*;
use crate::test_support::PrivateTemp;
use crate::{initialize,inspect,Role,PoolInfo,RegisteredVolume};
use std::{fs as osfs,io::Write,os::unix::fs::PermissionsExt,path::PathBuf};

struct Fixture { _first:PrivateTemp,_second:PrivateTemp,system:PathBuf,data:PathBuf,other:PathBuf,v:VolumeId }
impl Fixture{
    fn new()->Self{
        let first=PrivateTemp::under(&std::env::temp_dir());
        let second=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        assert_ne!(first.1.device,second.1.device);
        let system=first.child("system");let data=first.child("one");let other=first.child("two");
        let recycle=second.child("recycle");
        let info:PoolInfo=initialize(OperationId::new().unwrap(),&system,&recycle,&[data.clone(),other.clone()]).unwrap();
        let pick: &RegisteredVolume=info.volumes.iter().find(|r|r.volume.role==Role::File&&r.volume.path==data).unwrap();
        let v=pick.id;
        Self{_first:first,_second:second,system,data,other,v}
    }
}

#[test]
fn first_name_domain_never_guesses_unicode_or_portable_aliases(){
    for bad in ["","..","A","a/b","a.","con.txt","com1","资料","x\\y"]{assert!(wire::human_name(bad.as_bytes()).is_err(),"{bad}");}
    for good in ["report.txt","0","opaque.zxs","a-b_2.info"]{assert_eq!(wire::human_name(good.as_bytes()).unwrap(),good);}
    assert!(wire::Frame::size(b"not-a-frame").is_err());
    assert_eq!(wire::Frame::size(b"ZXS/FILE").unwrap(),None);
}

#[test]
#[ignore="two explicitly selected private test filesystems, distinct UUIDs, export handles and xattrs"]
fn registered_empty_file_has_one_identity_at_every_original_cut_and_reopens_without_other_roots(){
    for stop in [Cut::Reservation,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,Cut::MarkedBeforeSync,Cut::Decision,Cut::Installation,Cut::IntentReleased,Cut::Release]{
        let t=Fixture::new();let op=OperationId::new().unwrap();
        let original=b"not a FILE or root alias";
        let offline=t._first.0.join("other-offline");osfs::rename(&t.other,&offline).unwrap();
        assert!(inspect(&t.system).is_err()); // actual whole-pool inspection differs
        assert!(list_initial_files(&t.system,t.v).unwrap().files.is_empty());
        let err=create_inner(&t.system,op,t.v,"report.txt",|c|if c==stop{Err(io::Error::other("interrupt original"))}else{Ok(())}).unwrap_err();
        assert_eq!(err.to_string(),"interrupt original");
        if stop==Cut::MarkedBeforeSync{
            let failure=pool::reject_unconfirmed_for_test(&t.system,|_,_|Err(io::Error::other("checkpoint inode sync failed"))).unwrap_err();
            assert_eq!(failure.to_string(),"checkpoint inode sync failed");
        }
        let before=creation_receipt(&t.system,op).unwrap().unwrap();
        if stop==Cut::FrameSynced{assert_eq!(before.status,CreationStatus::DecisionAwaitingConfirmation);}
        let published=matches!(stop,Cut::MarkedBeforeSync|Cut::Decision|Cut::Installation|Cut::IntentReleased|Cut::Release);
        assert_eq!(before.file.decision_sequence.is_some(),published);
        if !published{assert!(read_initial_file(&t.system,before.file.file).is_err());}
        assert!(create_empty(&t.system,op,t.v,"other.txt").is_err());
        let result=create_empty(&t.system,op,t.v,"report.txt").unwrap();
        assert_eq!(result.file,before.file.file);assert_eq!(result.entry,before.file.entry);assert_eq!(result.history,before.file.history);
        assert_eq!(result.decision_sequence,Some(1));
        assert_eq!(list_initial_files(&t.system,t.v).unwrap().files,[result.clone()]);
        assert_eq!(read_initial_file(&t.system,result.file).unwrap(),(result.clone(),Vec::new()));
        assert_eq!(osfs::read_dir(t.data.join("report.txt")).unwrap().count(),1);
        assert_eq!(osfs::read_dir(&t.data).unwrap().count(),1);
        assert!(create_empty(&t.system,OperationId::new().unwrap(),t.v,"report.txt").is_err());
        let old=t._first.0.join("first-offline");osfs::rename(&t.data,&old).unwrap();
        t._first.child("one");osfs::write(t.data.join("report.txt"),original).unwrap();
        assert_eq!(create_empty(&t.system,op,t.v,"report.txt").unwrap(),result);
        assert!(read_initial_file(&t.system,result.file).is_err());
        assert_eq!(osfs::read(t.data.join("report.txt")).unwrap().as_slice(),original.as_slice());
    }
}

#[test]
#[ignore="two explicit test filesystems with native birth identity and anonymous file publication"]
fn a_torn_frame_can_only_be_completed_from_its_same_durable_sealed_bytes(){
    for matched in [false,true]{
        let t=Fixture::new();let op=OperationId::new().unwrap();
        create_inner(&t.system,op,t.v,"first",|c|if c==Cut::Intent{Err(io::Error::other("before append"))}else{Ok(())}).unwrap_err();
        let (prefix,id)={
            let p=LocalPool::open(&t.system,Access::Write).unwrap();
            let (s,j)=p.snapshot().unwrap();
            let f=j.pending.as_ref().unwrap().frame.creation().unwrap();
            let mut bytes=f.bytes[..13].to_vec();
            if !matched{bytes[4]^=0x40;}
            let mut raw=osfs::OpenOptions::new().append(true).open(t.system.join("REGISTRATION")).unwrap();
            assert_eq!(raw.metadata().unwrap().len(),s.stat.size());
            raw.write_all(&bytes).unwrap();raw.sync_all().unwrap();
            (osfs::read(t.system.join("REGISTRATION")).unwrap(),f.create.file)
        };
        if matched{
            assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap().status,CreationStatus::Prepared);
            let f=create_empty(&t.system,op,t.v,"first").unwrap();assert_eq!(f.file,id);
            assert!(read_initial_file(&t.system,id).unwrap().1.is_empty());
        }else{
            assert!(create_empty(&t.system,op,t.v,"first").is_err());
            assert!(creation_receipt(&t.system,op).is_err());
            assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),prefix);
            assert!(!t.data.join("first").exists());
        }
    }
}

#[test]
#[ignore="two explicit filesystems with native original material tokens"]
fn unsealed_equal_byte_replacement_and_unknown_inventory_remain_untouched(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    create_inner(&t.system,op,t.v,"report",|c|if c==Cut::BeforeIntent{Err(io::Error::other("before seal"))}else{Ok(())}).unwrap_err();
    let p=LocalPool::open(&t.system,Access::Read).unwrap();let (_,j)=p.snapshot().unwrap();
    let a=j.allocated.as_ref().unwrap();
    let stage=t.data.join(format!(".zxs-new-{}-{}",a.attempt,a.bound().unwrap().slot));let local=stage.join("FILE");
    let bytes=osfs::read(&local).unwrap();let original=t._first.0.join("original-material");
    drop(p);
    osfs::rename(&local,&original).unwrap();osfs::write(&local,&bytes).unwrap();
    osfs::set_permissions(&local,osfs::Permissions::from_mode(0o600)).unwrap();
    assert!(create_empty(&t.system,op,t.v,"report").is_err());
    assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap().status,CreationStatus::Reserved);
    let pending=list_initial_files(&t.system,t.v).unwrap();
    assert!(pending.files.is_empty());assert_eq!(pending.reserved_private_entries,1);
    assert_eq!(osfs::read(&local).unwrap(),bytes);assert!(!t.data.join("report").exists());
    osfs::remove_file(&local).unwrap();osfs::rename(&original,&local).unwrap();
    let good=create_empty(&t.system,op,t.v,"report").unwrap();
    osfs::write(t.data.join("unrecognized"),b"keep").unwrap();
    assert!(list_initial_files(&t.system,t.v).is_err());
    assert!(read_initial_file(&t.system,good.file).unwrap().1.is_empty());
    assert_eq!(osfs::read(t.data.join("unrecognized")).unwrap().as_slice(),b"keep");
}

#[test]
#[ignore="two explicit private filesystems with native original material"]
fn full_original_frame_before_sync_is_not_a_confirmed_publication(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    create_inner(&t.system,op,t.v,"first",|c|if c==Cut::Intent{Err(io::Error::other("before append"))}else{Ok(())}).unwrap_err();
    let file={
        let p=LocalPool::open(&t.system,Access::Write).unwrap();let (_,j)=p.snapshot().unwrap();
        let original=j.pending.as_ref().unwrap().frame.creation().unwrap();
        let mut raw=osfs::OpenOptions::new().append(true).open(t.system.join("REGISTRATION")).unwrap();
        raw.write_all(&original.bytes).unwrap(); // Complete bytes, deliberately no successful sync.
        original.create.file
    };
    let error=pool::reject_unconfirmed_for_test(&t.system,|_,_|Err(io::Error::other("original FILE journal sync failed"))).unwrap_err();
    assert_eq!(error.to_string(),"original FILE journal sync failed");
    assert!(!t.data.join("first").exists());
    let confirmed=create_empty(&t.system,op,t.v,"first").unwrap(); // real production path repeats the sync
    assert_eq!(confirmed.file,file);assert_eq!(confirmed.decision_sequence,Some(1));
    assert_eq!(list_initial_files(&t.system,t.v).unwrap().files,[confirmed]);
}

#[test]
#[ignore="real initialized owned test volumes"]
fn registered_role_and_operation_cannot_be_reused_and_unreviewed_names_do_not_get_written(){
    let t=Fixture::new();let registration=inspect(&t.system).unwrap();
    let old=std::fs::read(t.system.join("REGISTRATION")).unwrap();
    assert!(create_empty(&t.system,registration.initialized_by,t.v,"first").is_err());
    for r in registration.volumes.iter().filter(|v|v.volume.role!=Role::File){
        assert!(create_empty(&t.system,OperationId::new().unwrap(),r.id,"first").is_err());
    }
    assert_eq!(std::fs::read(t.system.join("REGISTRATION")).unwrap(),old);
    assert_eq!(osfs::read_dir(&t.data).unwrap().count(),0);
    osfs::write(t.data.join("first."),b"untouched").unwrap();
    assert!(create_empty(&t.system,OperationId::new().unwrap(),t.v,"first").is_err());
    assert_eq!(osfs::read(t.data.join("first.")).unwrap().as_slice(),b"untouched");
    assert_eq!(osfs::read_dir(&t.data).unwrap().count(),1);
}

#[test]
#[ignore="registered original alert boundary on actual owned test filesystems"]
fn loss_of_one_or_two_whole_acknowledged_frames_or_same_height_fork_never_reopens_as_a_smaller_pool(){
    for damage in 0..3{
        let t=Fixture::new();let op1=OperationId::new().unwrap();let op2=OperationId::new().unwrap();
        let register=t.system.join("REGISTRATION");
        let genesis=osfs::read(&register).unwrap();
        let first=create_empty(&t.system,op1,t.v,"first").unwrap();
        let one=osfs::read(&register).unwrap();
        let second=create_empty(&t.system,op2,t.v,"second").unwrap();
        assert_eq!(second.decision_sequence,Some(2));assert_ne!(first.file,second.file);
        let broken=match damage{
            0=>one.clone(),
            1=>genesis,
            _=>{
                let (p,_,j)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();
                let real=j.operation(op2).unwrap();let mut changed=real.create.clone();
                changed.allocation[0]^=1;
                let fork=Frame::new(p.id(),real.seq,real.previous,changed).unwrap();
                assert_eq!(fork.bytes.len(),real.bytes.len());
                assert_ne!(fork.digest,real.digest);Frame::read(p.id(),&fork.bytes).unwrap();
                let mut altered=one;altered.extend_from_slice(&fork.bytes);altered
            }
        };
        // Mutate bytes only on the test's original inode: its native floor and
        // formerly installed private containers still exist.
        osfs::write(&register,&broken).unwrap();
        assert!(inspect(&t.system).is_err());assert!(creation_receipt(&t.system,op2).is_err());
        assert!(list_initial_files(&t.system,t.v).is_err());assert!(read_initial_file(&t.system,first.file).is_err());
        assert!(create_empty(&t.system,OperationId::new().unwrap(),t.v,"third").is_err());
        assert_eq!(osfs::read(&register).unwrap(),broken);
        assert_eq!(osfs::read_dir(&t.data).unwrap().count(),2);
    }
}

#[test]
#[ignore="registered original prepared frame and associated objects on two test filesystems"]
fn a_full_unacknowledged_frame_needs_its_original_allocation_and_immutable_seal(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    create_inner(&t.system,op,t.v,"first",|c|if c==Cut::FrameSynced{Err(io::Error::other("no acknowledgement"))}else{Ok(())}).unwrap_err();
    let bytes=osfs::read(t.system.join("REGISTRATION")).unwrap();
    assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap().status,CreationStatus::DecisionAwaitingConfirmation);
    for name in ["FILE-INTENT","FILE-ALLOCATION"]{
        let original=t.system.join(name);let held=t._first.0.join("held-original");
        osfs::rename(&original,&held).unwrap();
        assert!(creation_receipt(&t.system,op).is_err());assert!(create_empty(&t.system,op,t.v,"first").is_err());
        assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),bytes);
        assert!(!t.data.join("first").exists());
        osfs::rename(&held,&original).unwrap();
    }
    let f=create_empty(&t.system,op,t.v,"first").unwrap();
    assert_eq!(f.decision_sequence,Some(1));assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),bytes);
}

#[test]
#[ignore="private actual original SYSTEM record with supported user extended attributes"]
fn missing_acknowledgement_is_not_interpreted_as_an_empty_new_pool(){
    use std::os::{fd::AsRawFd,raw::{c_char,c_int}};
    unsafe extern "C"{fn fremovexattr(fd:c_int,name:*const c_char)->c_int;}
    let t=Fixture::new();let op=OperationId::new().unwrap();
    let r=osfs::OpenOptions::new().read(true).write(true).open(t.system.join("REGISTRATION")).unwrap();
    let bytes=osfs::read(t.system.join("REGISTRATION")).unwrap();
    // SAFETY: test's own newly initialized private register, fixed attribute.
    assert_eq!(unsafe{fremovexattr(r.as_raw_fd(),c"user.zxs.confirmed-frontier".as_ptr())},0);
    r.sync_all().unwrap();drop(r);
    assert!(inspect(&t.system).is_err());assert!(create_empty(&t.system,op,t.v,"first").is_err());
    assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),bytes);
    assert!(osfs::read_dir(&t.data).unwrap().next().is_none());
}

#[test]
#[ignore="genuine bound FILE root and an interrupted original private mkdir"]
fn unbound_mkdir_is_neither_adopted_nor_swept_and_does_not_poison_business_listing(){
    for foreign in [false,true]{
        let t=Fixture::new();let prior=create_empty(&t.system,OperationId::new().unwrap(),t.v,"prior").unwrap();
        let op=OperationId::new().unwrap();
        create_inner(&t.system,op,t.v,"new",|c|if c==Cut::UnboundMkdir{Err(io::Error::other("after mkdir before binding"))}else{Ok(())}).unwrap_err();
        let before=creation_receipt(&t.system,op).unwrap().unwrap();
        assert_eq!(before.status,CreationStatus::Reserved);assert_eq!(before.file.decision_sequence,None);
        let old_slot={
            let (_,_,j)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();
            let a=j.allocated.as_ref().unwrap();assert!(a.stage.is_none());
            t.data.join(format!(".zxs-new-{}-0",a.attempt))
        };
        let old_birth=fs::Birth::read(&fs::HeldDir::open(&old_slot).unwrap().file).unwrap();
        assert!(osfs::read_dir(&old_slot).unwrap().next().is_none());
        let listed=list_initial_files(&t.system,t.v).unwrap();
        assert_eq!(listed.files,[prior.clone()]);assert_eq!(listed.reserved_private_entries,1);
        if foreign{
            osfs::write(old_slot.join("do-not-adopt"),b"unknown").unwrap();
            assert!(create_empty(&t.system,op,t.v,"new").is_err());
            assert_eq!(osfs::read(old_slot.join("do-not-adopt")).unwrap().as_slice(),b"unknown");
            assert!(!t.data.join("new").exists());assert_eq!(osfs::read_dir(&t.data).unwrap().count(),2);
            assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap(),before);
        }else{
            let done=create_empty(&t.system,op,t.v,"new").unwrap();
            assert_eq!((done.file,done.entry,done.history),(before.file.file,before.file.entry,before.file.history));
            assert_eq!(create_empty(&t.system,op,t.v,"new").unwrap(),done); // no additional physical attempt
            assert_eq!(old_birth,fs::Birth::read(&fs::HeldDir::open(&old_slot).unwrap().file).unwrap());
            assert!(osfs::read_dir(&old_slot).unwrap().next().is_none());
            let active=fs::HeldDir::open(&t.data.join("new")).unwrap();
            assert_ne!(fs::Birth::read(&active.file).unwrap(),old_birth);
            let complete=list_initial_files(&t.system,t.v).unwrap();
            assert_eq!(complete.files,[done.clone(),prior.clone()]);assert_eq!(complete.reserved_private_entries,1);
            assert!(read_initial_file(&t.system,done.file).unwrap().1.is_empty());
            assert_eq!(osfs::read_dir(&t.data).unwrap().count(),3);
        }
        assert_eq!(read_initial_file(&t.system,prior.file).unwrap().0,prior);
    }
}

#[test]
#[ignore="bounded genuine interruptions of one issued original ticket"]
fn unbound_private_attempt_budget_is_finite_and_never_generates_another_ticket(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    let mut seen=None;
    for n in 1..=allocation::SLOTS{
        create_inner(&t.system,op,t.v,"original",|c|if c==Cut::UnboundMkdir{Err(io::Error::other("interrupted before bind"))}else{Ok(())}).unwrap_err();
        let receipt=creation_receipt(&t.system,op).unwrap().unwrap();
        assert_eq!(*seen.get_or_insert(receipt.file.file),receipt.file.file);
        let listed=list_initial_files(&t.system,t.v).unwrap();
        assert!(listed.files.is_empty());assert_eq!(listed.reserved_private_entries,usize::from(n));
    }
    assert!(create_empty(&t.system,op,t.v,"original").is_err());
    assert_eq!(osfs::read_dir(&t.data).unwrap().count(),usize::from(allocation::SLOTS));
    assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap().status,CreationStatus::Reserved);
}

#[test]
#[ignore="two private registered native filesystems"]
fn late_create_occupant_keeps_the_unconfirmed_original_and_never_replaces_an_installed_name(){
    use std::os::unix::fs::MetadataExt;
    // At Decision the same obstruction is an installation failure, NOT a
    // reason to withdraw the already confirmed original CREATE.
    for (replace_local,when) in [(false,Cut::FrameSynced),(true,Cut::FrameSynced),(false,Cut::Decision)]{
        let t=Fixture::new();let op=OperationId::new().unwrap();
        let saved=t._first.0.join("held-original");let displaced=t._first.0.join("independent-occupant");
        let mut occupied=None;
        assert!(create_inner(&t.system,op,t.v,"new-file",|at|{
            if at!=when{return Ok(());}
            let path=if replace_local{
                let mut private=osfs::read_dir(&t.data)?;let dir=private.next().unwrap()?.path();assert!(private.next().is_none());
                let local=dir.join("FILE");osfs::rename(&local,&saved)?;osfs::copy(&saved,&local)?;local
            }else{
                let path=t.data.join("NEW-FILE");osfs::write(&path,b"original occupant")?;path
            };
            occupied=Some((path.clone(),osfs::metadata(&path)?.ino(),osfs::read(&path)?));Ok(())
        }).is_err());
        let(path,ino,bytes)=occupied.unwrap();if replace_local{assert_ne!(ino,osfs::metadata(&saved).unwrap().ino());}
        let original=creation_receipt(&t.system,op).unwrap().unwrap();let published=when==Cut::Decision;
        assert_eq!(original.status,if published{CreationStatus::PublishedInstallationUnconfirmed}else{CreationStatus::DecisionAwaitingConfirmation});
        assert_eq!(original.file.decision_sequence.is_some(),published);
        assert!(create_empty(&t.system,op,t.v,"new-file").is_err());
        assert_eq!(creation_receipt(&t.system,op).unwrap().unwrap(),original);
        assert_eq!(osfs::metadata(&path).unwrap().ino(),ino);assert_eq!(osfs::read(&path).unwrap(),bytes);
        osfs::rename(&path,&displaced).unwrap();if replace_local{osfs::rename(&saved,&path).unwrap();}
        let completed=create_empty(&t.system,op,t.v,"new-file").unwrap();
        assert_eq!((completed.file,completed.entry,completed.history),(original.file.file,original.file.entry,original.file.history));
        assert_eq!(completed.decision_sequence,Some(1));
        assert_eq!(read_initial_file(&t.system,completed.file).unwrap().0,completed);assert_eq!(osfs::read(displaced).unwrap(),bytes);
    }
}
