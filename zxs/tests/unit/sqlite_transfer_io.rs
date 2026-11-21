//! Actual named source/target FILEs on separately mounted fixture roots.
use super::*;
use crate::{fs::invalid,ordinary::portable_birth::Birth,system::reader::{InitialReader,smb::SmbSession,tests::transfer_pool},
    test_support::PrivateTemp};
use std::{path::Path,fs::OpenOptions,os::unix::fs::{DirBuilderExt,FileExt,OpenOptionsExt}};

#[test]
#[ignore="source-only: same pool, two genuine private tmpfs/backing paths, no Samba listener or existing data"]
fn another_ordinary_filesystem_selects_the_same_history_then_retires_only_the_old_source(){
    use crate::system::reader::{account::share_mode::Mode,tests::ordinary_transfer_pool};
    use crate::fs::directory_birth::DirectoryParent::Directory;
    let private=PrivateTemp::under(Path::new("/tmp"));let remote=PrivateTemp::under(Path::new("/dev/shm"));
    let destination=remote.0.join("ordinary-target");
    let(s,p,f,r,z1,z2)=ordinary_transfer_pool(&private,&destination);
    let src=InitialReader::open(&s,p).unwrap();let other=InitialReader::open(&s,p).unwrap();
    let auth=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let from=auth.connect(z1).unwrap();let target=auth.connect(z2).unwrap();
    let web=other.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let original_target=web.share_setting(z2).unwrap();
    assert!(HeldDir::trusted_storage(&f).unwrap().distinct_filesystems(&HeldDir::trusted_storage(&destination).unwrap()).unwrap());
    assert!(!r.exists()); // R is not necessary or silently used by this action.
    let (_,target_root)=target.ordinary_route(&[],true).unwrap();
    let op=|n|OperationId([n;16]);
    let (dir,_)=target.make_ordinary_directory(&target_root,op(0xa0),"Reviewed",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let to=target.scoped(Directory(dir.id));
    let file=from.create_file(op(0xa1),"Original.txt").unwrap().file;
    let input=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(private.0.join("changed")).unwrap();
    input.write_all_at(b"Old bytes",0).unwrap();input.sync_all().unwrap();
    from.open_writer("Original.txt",file).unwrap().save(op(0xa2),None,Some(0),9,&input).unwrap();
    let frozen=web.freeze(op(0xa3),web.manual_target(file).unwrap()).unwrap();
    let mut source=from.open_current("Original.txt").unwrap();let key=source.native_key().unwrap();
    let before=source.capacity_cut().unwrap();
    input.write_all_at(b"New bytes",0).unwrap();input.sync_all().unwrap();
    from.open_writer("Original.txt",file).unwrap().save(op(0xa4),Some(op(0xa2)),Some(0),9,&input).unwrap();
    let hold=source.begin_ordinary_move().unwrap();assert!(before.require_owner().is_err());
    let ownership=source.move_owner();assert!(ownership.is_member(&source));ownership.check().unwrap();
    let another_open=from.open_current("Original.txt").unwrap();
    assert!(!ownership.is_member(&another_open));drop(another_open);
    drop(before);
    let old=f.join(format!("Original-{file}.txt"));assert!(old.is_dir());
    let request=Handoff::MoveOrdinary{source:hold.place.clone(),target:"Arrival.txt".into()};
    let fresh=|target:&Tree<'_,'_>,run:&Handoff,new,point:&mut dyn FnMut(Cut)->io::Result<()>,select:&dyn Fn()->io::Result<()>|{
        let opened=OpenedOrdinaryMove{original:&hold.physical,select};
        from.with_transfer(Some(target),|at|{
            let (selected,source_route,target_route)=src.at(|db,c|Ok((Route::select(&at,db,&c,&request)?,
                DirectoryRoute::at(db,&c,from.setting.volume,from.parent)?,
                DirectoryRoute::at(db,&c,target.setting.volume,target.parent)?)))?;
            let start=TransferStart::Fresh(FreshOrdinaryOp{op:new,selected,from:source_route,to:target_route});
            at.transfer_started(run,start,Some(opened),||Ok(()),false,point)
        })
    };
    // A fresh engine start cannot reuse an existing Op even if it belongs
    // to a different operation and there is as yet no pending FILE move.
    assert_eq!(fresh(&to,&request,op(0xa0),&mut |_|panic!("already used ID started copy"),&||panic!("selected reused ID"))
        .unwrap_err().kind(),io::ErrorKind::WouldBlock);
    let changed=Handoff::MoveOrdinary{source:hold.place.clone(),target:"Different.txt".into()};
    assert_eq!(fresh(&to,&changed,op(0xae),&mut |_|panic!("changed target started copy"),&||panic!("selected changed target"))
        .unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    let mut seen=false;
    let fail=fresh(&to,&request,op(0xa5),&mut |cut|{
        if cut!=Cut::CandidateSynced{return Ok(());}
        seen=true;
        assert_eq!(other.at(|db,c|Place::current(db,&c,file))?,hold.place);
        web.set_share_mode(op(0xa6),z2,original_target.op,Mode::ReadOnly)?;
        Ok(())
    },&||Ok(()));
    assert!(seen&&fail.is_err());assert_eq!(source.native_key().unwrap(),key);
    assert!(auth.connect(z2).unwrap().scoped(Directory(dir.id)).open_current("Arrival.txt").is_err());
    web.set_share_mode(op(0xa7),z2,op(0xa6),Mode::ReadWrite).unwrap();
    let refreshed=auth.connect(z2).unwrap().scoped(Directory(dir.id));
    // No new owner can clear or adopt the old reservation even after the
    // target's old authorization is superseded. The source is still selected.
    assert_eq!(fresh(&refreshed,&request,op(0xac),&mut |_|panic!("adopted revoked pending reservation"),&||panic!("selected old Op"))
        .unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert!(from.transfer_ordinary_to(&refreshed,&hold,"Arrival.txt".into(),op(0xa5),||Ok(())).is_err());
    // Refuse just the final selection. This source fixture inspects the
    // authority only after the hook returns, not from inside the SQL cut.
    let captured=std::cell::Cell::new(false);let decisions=std::cell::Cell::new(0);
    let vetoed=from.ordinary_after(&refreshed,&hold,"Arrival.txt".into(),op(0xa8),|cut|{
        assert_eq!(decisions.get(),0);
        if cut==Cut::GuardCaptured{captured.set(true);}
        Ok(())
    },||{
        assert!(captured.get());decisions.set(decisions.get()+1);
        Err(io::ErrorKind::Interrupted.into())
    });
    assert_eq!(vetoed.unwrap_err().kind(),io::ErrorKind::Interrupted);
    assert_eq!(decisions.get(),1);
    other.at(|db,c|{
        assert_eq!(Place::current(db,&c,file)?,hold.place);
        let q=db.prepare(c"SELECT state,EXISTS(SELECT 1 FROM main.file_transfer_source WHERE transfer_op=?1)
                           FROM main.operation WHERE op_id=?1 AND kind=12")?;
        q.bind_bytes(1,&op(0xa8).0)?;
        assert!(work_record::start_one(&q)?);assert_eq!((q.byte(0)?,q.byte(1)?),(0,0));q.done()
    }).unwrap();
    assert!(old.is_dir()&&refreshed.open_current("Arrival.txt").is_err());
    assert!(destination.join("Reviewed").join(Birth::placement_parent(op(0xa8)).unwrap().into_string().unwrap()).is_dir());
    // A fresh call cannot adopt a still-live reservation either, even
    // with the exact target spelling or its old Op. This is tested before
    // the separate explicit legacy-policy continuation below.
    for next in [op(0xa8),op(0xad)]{
        assert_eq!(fresh(&refreshed,&request,next,&mut |_|panic!("adopted live pending reservation"),&||panic!("selected old Op"))
            .unwrap_err().kind(),io::ErrorKind::WouldBlock);
    }
    let moved=from.transfer_ordinary_to(&refreshed,&hold,"Arrival.txt".into(),op(0xa8),||Ok(())).unwrap();
    assert_eq!((moved.to.file,moved.to.parent,moved.to.role),(file,Directory(dir.id),Role::File));
    assert_eq!(from.transfer_ordinary_to(&refreshed,&hold,"Arrival.txt".into(),op(0xa8),||Ok(())).unwrap(),moved);
    assert!(from.open_current("Original.txt").is_err());
    let body=refreshed.open_current("Arrival.txt").unwrap();assert_eq!(body.native_key().unwrap(),key);
    let mut now=Vec::new();body.range_to(0,9,&mut now).unwrap();assert_eq!(now,b"New bytes");
    let mut past=Vec::new();refreshed.open_version("Arrival.txt",frozen.node).unwrap().range_to(0,9,&mut past).unwrap();
    assert_eq!(past,b"Old bytes");
    // The original open continues to see its exact earlier selected view;
    // original source permissions no longer control use of the new target.
    let adoption=refreshed.prepare_moved_file(moved.clone(),&hold).unwrap();
    assert_eq!(source.adopt_moved_file(adoption).unwrap(),moved);
    assert!(ownership.check().is_err());
    web.set_share_mode(op(0xa9),z1,from.setting.op,Mode::Disabled).unwrap();
    let mut prior=Vec::new();source.range_to(0,9,&mut prior).unwrap();assert_eq!(prior,b"Old bytes");
    assert_eq!(source.native_key().unwrap(),key);assert_eq!(source.grant().volume,target.setting.volume);
    let new_cut=source.capacity_cut().unwrap();new_cut.physical().unwrap();assert_eq!(new_cut.setting.volume,target.setting.volume);
    let final_path=destination.join("Reviewed").join(Birth::placement_parent(moved.op).unwrap().into_string().unwrap())
        .join(format!("Arrival-{file}.txt"));
    assert!(final_path.is_dir()&&!r.exists());
    assert_eq!(super::super::retirement::one_for_test(&other,|_|Ok(())).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    web.set_share_mode(op(0xaa),z2,refreshed.setting.op,Mode::Disabled).unwrap();
    web.set_share_mode(op(0xab),z2,op(0xaa),Mode::ReadWrite).unwrap();
    let new_grant=auth.connect(z2).unwrap().scoped(Directory(dir.id));
    assert!(new_grant.prepare_moved_file(moved.clone(),&hold).is_err());
    assert!(source.native_key().is_err());
    assert!(old.is_dir());drop(hold);
    super::super::retirement::one_for_test(&other,|_|Ok(())).unwrap();
    assert!(!old.exists()&&final_path.is_dir()&&!r.exists());
}

#[test]
#[ignore="source-only: two original ordinary roots on the same live filesystem; no SMB listener"]
fn do_not_create_a_copy_ticket_for_two_registrations_of_one_mounted_filesystem(){
    use crate::system::reader::tests::ordinary_transfer_pool;
    let a=PrivateTemp::under(Path::new("/tmp"));let b=PrivateTemp::under(Path::new("/tmp"));
    let(s,p,f,_,z1,z2)=ordinary_transfer_pool(&a,&b.0.join("not-a-different-filesystem"));
    let src=InitialReader::open(&s,p).unwrap();let auth=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let from=auth.connect(z1).unwrap();let to=auth.connect(z2).unwrap();
    let file=from.create_file(OperationId([0x94;16]),"Document").unwrap().file;
    let source=from.open_current("Document").unwrap().move_source().unwrap();
    let answer=from.transfer_ordinary_to(&to,&source,"Other".into(),OperationId([0x95;16]),||Ok(()));
    assert_eq!(answer.unwrap_err().kind(),io::ErrorKind::Unsupported);
    src.at(|db,c|{
        assert_eq!(Place::current(db,&c,file)?,source.place);
        let q=db.prepare(c"SELECT count(*) FROM main.file_transfer")?;
        assert!(work_record::start_one(&q)?);assert_eq!(q.integer(0)?,0);q.done()
    }).unwrap();
    assert!(f.join(format!("Document-{file}")).is_dir());
}

#[test]
#[ignore="source-only: actual allowed single-name interrupted SAVE, two isolated ordinary filesystems"]
fn unprovable_old_source_stops_before_a_move_ticket_or_target_stage(){
    use crate::system::reader::{tests::ordinary_transfer_pool,smb::work_change::Cut as WorkCut};
    let local=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
    let dest=other.0.join("ordinary");let(s,p,f,_,a,b)=ordinary_transfer_pool(&local,&dest);
    let src=InitialReader::open(&s,p).unwrap();let witness=InitialReader::open(&s,p).unwrap();
    let login=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let from=login.connect(a).unwrap();let to=login.connect(b).unwrap();
    let web=witness.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let op=|n|OperationId([n;16]);let name="Report";
    let file=from.create_file(op(0x70),name).unwrap().file;
    let payload=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600).open(local.0.join("payload")).unwrap();
    payload.write_all_at(b"ABCD",0).unwrap();payload.sync_all().unwrap();
    let initial=from.open_writer(name,file).unwrap().save(op(0x71),None,Some(0),4,&payload).unwrap();
    web.freeze(op(0x72),web.manual_target(file).unwrap()).unwrap();
    payload.write_all_at(b"Z",0).unwrap();payload.sync_all().unwrap();
    let left=op(0x73);let finished=op(0x74);
    let container=f.join(Birth::placed_container(name,file).unwrap().to_str().unwrap());
    let [unpaired,_,_,_]=crate::ordinary::working_round::names(left);
    let lonely=container.join(unpaired.to_str().unwrap());
    let mut left_name=false;
    let staged=from.open_writer(name,file).unwrap().save_after(left,Some(initial.key.save),Some(0),4,&payload,|cut|{
        if cut!=WorkCut::Reserved{return Ok(());}
        let piece=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&lonely)?;
        piece.write_all_at(b"?",0)?;piece.sync_all()?;std::fs::File::open(&container)?.sync_all()?;
        left_name=true;Err(io::ErrorKind::ConnectionAborted.into())
    });
    assert!(left_name&&staged.is_err());
    let working=from.open_writer(name,file).unwrap().save(finished,Some(initial.key.save),Some(0),4,&payload).unwrap();
    witness.at(|db,_|{
        let q=db.prepare(c"SELECT retained_mask FROM main.work_edit WHERE edit_op=?1")?;
        q.bind_bytes(1,&finished.0)?;assert!(work_record::start_one(&q)?);assert_eq!(q.byte(0)?,1);q.done()
    }).unwrap();
    let opened=from.open_current(name).unwrap().move_source().unwrap();let token=op(0x79);
    let rejected=from.transfer_ordinary_to(&to,&opened,"No copy yet".into(),token,||Ok(())).unwrap_err();
    assert_eq!(rejected.to_string(),"old FILE retirement cannot prove the original isolated object");
    witness.at(|db,c|{
        assert_eq!(Place::current(db,&c,file)?,opened.place);
        let q=db.prepare(c"SELECT count(*) FROM main.file_transfer")?;
        assert!(work_record::start_one(&q)?);assert_eq!(q.integer(0)?,0);q.done()
    }).unwrap();
    assert!(!dest.join(Birth::placement_parent(token).unwrap().to_str().unwrap()).exists());
    drop(opened);
    payload.write_all_at(b"Y",0).unwrap();payload.sync_all().unwrap();
    from.open_writer(name,file).unwrap().save(op(0x7a),Some(working.key.save),Some(0),4,&payload).unwrap();
    assert_eq!(std::fs::read(&lonely).unwrap(),b"?");
}

#[test]
#[ignore="source only: exact nested ordinary storage, actual private F/R and real SQLite; no SMB listener"]
fn nested_handoff_keeps_original_parents_and_the_same_confirmed_work(){
    let private=PrivateTemp::under(Path::new("/tmp"));let remote=PrivateTemp::under(Path::new("/dev/shm"));
    let recycle=remote.0.join("actual-R");let (s,p,f,rw)=transfer_pool(&private,&recycle);
    let src=InitialReader::open(&s,p).unwrap();
    let auth=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let top=auth.connect(rw).unwrap();let (_,r)=top.ordinary_route(&[],true).unwrap();
    let op=|n|OperationId([n;16]);
    let (a,_)=top.make_ordinary_directory(&r,op(0xe0),"Draft",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let (b,_)=top.make_ordinary_directory(&r,op(0xe1),"Restored",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    use crate::fs::directory_birth::DirectoryParent::Directory;
    let old=top.scoped(Directory(a.id));let target=top.scoped(Directory(b.id));
    let id=old.create_file(op(0xe2),"Report.txt").unwrap().file;
    let data=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(private.0.join("payload")).unwrap();
    data.write_all_at(b"reviewed",0).unwrap();data.sync_all().unwrap();
    let writer=old.open_writer("Report.txt",id).unwrap();
    writer.save(op(0xe3),None,Some(0),8,&data).unwrap();drop(writer);
    let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let historical=web.freeze(op(0xe4),web.manual_target(id).unwrap()).unwrap();
    let slot=src.at(|db,c|Place::current(db,&c,id)).unwrap();
    let handoff=Handoff::Delete(slot.clone());
    assert_eq!(slot.parent,Directory(a.id));
    assert!(top.transfer(&handoff,op(0xe5),||Ok(())).is_err());
    let del=old.transfer(&handoff,op(0xe5),||Ok(())).unwrap();
    assert_eq!(del.to.file,id);
    assert!(old.open_current("Report.txt").is_err());
    let same=Handoff::RestoreBound{source:del.to.clone(),target:"Resume.txt".into()};
    let restored=target.transfer(&same,op(0xe6),||Ok(())).unwrap();
    assert_eq!(restored.to.parent,Directory(b.id));
    assert_eq!(target.transfer(&same,op(0xe6),||Ok(())).unwrap(),restored);
    assert!(top.transfer(&same,op(0xe6),||Ok(())).is_err()); // not the same original target
    let mut bytes=Vec::new();target.open_version("Resume.txt",historical.node).unwrap().range_to(0,8,&mut bytes).unwrap();
    assert_eq!(bytes,b"reviewed");assert!(top.open_current("Resume.txt").is_err());
    let physical=f.join("Restored").join(Birth::placement_parent(op(0xe6)).unwrap().into_string().unwrap());
    assert!(physical.join(format!("Resume-{id}.txt/FILE")).is_file());
    let next=InitialReader::open(&s,p).unwrap();
    super::super::retirement::one_for_test(&next,|_|Ok(())).unwrap();
    assert!(!f.join("Draft").join(format!("Report-{id}.txt")).exists());
    assert!(physical.join(format!("Resume-{id}.txt/FILE")).is_file());
}

#[test]
#[ignore="real SQLite and two separate non-root fixture filesystems; no SMB process or existing disk"]
fn durable_transfer_preserves_the_confirmed_file_history_and_allows_actual_restore() {
    let private=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
    let recycle=other.0.join("actual-R");let (s,p,f,rw)=transfer_pool(&private,&recycle);
    let src=InitialReader::open(&s,p).unwrap();let same=InitialReader::open(&s,p).unwrap();
    let account=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();let tree=account.connect(rw).unwrap();
    let admin=same.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let op=|n|OperationId([n;16]);
    let file=tree.create_file(op(0xb0),"Report.txt").unwrap().file;
    let input=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(private.0.join("input")).unwrap();
    input.write_all_at(b"K",0).unwrap();input.sync_all().unwrap();
    let writer=tree.open_writer("Report.txt",file).unwrap();
    writer.save(op(0xb1),None,Some(2),4,&input).unwrap();drop(writer);
    let frozen=admin.freeze(op(0xb2),admin.manual_target(file).unwrap()).unwrap();
    input.write_all_at(b"B",0).unwrap();input.sync_all().unwrap();
    let writer=tree.open_writer("Report.txt",file).unwrap();
    writer.save(op(0xb3),Some(op(0xb1)),Some(0),4,&input).unwrap();drop(writer);
    let select=||src.at(|db,c|Place::current(db,&c,file)).unwrap();
    let count=||same.at(|db,_|{
        let q=db.prepare(c"SELECT count(*) FROM main.operation WHERE kind=12")?;
        assert!(work_record::start_one(&q)?);let n=q.integer(0)?;q.done()?;Ok(n)
    }).unwrap();
    let original=select();let intent=Handoff::Delete(original.clone());
    let away=other.0.join("offline-R");std::fs::rename(&recycle,&away).unwrap();
    assert!(tree.transfer(&intent,op(0xb4),||Ok(())).is_err());assert_eq!(count(),0);assert_eq!(select(),original);
    std::fs::rename(&away,&recycle).unwrap();
    let mut reached=false;
    let cand=tree.transfer_after(&intent,op(0xb4),||Ok(()),|cut|{
        if cut!=Cut::CandidateSynced{return Ok(());}
        reached=true;
        assert_eq!(select(),original);assert_eq!(count(),1);
        let mut old=Vec::new();tree.open_current("Report.txt").unwrap().range_to(0,4,&mut old).unwrap();
        assert_eq!(old,b"B\0K\0");
        Err(invalid("synthetic interrupted before selecting the synced target"))
    });
    assert!(cand.is_err()&&reached);
    let pocket=recycle.join(Birth::placement_parent(op(0xb4)).unwrap().into_string().unwrap());
    let body=pocket.join(format!("Report-{file}.txt"));
    let [raw,mark,_,_]=crate::ordinary::working_round::names(op(0xb1));
    let raw=body.join(raw.into_string().unwrap());let mark=body.join(mark.into_string().unwrap());
    let late_target=tree.transfer_after(&intent,op(0xb5),||Ok(()),|cut|{
        if cut!=Cut::GuardCaptured{return Ok(());}
        let replacement=other.0.join("late-original");
        let replacement_mark=other.0.join("late-witness");
        std::fs::rename(&raw,&replacement)?;std::fs::rename(&mark,&replacement_mark)?;
        let other=OpenOptions::new().write(true).create_new(true).mode(0o600).open(&raw)?;
        other.write_all_at(b"K",0)?;other.sync_all()?;drop(other);std::fs::hard_link(&raw,&mark)?;
        Ok(())
    });
    assert!(late_target.is_err()&&other.0.join("late-original").is_file());
    assert_eq!(select(),original);assert_eq!(count(),1);
    same.at(|db,_|{
        let q=db.prepare(c"SELECT 1 FROM main.file_transfer_source WHERE transfer_op=?1")?;
        q.bind_bytes(1,&op(0xb4).0)?;assert!(!work_record::start_one(&q)?);Ok(())
    }).unwrap();
    std::fs::remove_file(&raw).unwrap();std::fs::remove_file(&mark).unwrap();
    std::fs::rename(other.0.join("late-original"),&raw).unwrap();
    std::fs::rename(other.0.join("late-witness"),&mark).unwrap();
    let true_raw=other.0.join("before-raw");let true_mark=other.0.join("before-mark");
    std::fs::rename(&raw,&true_raw).unwrap();std::fs::rename(&mark,&true_mark).unwrap();
    let unrelated=OpenOptions::new().write(true).create_new(true).mode(0o600).open(&raw).unwrap();
    unrelated.write_all_at(b"Q",0).unwrap();unrelated.sync_all().unwrap();drop(unrelated);
    std::fs::hard_link(&raw,&mark).unwrap();
    assert!(tree.transfer(&intent,op(0xb5),||Ok(())).is_err());
    assert_eq!(std::fs::read(&raw).unwrap(),b"Q");assert_eq!(select(),original);
    std::fs::remove_file(&raw).unwrap();std::fs::remove_file(&mark).unwrap();
    std::fs::rename(&true_raw,&raw).unwrap();std::fs::rename(&true_mark,&mark).unwrap();
    for realm in [&pocket,&body]{
        let late=realm.join("foreign");
        let mut injected=false;
        let stopped=tree.transfer_after(&intent,op(0xb5),||Ok(()),|cut|{
            if cut==Cut::CandidateSynced{
                std::fs::DirBuilder::new().mode(0o700).create(&late)?;
                injected=true;
            }
            Ok(())
        });
        assert!(stopped.is_err()&&injected);assert_eq!(select(),original);
        std::fs::remove_dir(&late).unwrap();
    }
    // Native has selected B as the last CLOSE, but the removed A's
    // actual storage writer has not yet released this FILE's OFD.
    let other_session=SmbSession::fixture_authenticated(&same,"first.admin").unwrap();
    let other_tree=other_session.connect(rw).unwrap();
    let mut draining=Some(other_tree.open_writer("Report.txt",file).unwrap());
    let mut waited=false;
    let deleted=tree.with_transfer(None,|at|at.transfer_inner(&intent,op(0xb5),None,||Ok(()),true,|cut|{
        if cut==Cut::WaitingForSource{
            assert!(!waited);assert_eq!(select(),original);assert_eq!(count(),1);
            waited=true;drop(draining.take().expect("original separate writer still holds FILE EX"));
        }
        Ok(())
    })).unwrap();assert!(waited);assert_eq!(deleted.op,op(0xb4));
    assert_eq!(select(),deleted.to);assert_eq!(count(),1);
    assert!(tree.open_current("Report.txt").is_err());
    assert_eq!(tree.transfer(&intent,deleted.op,||Ok(())).unwrap(),deleted); // previous source no longer current
    let private_leaf=Birth::placement_parent(deleted.op).unwrap().into_string().unwrap();
    assert!(recycle.join(private_leaf).join(format!("Report-{file}.txt")).is_dir());
    let occupied=f.join("Native.txt");std::fs::DirBuilder::new().mode(0o700).create(&occupied).unwrap();
    assert!(tree.transfer(&Handoff::Restore{deleted:deleted.op,name:"Report.txt".into(),target:"Native.txt".into()},op(0xb6),||Ok(())).is_err());
    assert_eq!(count(),1);assert_eq!(select(),deleted.to);
    let retained=account.retained().unwrap();
    assert_eq!(retained.page(None,16).unwrap().iter().map(|r|r.op).collect::<Vec<_>>(),vec![deleted.op]);
    assert_eq!(retained.deletion(deleted.op).unwrap().name,"Report.txt");
    assert!(retained.open(deleted.op,"report.txt",super::super::retained_read::KeepReader::WhileRetained).is_err());
    use super::super::retained_read::KeepReader;
    let (normal,_)=retained.open(deleted.op,"Report.txt",KeepReader::WhileRetained).unwrap();
    let (opened_before_restore,_)=retained.open(deleted.op,"Report.txt",KeepReader::ShareDelete).unwrap();
    assert_eq!(normal.current_standard_eof().unwrap(),4);
    assert_eq!(opened_before_restore.current_standard_eof().unwrap(),4);
    let body=normal.smb_read_window(0,4,|b|Ok(b.to_vec())).unwrap();
    assert_eq!(body,b"B\0K\0");
    let mut wrong_generation=deleted.to.clone();wrong_generation.generation+=1;
    assert!(tree.transfer(&Handoff::RestoreBound{source:wrong_generation,target:"Restored.txt".into()},op(0xb7),||Ok(())).is_err());
    let return_plan=Handoff::RestoreBound{source:deleted.to.clone(),target:"Restored.txt".into()};
    let restored=tree.transfer(&return_plan,op(0xb7),||Ok(())).unwrap();
    assert_eq!(tree.transfer(&return_plan,restored.op,||Ok(())).unwrap(),restored);
    assert_eq!(restored.to.file,file);assert_eq!(select(),restored.to);
    assert!(retained.open(deleted.op,"Report.txt",KeepReader::ShareDelete).is_err());
    assert_eq!(normal.current_standard_eof().unwrap_err().kind(),io::ErrorKind::NotFound);
    assert_eq!(opened_before_restore.current_standard_eof().unwrap_err().kind(),io::ErrorKind::NotFound);
    assert!(normal.smb_read_window::<()>(0,4,|_|panic!("stale non-share-delete reader was delivered")).is_err());
    assert_eq!(opened_before_restore.smb_read_window(0,4,|b|Ok(b.to_vec())).unwrap(),b"B\0K\0");
    let mut list=tree.root_files().unwrap();
    assert_eq!(list.next(16).unwrap().into_iter().map(|e|(e.name,e.file)).collect::<Vec<_>>(),vec![("Restored.txt".into(),file)]);
    let mut version=Vec::new();tree.open_version("Restored.txt",frozen.node).unwrap().range_to(0,4,&mut version).unwrap();
    assert_eq!(version,b"\0\0K\0");
    let mut current=Vec::new();tree.open_current("Restored.txt").unwrap().range_to(0,4,&mut current).unwrap();
    assert_eq!(current,b"B\0K\0");
    let again=tree.transfer(&Handoff::Delete(restored.to),op(0xb8),||Ok(())).unwrap();
    assert_ne!(again.op,deleted.op);
    assert!(tree.transfer(&Handoff::Restore{deleted:deleted.op,name:"Report.txt".into(),target:"Stale.txt".into()},
        op(0xb9),||Ok(())).is_err());
    use crate::system::reader::account::share_mode::Mode;
    let canceled=Handoff::Restore{deleted:again.op,name:"Restored.txt".into(),target:"Old target.txt".into()};
    let mut reserved=false;
    assert!(tree.transfer_after(&canceled,op(0xb9),||Ok(()),|cut|{
        if cut!=Cut::Reserved{return Ok(());}
        reserved=true;Err(invalid("leave original ticket without a native candidate"))
    }).is_err()&&reserved);
    let setting=admin.share_setting(rw).unwrap();
    admin.set_share_mode(op(0xc0),rw,setting.op,Mode::Disabled).unwrap();
    admin.set_share_mode(op(0xc1),rw,op(0xc0),Mode::ReadWrite).unwrap();
    assert!(tree.transfer(&canceled,op(0xb9),||Ok(())).is_err());
    let new=account.connect(rw).unwrap();
    let accepted=new.transfer(&Handoff::Restore{deleted:again.op,name:"Restored.txt".into(),target:"New target.txt".into()},
        op(0xc2),||Ok(())).unwrap();
    assert_eq!(select(),accepted.to);assert_eq!(accepted.to.file,file);
    same.at(|db,_|{
        let q=db.prepare(c"SELECT reason FROM main.file_transfer_void WHERE transfer_op=?1")?;
        q.bind_bytes(1,&op(0xb9).0)?;assert!(work_record::start_one(&q)?);assert_eq!(q.byte(0)?,1);q.done()
    }).unwrap();
    // A crash may leave an unbound physical prefix. Keep its names exactly,
    // but do not make the source/destination reservation unrecoverable.
    let retry_delete=Handoff::Delete(accepted.to);
    let staged=|id|recycle.join(Birth::placement_parent(id).unwrap().into_string().unwrap());
    let outer=staged(op(0xc3));
    let early=new.transfer_after(&retry_delete,op(0xc3),||Ok(()),|cut|{
        if cut!=Cut::Reserved{return Ok(());}
        std::fs::DirBuilder::new().mode(0o700).create(&outer)?;
        Err(invalid("stop after making only the exact outer pocket"))
    });
    assert!(early.is_err()&&outer.is_dir());
    assert!(new.transfer(&retry_delete,op(0xc4),||Ok(())).is_err());
    assert!(outer.is_dir());assert_eq!(select().role,Role::File);
    let incomplete=new.transfer_after(&retry_delete,op(0xc4),||Ok(()),|cut|{
        if cut!=Cut::CandidateSynced{return Ok(());}
        let shell=staged(op(0xc4)).join(format!("New target-{file}.txt"));
        let [_,witness,_,_]=crate::ordinary::working_round::names(op(0xb1));
        std::fs::rename(shell.join(witness.into_string().unwrap()),other.0.join("untouched-partial-proof"))?;
        Err(invalid("stop with only one named role still in the private candidate"))
    });
    assert!(incomplete.is_err()&&other.0.join("untouched-partial-proof").is_file());
    assert!(new.transfer(&retry_delete,op(0xc5),||Ok(())).is_err());
    assert!(staged(op(0xc4)).is_dir());assert_eq!(select().role,Role::File);
    let final_move=new.transfer(&retry_delete,op(0xc5),||Ok(())).unwrap();
    assert_eq!(select(),final_move.to);assert_eq!(final_move.to.file,file);
    // Deliberately not evidence of the separate reader-aware source reaper.
    assert!(occupied.is_dir());
}

#[test]
#[ignore="real separate source and target roots with cold replay; no SMB process"]
fn source_retirement_preserves_a_leased_or_unknown_original_and_resumes_exact_unlinks(){
    let temp=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
    let recycle=other.0.join("actual-R");let (path,pool,f,rw)=transfer_pool(&temp,&recycle);
    let src=InitialReader::open(&path,pool).unwrap();let smb=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let tree=smb.connect(rw).unwrap();let op=|n|OperationId([n;16]);
    let file=tree.create_file(op(0xd1),"Part.txt").unwrap().file;
    let pos=src.at(|db,c|Place::current(db,&c,file)).unwrap();
    let birth=src.at(|db,c|file_create::committed_file(db,&c,file)).unwrap();
    let input=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(temp.0.join("data" )).unwrap();
    input.write_all_at(b"K",0).unwrap();input.sync_all().unwrap();
    tree.open_writer("Part.txt",file).unwrap().save(op(0xd2),None,Some(2),4,&input).unwrap();
    let old=tree.open_current("Part.txt").unwrap();let cold=InitialReader::open(&path,pool).unwrap();
    let moved=tree.transfer(&Handoff::Delete(pos),op(0xd3),||Ok(())).unwrap();
    let old_path=f.join(birth.container().unwrap().into_string().unwrap());
    let hidden=f.join(Birth::retirement_name(moved.op).unwrap().into_string().unwrap());
    let hold_at=super::super::retirement::one_for_test(&cold,|_|Ok(()));
    assert_eq!(hold_at.unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert!(old_path.is_dir()&&!hidden.exists());drop(old);
    let actual=recycle.join(Birth::placement_parent(moved.op).unwrap().into_string().unwrap());
    let away=other.0.join("stashed-target");std::fs::rename(&actual,&away).unwrap();
    assert!(super::super::retirement::one_for_test(&cold,|_|Ok(())).is_err());
    assert!(old_path.is_dir()&&!hidden.exists());std::fs::rename(&away,&actual).unwrap();
    use crate::fs::RetireCut;
    let rename=std::cell::Cell::new(false);
    assert!(super::super::retirement::one_for_test(&cold,|point|{
        if point==RetireCut::AfterRenameBeforeSync{
            rename.set(true);return Err(invalid("interrupted before original rename parent sync"));
        }
        Ok(())
    }).is_err()&&rename.get());
    assert!(hidden.is_dir());
    let cut=std::cell::Cell::new(false);
    assert!(super::super::retirement::one_for_test(&cold,|point|{
        if point==RetireCut::AfterFirstNameBeforeSync{
            cut.set(true);return Err(invalid("interrupted after visible original unlink, before parent sync"));
        }
        Ok(())
    }).is_err()&&cut.get());
    assert!(!old_path.exists()&&hidden.is_dir());
    let [data,witness,_,_]=crate::ordinary::working_round::names(op(0xd2));
    let a=hidden.join(data.into_string().unwrap());let b=hidden.join(witness.into_string().unwrap());
    assert!(!a.exists()&&b.is_file());
    let original=temp.0.join("verified-last-link");std::fs::rename(&b,&original).unwrap();
    let replacement=OpenOptions::new().write(true).create_new(true).mode(0o600).open(&b).unwrap();
    replacement.write_all_at(b"K",0).unwrap();drop(replacement);
    let again=InitialReader::open(&path,pool).unwrap();
    assert!(super::super::retirement::one_for_test(&again,|_|Ok(())).is_err());
    assert_eq!(std::fs::read(&b).unwrap(),b"K");
    std::fs::remove_file(&b).unwrap();std::fs::rename(&original,&b).unwrap();
    super::super::retirement::one_for_test(&again,|_|Ok(())).unwrap();
    assert!(!hidden.exists()&&!f.join(birth.witness().unwrap().into_string().unwrap()).exists());
    let r=smb.retained().unwrap();
    let (handle,_)=r.open(moved.op,"Part.txt",super::super::retained_read::KeepReader::ShareDelete).unwrap();
    assert_eq!(handle.smb_read_window(0,4,|b|Ok(b.to_vec())).unwrap(),b"\\0\\0K\\0");
    let returned=tree.transfer(&Handoff::RestoreBound{source:moved.to,target:"Back.txt".into()},op(0xd4),||Ok(())).unwrap();
    assert_eq!(returned.to.file,file);
    let current_writer=tree.open_writer("Back.txt",file).unwrap();
    assert_eq!(super::super::retirement::one_for_test(&again,|_|Ok(())).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert_eq!(handle.smb_read_window(0,4,|b|Ok(b.to_vec())).unwrap(),b"\\0\\0K\\0");
    drop(handle);
    super::super::retirement::one_for_test(&again,|_|Ok(())).unwrap();
    assert!(!actual.exists());
    drop(current_writer);
    assert!(super::super::retirement::one_for_test(&again,|_|Ok(())).is_err());
}

#[test]
#[ignore="independent physical F and R; simulate an unrelated private target pair, not a live client"]
fn earlier_retirement_requires_the_actual_latest_transfer_target(){
    let tmp=PrivateTemp::under(Path::new("/tmp"));let rr=PrivateTemp::under(Path::new("/dev/shm"));
    let r=rr.0.join("actual-R");let (path,pool,f,rw)=transfer_pool(&tmp,&r);
    let src=InitialReader::open(&path,pool).unwrap();let auth=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let tree=auth.connect(rw).unwrap();let op=|n|OperationId([n;16]);
    let file=tree.create_file(op(0xe1),"Before.txt").unwrap().file;
    let pos=src.at(|db,c|Place::current(db,&c,file)).unwrap();
    let input=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).open(tmp.0.join("input-C")).unwrap();
    input.write_all_at(b"N",0).unwrap();input.sync_all().unwrap();
    tree.open_writer("Before.txt",file).unwrap().save(op(0xe2),None,Some(0),1,&input).unwrap();
    let deleted=tree.transfer(&Handoff::Delete(pos),op(0xe3),||Ok(())).unwrap();
    let returned=tree.transfer(&Handoff::RestoreBound{source:deleted.to,target:"After.txt".into()},op(0xe4),||Ok(())).unwrap();
    let current=f.join(Birth::placement_parent(returned.op).unwrap().into_string().unwrap()).join(format!("After-{file}.txt"));
    let [a,b,_,_]=crate::ordinary::working_round::names(op(0xe2));
    let was=f.join(format!("Before-{file}.txt")).join(a.to_str().unwrap());
    let first=current.join(a.to_str().unwrap());let second=current.join(b.to_str().unwrap());
    std::fs::rename(&first,tmp.0.join("kept-a")).unwrap();std::fs::rename(&second,tmp.0.join("kept-b")).unwrap();
    let different=OpenOptions::new().write(true).create_new(true).mode(0o600).open(&first).unwrap();
    different.write_all_at(b"N",0).unwrap();different.sync_all().unwrap();drop(different);
    std::fs::hard_link(&first,&second).unwrap();
    assert!(super::super::retirement::one_for_test(&src,|_|Ok(())).is_err());
    assert_eq!(std::fs::read(was).unwrap(),b"N");
    assert!(!f.join(Birth::retirement_name(deleted.op).unwrap().into_string().unwrap()).exists());
}
