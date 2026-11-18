use super::command::{Request,perform};
use crate::{self as z,FileInfo,FormalNode,MoveStatus,OperationId,VolumeId,ordinary::Cut,test_support::PrivateTemp};
use std::{fs,io::{self,Cursor,Write},os::unix::fs::MetadataExt,path::{Path,PathBuf}};
fn op()->OperationId{OperationId::new().unwrap()}
fn claim(v:&[u8])->z::DraftClaim{z::DraftClaim::new(v.len()as u64,z::hash::digest(v).unwrap()).unwrap()}
const BEFORE:&[u8]=b"old retained FULL";const AFTER:&[u8]=b"new retained FULL";
struct Fixture{_s:PrivateTemp,_d:PrivateTemp,_r:PrivateTemp,sys:PathBuf,old:PathBuf,new:PathBuf,from:VolumeId,to:VolumeId,
    file:FileInfo,first:FormalNode,last:FormalNode}
impl Fixture{
    fn build(cross:bool,birth_gap:bool)->Self{Self::planned(cross,birth_gap,None,None)}
    fn planned(cross:bool,birth_gap:bool,old_length:Option<usize>,new_length:Option<usize>)->Self{
        let s=PrivateTemp::under(&std::env::temp_dir());
        let d=if cross{PrivateTemp::under(Path::new(&std::env::var_os("ZXS_MOVE_FILE_TMPDIR").unwrap()))}else{PrivateTemp::under(&std::env::temp_dir())};
        let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        let sys=s.child("system");let old=old_length.map_or_else(||s.child("original"),|n|native_root_length(&s.0,n));
        let new=new_length.map_or_else(||d.child("planned"),|n|native_root_length(&d.0,n));let recycle=r.child("recycle");
        if cross{assert_ne!(fs::metadata(&old).unwrap().dev(),fs::metadata(&new).unwrap().dev());}
        let reg=z::initialize(op(),&sys,&recycle,&[old.clone(),new.clone()]).unwrap();
        let from=reg.volumes.iter().find(|v|v.volume.path==old).unwrap().id;let to=reg.volumes.iter().find(|v|v.volume.path==new).unwrap().id;
        let creation=op();if birth_gap{assert!(z::ordinary::create_inner(&sys,creation,from,"name",|s|if s==Cut::UnboundMkdir{
            Err(io::Error::other("created but not bound"))}else{Ok(())}).is_err());}
        let file=z::create_empty(&sys,creation,from,"name").unwrap();
        z::save_first(&sys,op(),file.file,claim(BEFORE),Cursor::new(BEFORE)).unwrap();let first=z::commit_working(&sys,op(),file.file,1).unwrap();
        z::save_interval(&sys,op(),file.file,1,z::SpanEdit::new(0,claim(b"new"),z::LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"new")).unwrap();
        let last=z::commit_working(&sys,op(),file.file,2).unwrap();
        Self{_s:s,_d:d,_r:r,sys,old,new,from,to,file,first,last}
    }
    fn cmd(&self)->Request{Request{call:crate::ordinary::custody::purpose::Call::Move,op:op(),id:self.file.file,from:self.from,to:self.to,epoch:0}}
    fn call(&self,r:Request)->io::Result<z::MoveReceipt>{z::move_file(&self.sys,r.op,r.id,r.from,r.to,r.epoch)}
    fn receipt(&self,r:Request)->z::MoveReceipt{z::move_receipt(&self.sys,r.op,r.id,r.from,r.to,r.epoch).unwrap().unwrap()}
    fn all_history(&self){let id=self.file.file;
        let mut b=Vec::new();assert_eq!(z::read_node_to(&self.sys,id,self.first.node,&mut b).unwrap(),self.first);assert_eq!(b,BEFORE);
        b.clear();assert_eq!(z::read_node_to(&self.sys,id,self.last.node,&mut b).unwrap(),self.last);assert_eq!(b,AFTER);
        b.clear();assert_eq!(z::read_current_to(&self.sys,id,&mut b).unwrap().head,Some(self.last.clone()));assert_eq!(b,AFTER);
    }
    fn exercise(&self,cross:bool){
        let r=self.cmd();let before=fs::metadata(self.old.join("name")).unwrap();
        let moved=self.call(r).unwrap();assert_eq!(moved.status,MoveStatus::CompleteCheckedNow);
        assert_eq!((moved.file,moved.entry,moved.history),(self.file.file,self.file.entry,self.file.history));
        assert_eq!((moved.from,moved.to,moved.source_epoch,moved.destination_epoch),(self.from,self.to,0,1));
        assert!(!self.old.join("name").exists());let after=fs::metadata(self.new.join("name")).unwrap();
        if cross{assert_ne!(before.dev(),after.dev());}else{assert_eq!((before.dev(),before.ino()),(after.dev(),after.ino()));}
        let old_off=self.old.with_file_name("old-offline");fs::rename(&self.old,&old_off).unwrap();
        self.all_history();assert_eq!(z::current_info(&self.sys,r.id).unwrap().file.volume,self.to);
        let f=z::list_initial_files(&self.sys,self.to).unwrap();assert_eq!(f.files.len(),1);assert!(f.pending_moves.is_empty());
        assert_eq!(self.call(r).unwrap().status,MoveStatus::OriginalPublished);
        assert_eq!(self.receipt(r).decision_sequence,moved.decision_sequence);
        assert_eq!(z::create_empty(&self.sys,self.file.created_by,self.from,"name").unwrap(),self.file);
        assert_eq!(z::commit_working(&self.sys,self.first.created_by,r.id,1).unwrap(),self.first);
        assert_eq!(z::commit_working(&self.sys,self.last.created_by,r.id,2).unwrap(),self.last);
        assert!(z::move_file(&self.sys,r.op,r.id,self.to,self.from,0).is_err());
        assert!(z::move_file(&self.sys,r.op,r.id,self.from,self.to,1).is_err());
        fs::rename(old_off,&self.old).unwrap();let different=z::create_empty(&self.sys,op(),self.from,"name").unwrap();
        assert_ne!(different.file,r.id);assert_eq!(z::list_initial_files(&self.sys,self.from).unwrap().files,[different]);
        self.all_history();assert_eq!(self.receipt(r).file,r.id);
    }
}
#[test]
#[ignore="two private original filesystems"]
fn real_same_mount_move_keeps_native_directory_identity_and_both_nodes_after_source_goes_offline(){Fixture::build(false,false).exercise(false);}
#[test]
#[ignore="three private original filesystems"]
fn real_cross_filesystem_move_transfers_the_complete_same_file_without_adopting_the_later_same_name(){Fixture::build(true,false).exercise(true);}

