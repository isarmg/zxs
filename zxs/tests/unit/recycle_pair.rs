//! Opt-in local native source scenarios, not results. Never mount, reformat
//! or fill a filesystem as part of these tests.
use crate::test_support::physical as native_leaf;
use super::*;
use crate::{DeletionKey,RecycleAction as Action,RecycleFilter,RecycleKind,RestoreTarget};
use crate::ordinary::custody::recycle;
use crate::pool::lease::{Access,LocalPool};
fn action(f:&Fixture)->Action{Action::Delete{file:f.file.file,from:f.from}}
fn keep(f:&Fixture,op:OperationId)->DeletionKey{z::apply_recycle(&f.sys,op,0,action(f)).unwrap().deletion.unwrap()}
fn reclaim(key:DeletionKey,to:RestoreTarget)->Action{Action::Restore{deleted:key,target:to}}
fn normal(f:&Fixture,volume:VolumeId,name:&str,body:&[u8])->(FileInfo,FormalNode){
    let x=z::create_empty(&f.sys,op(),volume,name).unwrap();
    z::save_first(&f.sys,op(),x.file,claim(body),Cursor::new(body)).unwrap();
    let node=z::commit_working(&f.sys,op(),x.file,1).unwrap();(x,node)
}
fn read(f:&Fixture,id:crate::FileId,body:&[u8]){
    let mut now=Vec::new();assert_eq!(z::read_current_to(&f.sys,id,&mut now).unwrap().file.file,id);assert_eq!(now,body);
}
fn receipt(f:&Fixture,op:OperationId,epoch:u64,a:Action)->z::RecycleReceipt{
    z::recycle_receipt(&f.sys,op,epoch,a).unwrap().unwrap()
}
#[test]
#[ignore="three private registered filesystems; both directions interrupted at each original publication/cleanup boundary"]
fn delete_and_restore_are_one_exact_original_each_across_independent_cold_retries(){
    let cuts=[Cut::Reservation,Cut::UnboundMkdir,Cut::MoveLeafLinked,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,
        Cut::MarkedBeforeSync,Cut::Decision,Cut::MoveInstalled,Cut::MoveTargetFlagBeforeSync,Cut::MoveTargetChecked,
        Cut::MoveRetireFlagBeforeSync,Cut::MoveSourceAdmitted,Cut::MoveSourceIsolated,Cut::MoveEraseFlagBeforeSync,
        Cut::MoveLeafRetired,Cut::MoveEmptyFlagBeforeSync,Cut::MoveEmptyConfirmed,Cut::MoveDirectoryUnlinked,
        Cut::IntentReleased,Cut::Release];
    for cut in cuts{for restoring in [false,true]{
        let f=Fixture::build(true,false);let del=op();
        let(command,epoch)=if restoring{(reclaim(keep(&f,del),RestoreTarget::Volume(f.to)),1)}else{(action(&f),0)};
        let id=if restoring{op()}else{del};let before=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap().2.frontier().0;
        let mut hit=false;assert!(recycle::apply(&f.sys,id,epoch,command,&mut|c|{
            if c==cut{hit=true;Err(io::Error::other("interrupted at exact requested native step"))}else{Ok(())}
        }).is_err());assert!(hit,"cut did not occur: {cut:?} restoring={restoring}");
        let observed=receipt(&f,id,epoch,command);assert_eq!(observed.operation,id);
        assert!(z::recycle_receipt(&f.sys,id,epoch+1,command).is_err());
        let next=z::apply_recycle(&f.sys,id,epoch,command).unwrap();
        assert!(matches!(next.status,MoveStatus::CompleteCheckedNow|MoveStatus::OriginalPublished));
        assert_eq!(next.decision_sequence,Some(before+1));assert_eq!(next.deleted_at,observed.deleted_at);
        assert_eq!(next.file,f.file.file);assert_eq!(next.history,f.file.history);assert_eq!(next.entry,f.file.entry);
        let after=receipt(&f,id,epoch,command);assert_eq!(after.status,MoveStatus::OriginalPublished);
        assert_eq!(after.decision,next.decision);assert_eq!(after.deletion,next.deletion);
        assert_eq!(LocalPool::with_snapshot(&f.sys,Access::Read).unwrap().2.frontier().0,before+1);
        if restoring{f.all_history();assert!(z::list_retained(&f.sys,RecycleFilter::default(),8,None).unwrap().entries.is_empty());}
        else{assert_eq!(z::list_retained(&f.sys,RecycleFilter::default(),8,None).unwrap().entries[0].original.deletion,next.deletion);
            assert!(z::current_info(&f.sys,next.file).is_err());}
    }}
}
#[test]
#[ignore="three private registered filesystems; same displayed name, frozen pages and same actor's original receipts"]
fn independent_same_name_deletions_page_by_original_sequence_even_if_media_is_offline_or_a_later_cycle_changes(){
    let f=Fixture::build(true,false);let(second,_)=normal(&f,f.to,"name",b"another FILE");let aop=op();let b_op=op();
    let left=keep(&f,aop);let right_action=Action::Delete{file:second.file,from:f.to};
    let right=z::apply_recycle(&f.sys,b_op,0,right_action).unwrap().deletion.unwrap();
    assert_ne!(left.file,right.file);assert_ne!(left.operation,right.operation);
    let all=RecycleFilter::default();
    let first=z::list_retained(&f.sys,all,1,None).unwrap();let next=first.next.unwrap();let serialized=next.to_string();
    assert_eq!(serialized.parse::<z::RecycleCursor>().unwrap(),next);
    assert_eq!(first.entries[0].original.deletion,Some(right));assert_eq!(first.entries[0].original.name,"name");
    let rr=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
    let down=rr.volume.path.with_file_name("unavailable");fs::rename(&rr.volume.path,&down).unwrap();
    let offline=z::list_retained(&f.sys,all,1,Some(&next)).unwrap();assert!(offline.next.is_none());
    assert_eq!(offline.entries[0].original.deletion,Some(left));assert!(offline.entries[0].retained_by_this_deletion_now);
    let only_left=RecycleFilter{ordinary_source:Some(f.from),..all};
    assert!(z::list_retained(&f.sys,only_left,1,Some(&next)).is_err());
    assert_eq!(z::list_retained(&f.sys,only_left,1,None).unwrap().entries[0].original.deletion,Some(left));
    assert!(z::list_retained(&f.sys,all,2,Some(&next)).is_err());
    let mut altered=serialized.into_bytes();let n=altered.len()-1;altered[n]=if altered[n]==b'1'{b'2'}else{b'1'};
    let tampered=String::from_utf8(altered).unwrap().parse().unwrap();
    assert!(z::list_retained(&f.sys,all,1,Some(&tampered)).is_err());
    assert_eq!(receipt(&f,aop,0,action(&f)).status,MoveStatus::OriginalPublished);
    let restore=op();let cmd=reclaim(left,RestoreTarget::Original);
    assert!(z::apply_recycle(&f.sys,restore,1,cmd).is_err());assert!(z::recycle_receipt(&f.sys,restore,1,cmd).unwrap().is_none());
    fs::rename(&down,&rr.volume.path).unwrap();
    z::apply_recycle(&f.sys,restore,1,cmd).unwrap();let again=op();let newer=z::apply_recycle(&f.sys,again,2,action(&f)).unwrap();
    let old_page=z::list_retained(&f.sys,all,1,Some(&next)).unwrap();
    assert_eq!(old_page.as_of_digest,first.as_of_digest);assert_eq!(old_page.entries[0].original.deletion,Some(left));
    assert!(!old_page.entries[0].retained_by_this_deletion_now);
    let current=z::list_retained(&f.sys,all,128,None).unwrap();
    assert_eq!(current.entries.iter().map(|r|r.original.deletion).collect::<Vec<_>>(),[newer.deletion,Some(right)]);
    assert!(current.entries.iter().all(|r|r.original.name=="name"&&r.retained_by_this_deletion_now&&r.original.kind==RecycleKind::Delete));
    assert_eq!(receipt(&f,restore,1,cmd).delete_operation,aop);assert!(z::apply_recycle(&f.sys,op(),3,cmd).is_err());
    let wrong=DeletionKey{file:left.file,..right};assert!(z::apply_recycle(&f.sys,op(),3,reclaim(wrong,RestoreTarget::Original)).is_err());
}
#[test]
#[ignore="private ordinary and dedicated volumes; a pre-admitted historical reader spans an actual deletion"]
fn known_deletion_and_unrelated_reads_do_not_imply_either_old_reads_or_new_writes_are_still_admissible(){
    let f=Fixture::build(true,false);let(other,_)=normal(&f,f.to,"unrelated",b"other");let delete=op();let create=op();let save=op();
    struct Sink<F>{call:Option<F>,bytes:Vec<u8>}
    impl<F:FnOnce()->io::Result<()>> Write for Sink<F>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{if let Some(cb)=self.call.take(){cb()?;}self.bytes.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    let edit=z::SpanEdit::new(0,claim(b"X"),z::LengthPolicy::KeepOrGrow).unwrap();
    let mut sink=Sink{call:Some(||{
        assert!(z::apply_recycle(&f.sys,delete,0,action(&f)).is_err());
        let original=receipt(&f,delete,0,action(&f));assert_eq!(original.status,MoveStatus::TargetReopenedSourcePending);
        let old=z::list_initial_files(&f.sys,f.from)?;assert!(old.files.is_empty()&&old.pending_moves.is_empty());
        assert_eq!(old.pending_lifecycle,[original.clone()]);assert_eq!(original.kind,RecycleKind::Delete);
        let page=z::list_retained(&f.sys,RecycleFilter::default(),4,None)?;assert_eq!(page.entries[0].original,original);
        assert!(z::current_info(&f.sys,f.file.file).is_err());let mut b=Vec::new();
        assert!(z::read_node_range_to(&f.sys,f.file.file,f.first.node,0,1,&mut b).is_err());assert!(b.is_empty());
        read(&f,other.file,b"other");assert!(z::create_empty(&f.sys,create,f.to,"later").is_err());
        assert!(z::save_interval(&f.sys,save,other.file,1,edit,Cursor::new(b"X")).is_err());
        assert!(z::creation_receipt(&f.sys,create)?.is_none()&&z::interval_receipt(&f.sys,save)?.is_none());
        let too_early=op();let put=reclaim(original.deletion.unwrap(),RestoreTarget::Original);
        assert!(z::apply_recycle(&f.sys,too_early,1,put).is_err());assert!(z::recycle_receipt(&f.sys,too_early,1,put)?.is_none());Ok(())
    }),bytes:Vec::new()};
    z::read_node_to(&f.sys,f.file.file,f.first.node,&mut sink).unwrap();assert_eq!(sink.bytes,BEFORE);
    z::apply_recycle(&f.sys,delete,0,action(&f)).unwrap();
    z::create_empty(&f.sys,create,f.to,"later").unwrap();z::save_interval(&f.sys,save,other.file,1,edit,Cursor::new(b"X")).unwrap();read(&f,other.file,b"Xther");
}
#[test]
#[ignore="expensive: fill one real fresh bounded SYSTEM using ordinary creates; needs private scratch capacity for up to 8192 tiny FILEs"]
fn unrelated_native_operations_cannot_consume_the_last_two_real_original_restore_credits(){
    let f=Fixture::build(true,false);let(second,_)=normal(&f,f.to,"name",b"right");
    let left=keep(&f,op());let right=z::apply_recycle(&f.sys,op(),0,Action::Delete{file:second.file,from:f.to}).unwrap().deletion.unwrap();
    for n in 0..=8192{
        // A genuine confirmed prefix and the same predicate the actual business
        // writer uses. Which hard dimension fills first depends on native size;
        // separate pure arithmetic cases in journal/credits.rs exercise BOTH.
        {let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
            if s.room_for(crate::ordinary::wire::MAX_FRAME).is_err(){break;}}
        let vol=if n%2==0{f.from}else{f.to};z::create_empty(&f.sys,op(),vol,&format!("small-{n:04}")).unwrap();
    }
    let foreign=op();assert!(z::create_empty(&f.sys,foreign,f.from,"no-credit").is_err());assert!(z::creation_receipt(&f.sys,foreign).unwrap().is_none());
    let left_receipt=z::apply_recycle(&f.sys,op(),1,reclaim(left,RestoreTarget::Original)).unwrap();assert_eq!(left_receipt.file,left.file);
    assert_eq!(z::list_retained(&f.sys,RecycleFilter::default(),4,None).unwrap().entries[0].original.deletion,Some(right));
    z::apply_recycle(&f.sys,op(),1,reclaim(right,RestoreTarget::Original)).unwrap();f.all_history();read(&f,second.file,b"right");
}

#[test]
#[ignore="requires a preconfigured private separate recycle test quota smaller than 64 MiB; never fills or reconfigures a host disk"]
fn a_tiny_current_node_does_not_substitute_for_the_real_large_historical_full_space_required_at_recycle(){
    use std::io::Read;
    let a=PrivateTemp::under(&std::env::temp_dir());let sys=a.child("system");let data=a.child("ordinary");
    let quota=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_LIMITED_RECYCLE_TEST_TMPDIR").expect("explicit small, isolated quota fixture")));
    let bin=quota.child("recycle");let reg=z::initialize(op(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let v=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;let file=z::create_empty(&sys,op(),v,"formerly-large").unwrap();
    const N:u64=64*1024*1024;let block=[b'a';64*1024];let mut h=z::hash::Sha256::new();
    for _ in 0..N/block.len() as u64{h.update(&block).unwrap();}
    let claim_full=z::DraftClaim::new(N,h.finish()).unwrap();z::save_first(&sys,op(),file.file,claim_full,io::repeat(b'a').take(N)).unwrap();
    let full=z::commit_working(&sys,op(),file.file,1).unwrap();
    z::save_interval(&sys,op(),file.file,1,z::SpanEdit::new(0,claim(b""),z::LengthPolicy::Set(0)).unwrap(),io::empty()).unwrap();
    let tiny=z::commit_working(&sys,op(),file.file,2).unwrap();assert_eq!(tiny.body.bytes,0);
    let id=op();let d=Action::Delete{file:file.file,from:v};let error=z::apply_recycle(&sys,id,0,d).unwrap_err();
    assert!(matches!(error.raw_os_error(),Some(28)|Some(122)),"fixture must actually fail original allocation with ENOSPC/EDQUOT: {error}");
    assert!(z::recycle_receipt(&sys,id,0,d).unwrap().is_none());assert_eq!(fs::read_dir(&bin).unwrap().count(),0);
    assert_eq!(z::current_info(&sys,file.file).unwrap().head,Some(tiny));let mut out=Vec::new();
    z::read_node_range_to(&sys,file.file,full.node,N-1,1,&mut out).unwrap();assert_eq!(out,b"a");
}

#[test]
#[ignore="two independently initialized private SYSTEMs; no invocation may switch registers after resolving selectors"]
fn a_resolved_public_action_cannot_be_replayed_on_another_new_system_before_the_first_reservation(){
    use crate::ordinary::custody::{lifecycle,purpose::Call};
    let f=Fixture::build(false,false);let other=Fixture::build(false,false);let issued=op();
    let call={let(p,_,_)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
        let r=Request{op:issued,call:Call::Delete,id:f.file.file,from:f.from,to:p.recycle_for(f.from).unwrap(),epoch:0};
        lifecycle::Invocation::new(&p,r).unwrap()};
    let(_,_,before)=LocalPool::with_snapshot(&other.sys,Access::Read).unwrap();
    assert!(lifecycle::perform_for(&other.sys,&call,&mut |_|panic!("wrong SYSTEM reached original operation")).is_err());
    let(_,_,after)=LocalPool::with_snapshot(&other.sys,Access::Read).unwrap();
    assert_eq!(before.frontier(),after.frontier());assert!(after.moving.is_none()&&after.pending.is_none()&&after.decided(issued).is_none());
    assert!(z::recycle_receipt(&f.sys,issued,0,action(&f)).unwrap().is_none());f.all_history();other.all_history();
}

#[test]
#[ignore="three private registered native filesystems; F0 and both immutable increments"]
fn move_and_delete_require_even_the_part_of_i1_completely_overwritten_by_i2(){
    use std::os::unix::fs::FileExt;
    let f=Fixture::build(true,false);let id=f.file.file;
    z::save_interval(&f.sys,op(),id,2,z::SpanEdit::new(0,claim(b"X"),z::LengthPolicy::Set(1)).unwrap(),Cursor::new(b"X")).unwrap();
    let i2=z::commit_working(&f.sys,op(),id,3).unwrap();
    let n=match &f.last.kind{z::FormalStorage::Incremental{changed,..}=>changed.bytes,_=>panic!("expected actual INCR")};assert!(n>0&&n<z::DRAFT_CHUNK as u64);
    let path=f.old.join(native_leaf("name",f.file.file)).join(format!("{}-name",f.last.storage));
    let pack=fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let at=pack.metadata().unwrap().len()-32-n;let mut original=[0];pack.read_exact_at(&mut original,at).unwrap();
    pack.write_all_at(&[original[0]^1],at).unwrap();pack.sync_all().unwrap();
    let mut visible=Vec::new();assert_eq!(z::read_node_to(&f.sys,id,i2.node,&mut visible).unwrap(),i2);assert_eq!(visible,b"X");
    let place=op();let del=op();assert!(z::move_file(&f.sys,place,id,f.from,f.to,0).is_err());assert!(z::apply_recycle(&f.sys,del,0,action(&f)).is_err());
    assert!(z::move_receipt(&f.sys,place,id,f.from,f.to,0).unwrap().is_none());assert!(z::recycle_receipt(&f.sys,del,0,action(&f)).unwrap().is_none());
    assert!(fs::read_dir(&f.new).unwrap().next().is_none());assert_eq!(fs::read_dir(f.old.join(native_leaf("name",f.file.file))).unwrap().count(),5);
    pack.write_all_at(&original,at).unwrap();pack.sync_all().unwrap();drop(pack);
    assert_eq!(z::move_file(&f.sys,place,id,f.from,f.to,0).unwrap().status,MoveStatus::CompleteCheckedNow);
    assert!(!f.old.join(native_leaf("name",f.file.file)).exists()&&fs::read_dir(f.new.join(native_leaf("name",f.file.file))).unwrap().count()==5);
    for(node,expected)in[(f.first.node,BEFORE),(f.last.node,AFTER),(i2.node,b"X".as_slice())]{
        let mut out=Vec::new();z::read_node_to(&f.sys,id,node,&mut out).unwrap();assert_eq!(out,expected);
    }
}

#[test]
#[ignore="three private filesystems; original signed fifth unlink is not confirmed-empty in either transfer direction"]
fn five_actual_roles_stop_before_the_last_unlink_and_never_adopt_a_replacement_as_the_empty_source(){
    use crate::ordinary::custody::allocation::Progress;
    use std::os::unix::ffi::OsStrExt;
    for restore in [false,true]{
        let f=Fixture::build(true,false);let id=f.file.file;
        z::save_interval(&f.sys,op(),id,2,z::SpanEdit::new(1,claim(b"X"),z::LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"X")).unwrap();
        let i2=z::commit_working(&f.sys,op(),id,3).unwrap();let mut wanted=AFTER.to_vec();wanted[1]=b'X';
        let(call,epoch)=if restore{(reclaim(keep(&f,op()),RestoreTarget::Volume(f.to)),1)}else{(action(&f),0)};
        let operation=op();let mut issued=0;
        assert!(recycle::apply(&f.sys,operation,epoch,call,&mut|step|{
            if step==Cut::MoveEraseFlagBeforeSync{issued+=1;if issued==5{return Err(io::Error::other("last original leaf permitted, not empty"));}}Ok(())
        }).is_err());assert_eq!(issued,5);
        let source=if restore{z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap().volume.path}else{f.old.clone()};
        let q={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let(m,a)=s.confirmed_handoff(operation).unwrap();
            assert_eq!(a.progress(),Progress::leaf(4).unwrap());assert_ne!(a.progress(),Progress::EMPTY);
            assert_eq!(m.original.role_count(),5);assert_eq!(m.target.children.len(),2);
            source.join(std::ffi::OsStr::from_bytes(m.original.quarantine_name().to_bytes()))};
        assert_eq!(fs::read_dir(&q).unwrap().count(),1);let actual=q.join("FILE");let same=fs::read(&actual).unwrap();
        let away=source.parent().unwrap().join("held-real-fifth-role");fs::rename(&actual,&away).unwrap();
        fs::write(&actual,&same).unwrap();let original=fs::metadata(&away).unwrap();
        fs::set_permissions(&actual,original.permissions()).unwrap();
        let wrong=fs::metadata(&actual).unwrap().ino();assert_ne!(wrong,original.ino());
        assert!(z::apply_recycle(&f.sys,operation,epoch,call).is_err());
        assert_eq!(fs::metadata(&actual).unwrap().ino(),wrong);assert_eq!(fs::read(&actual).unwrap(),same);assert!(away.exists());
        let still=receipt(&f,operation,epoch,call);assert_eq!(still.status,MoveStatus::TargetReopenedSourcePending);
        fs::rename(&actual,source.parent().unwrap().join("unclaimed-substitute")).unwrap();fs::rename(&away,&actual).unwrap();
        assert_eq!(z::apply_recycle(&f.sys,operation,epoch,call).unwrap().status,MoveStatus::CompleteCheckedNow);assert!(!q.exists());
        if !restore{
            let key=receipt(&f,operation,epoch,call).deletion.unwrap();
            z::apply_recycle(&f.sys,op(),1,reclaim(key,RestoreTarget::Volume(f.to))).unwrap();
        }
        for(node,expected)in[(f.first.node,BEFORE),(f.last.node,AFTER),(i2.node,wanted.as_slice())]{
            let mut out=Vec::new();z::read_node_to(&f.sys,id,node,&mut out).unwrap();assert_eq!(out,expected);
        }
    }
}

#[test]
#[ignore="actual private ordinary and dedicated roots; a skipped transfer slot is never cleanup authority"]
fn an_occupied_lower_slot_blocks_the_confirmation_frontier_in_both_recycle_directions(){
    for restore in [false,true]{
        let f=Fixture::build(true,false);let r=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|v|v.volume.role==z::Role::Recycle).unwrap();
        let(call,epoch,target)=if restore{(reclaim(keep(&f,op()),RestoreTarget::Volume(f.to)),1,&f.new)}else{(action(&f),0,&r.volume.path)};
        let operation=op();let floor=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap().2.frontier();
        assert!(recycle::apply(&f.sys,operation,epoch,call,&mut|c|if c==Cut::UnboundMkdir{Err(io::Error::other("slot zero still unbound"))}else{Ok(())}).is_err());
        let slot={let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();
            let a=s.moving.as_ref().unwrap();assert!(a.stage().is_none());target.join(format!(".zxs-new-{}-0",a.original.ticket))};
        let sentinel=slot.join("outside");let mut inside=false;
        assert!(recycle::apply(&f.sys,operation,epoch,call,&mut|c|{
            if c==Cut::FrameSynced{fs::write(&sentinel,b"not mine")?;inside=true;}Ok(())
        }).is_err());assert!(inside);assert!(z::apply_recycle(&f.sys,operation,epoch,call).is_err());
        {let(_,_,s)=LocalPool::with_snapshot(&f.sys,Access::Read).unwrap();let a=s.moving.as_ref().unwrap();
            assert_eq!(s.frontier(),floor);assert_eq!(a.stage().unwrap().slot,1);assert!(s.decided(operation).is_none());}
        let preserved=target.with_file_name("held-unclaimed-sentinel");fs::rename(&sentinel,&preserved).unwrap();
        assert_eq!(z::apply_recycle(&f.sys,operation,epoch,call).unwrap().status,MoveStatus::CompleteCheckedNow);
        assert_eq!(fs::read(preserved).unwrap(),b"not mine");assert!(slot.is_dir());
    }
}

#[test]
#[ignore="three private filesystems; exact original return credit and the 4096-byte destination boundary"]
fn a_too_long_restoration_leaf_never_consumes_the_retained_file_or_its_original_return_credit(){
    use crate::ordinary::journal::credits::{Deletion,Effect};
    const NAME:&str="abcdefghijklmnopqrst";
    for width in [4042,4043]{
        let f=Fixture::planned(true,false,None,Some(width));let(file,node)=normal(&f,f.from,NAME,b"retained original");
        let physical=native_leaf(NAME,file.file);assert_eq!(physical.as_bytes().len(),53);
        let deletion=op();let key=z::apply_recycle(&f.sys,deletion,0,Action::Delete{file:file.file,from:f.from}).unwrap().deletion.unwrap();
        let bin=z::inspect(&f.sys).unwrap().volumes.into_iter().find(|r|r.volume.role==z::Role::Recycle).unwrap().volume.path;
        let kept=bin.join(&physical);let original=fs::metadata(&kept).unwrap();
        let cut=||LocalPool::with_snapshot(&f.sys,Access::Read).unwrap().2;
        let before=cut().frontier();let issued=op();let requested=reclaim(key,RestoreTarget::Volume(f.to));
        let actual=z::apply_recycle(&f.sys,issued,1,requested);
        if width==4042{
            assert_eq!(width+1+physical.len(),4096);assert_eq!(actual.unwrap().status,MoveStatus::CompleteCheckedNow);
            assert_eq!(z::current_info(&f.sys,file.file).unwrap().file.volume,f.to);
        }else{
            assert_eq!(width+1+physical.len(),4097);assert!(actual.is_err());assert!(z::recycle_receipt(&f.sys,issued,1,requested).unwrap().is_none());
            let s=cut();assert_eq!(s.frontier(),before);assert!(s.moving.is_none()&&s.pending.is_none()&&s.decided(issued).is_none());
            let credit=Deletion{file:key.file,operation:key.operation,decision:key.decision};
            s.room_for_effect(crate::ordinary::wire::MAX_FRAME,Effect::Restore(credit)).unwrap();
            let page=z::list_retained(&f.sys,RecycleFilter::default(),4,None).unwrap();
            assert_eq!(page.entries.len(),1);assert!(page.entries[0].retained_by_this_deletion_now);
            assert_eq!(page.entries[0].original.deletion,Some(key));assert!(page.entries[0].pending_restore.is_none());
            assert_eq!(fs::metadata(&kept).unwrap().ino(),original.ino());assert!(fs::read_dir(&f.new).unwrap().next().is_none());
            assert!(!f.sys.join("FILE-ALLOCATION").exists()&&!f.sys.join("FILE-INTENT").exists());drop(s);
            assert_eq!(z::apply_recycle(&f.sys,op(),1,reclaim(key,RestoreTarget::Original)).unwrap().status,MoveStatus::CompleteCheckedNow);
            assert_eq!(z::current_info(&f.sys,file.file).unwrap().file.volume,f.from);
        }
        assert!(!kept.exists());let mut bytes=Vec::new();assert_eq!(z::read_node_to(&f.sys,file.file,node.node,&mut bytes).unwrap(),node);
        assert_eq!(bytes,b"retained original");
    }
}
