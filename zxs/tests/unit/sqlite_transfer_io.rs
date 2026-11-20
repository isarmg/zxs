//! Actual named source/target FILEs on separately mounted fixture roots.
use super::*;
use crate::{fs::invalid,ordinary::portable_birth::Birth,system::reader::{InitialReader,smb::SmbSession,tests::transfer_pool},
    test_support::PrivateTemp};
use std::{path::Path,fs::OpenOptions,os::unix::fs::{DirBuilderExt,FileExt,OpenOptionsExt}};

#[test]
#[ignore="source only: exact nested ordinary storage, actual private F/R and real SQLite; no SMB listener"]
fn nested_handoff_keeps_original_parents_and_the_same_confirmed_work(){
    let private=PrivateTemp::under(Path::new("/tmp"));let remote=PrivateTemp::under(Path::new("/dev/shm"));
    let recycle=remote.0.join("actual-R");let (s,p,f,rw)=transfer_pool(&private,&recycle);
    let src=InitialReader::open(&s,p).unwrap();
    let auth=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
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
    let account=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let tree=account.connect(rw).unwrap();
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
    let other_session=SmbSession::fixture_authenticated(&same,"files.smb").unwrap();
    let other_tree=other_session.connect(rw).unwrap();
    let mut draining=Some(other_tree.open_writer("Report.txt",file).unwrap());
    let mut waited=false;
    let deleted=tree.transfer_inner(&intent,op(0xb5),||Ok(()),true,|cut|{
        if cut==Cut::WaitingForSource{
            assert!(!waited);assert_eq!(select(),original);assert_eq!(count(),1);
            waited=true;drop(draining.take().expect("original separate writer still holds FILE EX"));
        }
        Ok(())
    }).unwrap();assert!(waited);assert_eq!(deleted.op,op(0xb4));
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
    let body=normal.smb_read_window(0,4,|b|Ok(b.to_vec())).unwrap();
    assert_eq!(body,b"B\0K\0");
    let mut wrong_generation=deleted.to.clone();wrong_generation.generation+=1;
    assert!(tree.transfer(&Handoff::RestoreBound{source:wrong_generation,target:"Restored.txt".into()},op(0xb7),||Ok(())).is_err());
    let return_plan=Handoff::RestoreBound{source:deleted.to.clone(),target:"Restored.txt".into()};
    let restored=tree.transfer(&return_plan,op(0xb7),||Ok(())).unwrap();
    assert_eq!(tree.transfer(&return_plan,restored.op,||Ok(())).unwrap(),restored);
    assert_eq!(restored.to.file,file);assert_eq!(select(),restored.to);
    assert!(retained.open(deleted.op,"Report.txt",KeepReader::ShareDelete).is_err());
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
    let src=InitialReader::open(&path,pool).unwrap();let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
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
    let src=InitialReader::open(&path,pool).unwrap();let auth=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
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