#[test]
#[ignore="three private original filesystems"]
fn original_historical_full_reader_holds_all_source_retirement_until_it_finishes_but_new_custody_is_readable(){
    struct WhileReading<'a>{f:&'a Fixture,r:Request,entered:bool,received:Vec<u8>}
    impl Write for WhileReading<'_>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{if !self.entered{
            self.entered=true;assert!(self.f.call(self.r).is_err());
            assert_eq!(self.f.receipt(self.r).status,MoveStatus::TargetReopenedSourcePending);
            assert_eq!(fs::read_dir(self.f.old.join("name"))?.count(),4);
            assert_eq!(z::list_initial_files(&self.f.sys,self.f.from)?.files.len(),0);
            let listed=z::list_initial_files(&self.f.sys,self.f.to)?;assert_eq!(listed.files[0].file,self.r.id);
            assert_eq!(listed.pending_moves,[self.f.receipt(self.r)]);
            self.f.all_history();
        }self.received.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    let f=Fixture::build(true,false);let r=f.cmd();let mut sink=WhileReading{f:&f,r,entered:false,received:Vec::new()};
    z::read_node_to(&f.sys,r.id,f.first.node,&mut sink).unwrap();assert!(sink.entered);assert_eq!(sink.received,BEFORE);
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);assert!(!f.old.join("name").exists());
}

#[test]
#[ignore="three private original filesystems; exact original sync/fault boundaries"]
fn original_move_is_not_old_source_fallback_after_decision_and_recovers_only_the_same_partial_retirement(){
    for step in [Cut::Reservation,Cut::Intent,Cut::FrameSynced,Cut::Decision,Cut::MoveInstalled,Cut::MoveTargetFlagBeforeSync,Cut::MoveTargetChecked,Cut::MoveRetireFlagBeforeSync,Cut::MoveSourceAdmitted,Cut::MoveSourceIsolated,
        Cut::MoveEraseFlagBeforeSync,Cut::MoveLeafRetired,Cut::MoveEmptyFlagBeforeSync,Cut::MoveEmptyConfirmed,Cut::MoveDirectoryUnlinked,Cut::IntentReleased,Cut::Release]{
        let f=Fixture::build(true,false);let r=f.cmd();let mut hit=false;
        assert!(perform(&f.sys,r,&mut |s|{if s==step&&!hit{hit=true;return Err(io::Error::other("injected at exact native boundary"));}Ok(())}).is_err());assert!(hit);
        let state=f.receipt(r).status;
        if matches!(step,Cut::Reservation|Cut::Intent|Cut::FrameSynced){
            assert!(matches!(state,MoveStatus::Reserved|MoveStatus::Prepared|MoveStatus::AwaitingDecisionConfirmation));
            f.all_history();assert_eq!(z::current_info(&f.sys,r.id).unwrap().file.volume,f.from);
        }else if matches!(step,Cut::Decision|Cut::MoveInstalled){
            assert_eq!(state,MoveStatus::DecidedTargetPending);assert!(z::current_info(&f.sys,r.id).is_err());
            let mut b=Vec::new();assert!(z::read_node_to(&f.sys,r.id,f.first.node,&mut b).is_err());assert!(b.is_empty());
            for volume in [f.from,f.to]{let view=z::list_initial_files(&f.sys,volume).unwrap();assert!(view.files.is_empty());assert_eq!(view.pending_moves,[f.receipt(r)]);}
        }else{f.all_history();}
        let finish=f.call(r).unwrap();assert!(matches!(finish.status,MoveStatus::CompleteCheckedNow|MoveStatus::OriginalPublished));
        assert!(!f.old.join("name").exists());f.all_history();
    }
}
#[test]
#[ignore="three private registered filesystems"]
fn private_tickets_remain_only_on_the_volume_that_issued_them_and_do_not_hide_unknown_occupants(){
    let f=Fixture::build(true,true);let sibling=z::create_empty(&f.sys,op(),f.to,"sibling").unwrap();let r=f.cmd();
    let old_slot=fs::read_dir(&f.old).unwrap().map(|e|e.unwrap().file_name()).find(|n|n.to_string_lossy().starts_with(".zxs-new-")).unwrap();
    assert!(perform(&f.sys,r,&mut |s|if s==Cut::UnboundMkdir{Err(io::Error::other("mkdir before bound"))}else{Ok(())}).is_err());
    assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().files,[sibling.clone()]);
    assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().reserved_private_entries,1);
    f.call(r).unwrap();let before=z::list_initial_files(&f.sys,f.to).unwrap();assert_eq!(before.files.len(),2);assert_eq!(before.reserved_private_entries,1);
    assert_eq!(z::list_initial_files(&f.sys,f.from).unwrap().reserved_private_entries,1);
    fs::create_dir(f.new.join(&old_slot)).unwrap();assert!(z::list_initial_files(&f.sys,f.to).is_err());
    assert!(f.new.join(old_slot).is_dir());assert!(f.new.join("sibling").is_dir());
}

fn stop_at(f:&Fixture,r:Request,cut:Cut){
    let mut reached=false;
    assert!(perform(&f.sys,r,&mut |point|{if point==cut&&!reached{reached=true;return Err(io::Error::other("stop at original MOVE cut"));}Ok(())}).is_err());
    assert!(reached);
}
fn actual_raw(dir:&Path)->PathBuf{
    // Fixture oracle only: in these two versions only the original unwrapped
    // FULL consists of exactly BEFORE. Product reads use the original role.
    let mut entries=fs::read_dir(dir).unwrap().map(|p|p.unwrap().path()).filter(|p|fs::read(p).unwrap()==BEFORE);
    let p=entries.next().unwrap();assert!(entries.next().is_none());p
}
fn inventory_ids(dir:&Path)->std::collections::BTreeMap<std::ffi::OsString,(u64,u64)>{
    fs::read_dir(dir).unwrap().map(|p|{let e=p.unwrap();let m=e.metadata().unwrap();(e.file_name(),(m.dev(),m.ino()))}).collect()
}
#[test]
#[ignore="three private registered filesystems"]
fn decided_transfer_does_not_serve_or_reconstruct_from_an_equal_byte_replacement_in_its_original_target(){
    let f=Fixture::build(true,false);let r=f.cmd();stop_at(&f,r,Cut::Decision);
    let stage=fs::read_dir(&f.new).unwrap().map(|p|p.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with(".zxs-new-")).unwrap();
    let raw=actual_raw(&stage);let saved=f.new.with_file_name("held-original-raw");fs::rename(&raw,&saved).unwrap();
    fs::copy(&saved,&raw).unwrap();assert_ne!(fs::metadata(&saved).unwrap().ino(),fs::metadata(&raw).unwrap().ino());
    let source=inventory_ids(&f.old.join("name"));
    assert!(f.call(r).is_err());assert_eq!(f.receipt(r).status,MoveStatus::DecidedTargetPending);
    assert_eq!(inventory_ids(&f.old.join("name")),source);assert!(!f.new.join("name").exists());assert_eq!(fs::read(&raw).unwrap(),BEFORE);
    let mut got=Vec::new();assert!(z::read_current_to(&f.sys,r.id,&mut got).is_err());assert!(got.is_empty());
    assert!(z::read_node_to(&f.sys,r.id,f.first.node,&mut got).is_err());assert!(got.is_empty());
}
#[test]
#[ignore="three private registered filesystems"]
fn occupied_final_target_is_not_overwritten_and_missing_or_substituted_old_roles_are_never_implicitly_retired(){
    let f=Fixture::build(true,false);let r=f.cmd();stop_at(&f,r,Cut::Decision);
    let original=inventory_ids(&f.old.join("name"));let n=f.new.join("name");fs::create_dir(&n).unwrap();fs::write(n.join("foreign"),b"not the FILE").unwrap();
    assert!(f.call(r).is_err());assert_eq!(inventory_ids(&f.old.join("name")),original);assert_eq!(fs::read(n.join("foreign")).unwrap(),b"not the FILE");
    fs::rename(n,f.new.with_file_name("foreign-occupied")).unwrap();stop_at(&f,r,Cut::MoveTargetChecked);
    let live=f.old.join("name");let raw=actual_raw(&live);let moved=f.old.with_file_name("still-original-leaf");fs::rename(&raw,&moved).unwrap();
    // New target is already the only readable custodian; the source must not
    // be repaired, treated as already retired, or touched in some other order.
    let absent=inventory_ids(&live);assert!(f.call(r).is_err());assert_eq!(inventory_ids(&live),absent);f.all_history();
    fs::copy(&moved,&raw).unwrap();let impostor=inventory_ids(&live);assert_ne!(impostor,original);
    assert!(f.call(r).is_err());assert_eq!(f.receipt(r).status,MoveStatus::TargetReopenedSourcePending);
    assert_eq!(inventory_ids(&live),impostor);assert_eq!(fs::read(&raw).unwrap(),BEFORE);
    fs::rename(&f.old,f.old.with_file_name("unavailable-old-volume")).unwrap();f.all_history();
    assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().files[0].file,r.id);
}
#[test]
#[ignore="three private registered filesystems"]
fn old_role_changed_after_the_whole_retirement_admission_stops_before_any_unlink(){
    use std::os::unix::fs::FileExt;
    let f=Fixture::build(true,false);let r=f.cmd();let dir=f.old.join("name");let raw=actual_raw(&dir);
    let ids=inventory_ids(&dir);let mut hit=false;
    assert!(perform(&f.sys,r,&mut |step|{
        if step==Cut::MoveSourceAdmitted&&!hit{hit=true;let out=fs::OpenOptions::new().write(true).open(&raw)?;
            assert_eq!(out.write_at(b"X",0)?,1);out.sync_all()?;}
        Ok(())
    }).is_err());assert!(hit);assert_eq!(inventory_ids(&dir),ids);
    assert!(f.call(r).is_err());assert_eq!(inventory_ids(&dir),ids);f.all_history();
}

#[test]
#[ignore="three private registered filesystems"]
fn a_missing_whole_original_is_never_discharged_as_an_empty_directory(){
    for cut in [Cut::MoveSourceAdmitted,Cut::MoveSourceIsolated,Cut::MoveEraseFlagBeforeSync]{
        let f=Fixture::build(true,false);let r=f.cmd();stop_at(&f,r,cut);
        let original=if cut==Cut::MoveSourceAdmitted{f.old.join("name")}else{
            fs::read_dir(&f.old).unwrap().map(|p|p.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with(".zxs-move-out-")).unwrap()
        };
        let ids=inventory_ids(&original);assert_eq!(ids.len(),4);
        let away=f.old.with_file_name("whole-renamed-away");fs::rename(&original,&away).unwrap();
        assert!(f.call(r).is_err());assert_eq!(f.receipt(r).status,MoveStatus::TargetReopenedSourcePending);
        assert_eq!(inventory_ids(&away),ids);assert!(z::list_initial_files(&f.sys,f.from).is_err());
        assert_eq!(z::list_initial_files(&f.sys,f.to).unwrap().pending_moves,[f.receipt(r)]);f.all_history();
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&original).unwrap();
        fs::write(original.join("foreign"),b"not an admitted original").unwrap();
        assert!(f.call(r).is_err());assert_eq!(fs::read(original.join("foreign")).unwrap(),b"not an admitted original");
        fs::rename(&original,f.old.with_file_name("private-impostor")).unwrap();
        fs::rename(away,&original).unwrap();assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
    }
}
#[test]
#[ignore="three private registered filesystems"]
fn a_quarantine_never_accepts_a_missing_role_beyond_its_one_authorized_unlink(){
    let f=Fixture::build(true,false);let r=f.cmd();stop_at(&f,r,Cut::MoveEraseFlagBeforeSync);
    let q=fs::read_dir(&f.old).unwrap().map(|p|p.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with(".zxs-move-out-")).unwrap();
    let raw=actual_raw(&q);let stash=f.old.with_file_name("unapproved-raw");fs::rename(&raw,&stash).unwrap();
    let ids=inventory_ids(&q);assert_eq!(ids.len(),3);
    assert!(f.call(r).is_err());assert_eq!(inventory_ids(&q),ids);assert_eq!(fs::read(&stash).unwrap(),BEFORE);
    assert_eq!(f.receipt(r).status,MoveStatus::TargetReopenedSourcePending);f.all_history();
    fs::rename(stash,raw).unwrap();assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);assert!(!q.exists());
}
#[test]
#[ignore="two private registered filesystems"]
fn original_cross_parent_rename_cannot_open_target_until_both_held_parents_are_resynced(){
    let f=Fixture::build(false,false);let r=f.cmd();stop_at(&f,r,Cut::MoveInstalled);
    assert!(f.new.join("name").is_dir());assert!(!f.old.join("name").exists());
    let offline=f.old.with_file_name("offline-before-old-parent-fsync");fs::rename(&f.old,&offline).unwrap();
    assert!(f.call(r).is_err());assert_eq!(f.receipt(r).status,MoveStatus::DecidedTargetPending);
    let mut bytes=Vec::new();assert!(z::read_current_to(&f.sys,r.id,&mut bytes).is_err());assert!(bytes.is_empty());
    let listed=z::list_initial_files(&f.sys,f.to).unwrap();assert!(listed.files.is_empty());assert_eq!(listed.pending_moves,[f.receipt(r)]);
    fs::rename(offline,&f.old).unwrap();assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);f.all_history();
}

fn native_root_length(parent:&Path,n:usize)->PathBuf{
    use std::os::unix::{ffi::OsStrExt,fs::DirBuilderExt};
    let mut p=parent.to_owned();
    while p.as_os_str().as_bytes().len()<n{
        let delta=n-p.as_os_str().as_bytes().len();assert!(delta>=2);
        let mut k=(delta-1).min(160);if delta-k-1==1{k-=1;}
        p.push("a".repeat(k));fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    }
    assert_eq!(p.as_os_str().as_bytes().len(),n);p
}
#[test]
#[ignore="three private native filesystems; exact registered boundary paths"]
fn actual_move_never_reserves_if_its_source_isolation_or_destination_slots_cannot_subsequently_be_opened(){
    // max 4096 UTF-8 bytes. An issued '.zxs-new-{id}-0' uses 43 bytes;
    // '.zxs-move-out-{id}' uses 46, each also needs its separating slash.
    for (bad_source,bad_target) in [(Some(4051),None),(None,Some(4054))]{
        let f=Fixture::planned(true,false,bad_source,bad_target);let r=f.cmd();
        assert!(f.call(r).is_err());
        assert!(z::move_receipt(&f.sys,r.op,r.id,r.from,r.to,0).unwrap().is_none());
        for n in ["FILE-ALLOCATION","FILE-INTENT"]{assert!(!f.sys.join(n).exists());}
        let expected=z::current_info(&f.sys,r.id).unwrap();assert_eq!(expected.head,Some(f.last.clone()));
        // A failed request did not consume the frontier or create any named task.
        if bad_source.is_some(){assert!(fs::read_dir(&f.new).unwrap().next().is_none());z::create_empty(&f.sys,op(),f.to,"sibling").unwrap();}
        else{z::create_empty(&f.sys,op(),f.from,"sibling").unwrap();}
    }
    let f=Fixture::planned(true,false,Some(4049),Some(4052));let r=f.cmd();
    assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);f.all_history();
}

mod single_full {
    use super::*;
    include!("custody_single_full.rs");
}

// The target can be readable while an unrelated FILE is still unable to
// acquire the unique SYSTEM decision slot. No fallback or early release.
#[test]
#[ignore="three genuinely private filesystems; held reader, original offline then reattached"]
fn a_move_waiting_for_its_source_keeps_unrelated_reads_but_not_new_writes_until_the_same_move_finishes(){
    let f=Fixture::build(true,false);let r=f.cmd();
    let x=z::create_empty(&f.sys,op(),f.to,"outside-move").unwrap();
    z::save_first(&f.sys,op(),x.file,claim(b"outside"),Cursor::new(b"outside")).unwrap();
    let head=z::commit_working(&f.sys,op(),x.file,1).unwrap();
    let create=op();let save=op();let edit=z::SpanEdit::new(0,claim(b"X"),z::LengthPolicy::KeepOrGrow).unwrap();
    let check=||->io::Result<()>{
        let pending=f.receipt(r);assert_eq!(pending.status,MoveStatus::TargetReopenedSourcePending);
        let list=z::list_initial_files(&f.sys,f.to)?;assert_eq!(list.pending_moves,[pending]);assert_eq!(list.files.len(),2);
        assert!(list.files.iter().any(|a|a.file==r.id));assert!(list.files.contains(&x));
        f.all_history();let mut actual=Vec::new();
        assert_eq!(z::read_current_to(&f.sys,x.file,&mut actual)?.head,Some(head.clone()));assert_eq!(actual,b"outside");
        assert!(z::create_empty(&f.sys,create,f.to,"wait-for-move").is_err());
        assert!(z::save_interval(&f.sys,save,x.file,1,edit,Cursor::new(b"X")).is_err());
        assert!(z::creation_receipt(&f.sys,create)?.is_none());assert!(z::interval_receipt(&f.sys,save)?.is_none());
        assert!(!f.new.join("wait-for-move").exists());Ok(())
    };
    struct Inspect<F>{once:Option<F>,bytes:Vec<u8>}
    impl<F:FnOnce()->io::Result<()>> Write for Inspect<F>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{if let Some(f)=self.once.take(){f()?;}self.bytes.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    let mut reader=Inspect{once:Some(||{assert!(f.call(r).is_err());check()}),bytes:Vec::new()};
    z::read_node_to(&f.sys,r.id,f.first.node,&mut reader).unwrap();assert!(reader.once.is_none());assert_eq!(reader.bytes,BEFORE);
    // Even after the old reader drops, offline A is not proof of retirement.
    let offline=f.old.with_file_name("source-offline-during-cleanup");fs::rename(&f.old,&offline).unwrap();
    assert!(f.call(r).is_err());check().unwrap();fs::rename(&offline,&f.old).unwrap();
    let done=f.call(r).unwrap();assert_eq!(done.status,MoveStatus::CompleteCheckedNow);
    assert_eq!(f.receipt(r).decision_sequence,done.decision_sequence);
    assert!(z::list_initial_files(&f.sys,f.to).unwrap().pending_moves.is_empty());
    let other=z::create_empty(&f.sys,create,f.to,"wait-for-move").unwrap();assert_ne!(other.file,x.file);
    let saved=z::save_interval(&f.sys,save,x.file,1,edit,Cursor::new(b"X")).unwrap();assert_eq!(saved.base_node,head.node);
    assert_eq!(z::creation_receipt(&f.sys,create).unwrap().unwrap().file,other);
    assert_eq!(z::interval_receipt(&f.sys,save).unwrap().unwrap().published,Some(saved));
    let mut actual=Vec::new();z::read_current_to(&f.sys,x.file,&mut actual).unwrap();assert_eq!(actual,b"Xutside");
    f.all_history()
}

#[test]
#[ignore="private registered roots on two or three filesystems; hot and cold late conflict for both custody methods"]
fn a_complete_unconfirmed_move_does_not_override_a_late_competing_final_name_in_either_method(){
    for cross in [false,true]{
        let f=Fixture::build(cross,false);let r=f.cmd();let foreign=f.new.join(if cross{"name"}else{"NaMe"});
        let original=inventory_ids(&f.old.join("name"));let mut hit=false;
        let before=crate::pool::lease::LocalPool::with_snapshot(&f.sys,crate::pool::lease::Access::Read).unwrap().2.frontier();
        assert!(perform(&f.sys,r,&mut |cut|{if cut==Cut::FrameSynced{
            let mut claimant=fs::OpenOptions::new().write(true).create_new(true).open(&foreign)?;
            claimant.write_all(b"independent late occupant")?;claimant.sync_all()?;hit=true;
        }Ok(())}).is_err());assert!(hit);
        let meta=fs::metadata(&foreign).unwrap();let identity=(meta.dev(),meta.ino());
        assert_eq!(f.receipt(r).status,MoveStatus::AwaitingDecisionConfirmation);
        assert_eq!(f.receipt(r).decision_sequence,None);assert!(f.call(r).is_err());
        let(_,_,s)=crate::pool::lease::LocalPool::with_snapshot(&f.sys,crate::pool::lease::Access::Read).unwrap();
        assert_eq!(s.frontier(),before);assert!(s.decided(r.op).is_none());assert_eq!(s.placement(r.id).unwrap().volume(),f.from);
        assert_eq!(fs::read(&foreign).unwrap(),b"independent late occupant");assert_eq!(inventory_ids(&f.old.join("name")),original);f.all_history();
        let away=f.new.with_file_name("own-fixture-occupant-relocated");fs::rename(&foreign,&away).unwrap();
        assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);assert_eq!(f.receipt(r).decision_sequence,Some(before.0+1));
        let moved=fs::metadata(&away).unwrap();assert_eq!((moved.dev(),moved.ino()),identity);
        assert_eq!(fs::read(&away).unwrap(),b"independent late occupant");assert!(!f.old.join("name").exists());f.all_history();
    }
}

#[test]
#[ignore="private native roots; a conflict after confirmation cannot undo the sole original placement"]
fn a_conflict_introduced_after_the_confirmed_move_keeps_the_original_target_pending_instead_of_reactivating_the_source(){
    let f=Fixture::build(true,false);let r=f.cmd();let foreign=f.new.join("name");let mut hit=false;
    assert!(perform(&f.sys,r,&mut |c|{if c==Cut::Decision{
        let mut other=fs::OpenOptions::new().write(true).create_new(true).open(&foreign)?;
        other.write_all(b"late after original decision")?;other.sync_all()?;hit=true;
    }Ok(())}).is_err());assert!(hit);
    assert_eq!(f.receipt(r).status,MoveStatus::DecidedTargetPending);assert!(f.old.join("name").is_dir());
    let mut bytes=Vec::new();assert!(z::read_node_to(&f.sys,r.id,f.first.node,&mut bytes).is_err());assert!(bytes.is_empty());
    assert_eq!(fs::read(&foreign).unwrap(),b"late after original decision");assert!(f.call(r).is_err());
    fs::rename(&foreign,f.new.with_file_name("external-owner-kept")).unwrap();assert_eq!(f.call(r).unwrap().status,MoveStatus::CompleteCheckedNow);
    f.all_history();
}
