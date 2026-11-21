//! Original SQLite rename decision, shared-name edges and physical identity.
//! The raw fixture binds the real SQL tuples; the wire issuer is separate.
use super::*;
use crate::{FileId,OperationId,fs::HeldDir,identity::ShareId,
    ordinary::portable_birth::{Birth,RenameInstall,RenameStage},
    system::reader::{account::share_mode::Mode,file_location::Place,ordinary_pool,work_record},
    test_support::PrivateTemp};
use std::{ffi::CString,fs::OpenOptions,io::Write,os::unix::fs::{MetadataExt,OpenOptionsExt}};

fn op(x:u8)->OperationId{OperationId([x;16])}
fn hex(bytes:&[u8])->String{bytes.iter().map(|byte|format!("{byte:02x}")).collect()}
fn changed(db:&Connection,input:String,op:OperationId)->io::Result<()>{
    let q=db.prepare(&CString::new(input).unwrap())?;
    if !work_record::start_one(&q)?||q.array::<16>(0)?!=op.0{
        return Err(io::ErrorKind::NotFound.into());
    }
    q.done()
}
fn issue(src:&InitialReader,db:&Connection,token:OperationId,file:FileId,share:ShareId,target:&str)->io::Result<()>{
    let o=hex(&token.0);let f=hex(&file.0);let g=hex(&share.0);let n=hex(target.as_bytes());
    let terminal=hex(Birth::placed_container(target,file)?.to_bytes());
    src.transaction(db,c"BEGIN IMMEDIATE",|db,_|{
        changed(db,format!("INSERT OR REPLACE INTO file_rename(rename_op,file_id,source_op,source_gen,volume_id,parent_id,source_name,target_name,
              backing_op,source_terminal,target_terminal,share_id,setting_op,auth_op,save_op,manual_op)
          SELECT X'{o}',l.file_id,l.position_op,l.generation,l.volume_id,l.parent_id,l.display,X'{n}',l.backing_op,l.flat_terminal,
            CASE WHEN l.backing_op IS NULL THEN X'{terminal}' ELSE NULL END,
            a.share_id,a.setting_op,a.auth_op,l.save_op,l.manual_op
          FROM transferable_location l JOIN live_ordinary_writer a ON a.volume_id=l.volume_id
          WHERE l.file_id=X'{f}' AND a.share_id=X'{g}' RETURNING rename_op"),token)?;
        changed(db,format!("INSERT INTO operation SELECT X'{o}',14,a.actor_id,zeroblob(32),0
            FROM live_ordinary_writer a WHERE a.share_id=X'{g}' RETURNING op_id"),token)
    })
}
fn finish(db:&Connection,id:OperationId)->io::Result<()>{
    changed(db,format!("UPDATE operation SET state=1 WHERE op_id=X'{}' AND kind=14 RETURNING op_id",hex(&id.0)),id)
}

#[test]
#[ignore="source only: real SQLite, same-parent leaf rename and original FDs; no SMB daemon"]
fn a_rename_preserves_real_file_identity_and_its_settled_view_without_sharing_a_name(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,f,rw,_)=ordinary_pool(&tmp);
    let src=InitialReader::open(&s,pool).unwrap();
    let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let tree=session.connect(rw).unwrap();
    let born=tree.create_file(op(0xd0),"Report.txt").unwrap();
    let bytes=tmp.0.join("working-source");
    let mut input=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600).open(&bytes).unwrap();
    input.write_all(b"same FILE").unwrap();input.sync_all().unwrap();
    tree.save_first(op(0xd1),"Report.txt",born.file,&input).unwrap();
    let initial=src.at(|db,c|Place::current(db,&c,born.file)).unwrap();
    let file_id=tree.open_current("Report.txt").unwrap().native_key().unwrap();
    let parent=HeldDir::trusted_storage(&f).unwrap();
    let physical=|name:&str|parent.checked_child_path(&Birth::placed_container(name,born.file)?);
    let inode=std::fs::metadata(physical("Report.txt").unwrap()).unwrap().ino();

    tree.create_file(op(0xd2),"Occupied.txt").unwrap();
    let (_,route)=tree.ordinary_route(&[],true).unwrap();
    let terminal=Birth::placed_container("Target.txt",born.file).unwrap();
    tree.make_ordinary_directory(&route,op(0xd3),terminal.to_str().unwrap(),|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
    let raw=Connection::open(&src.dir,true).unwrap();
    assert!(issue(&src,&raw,op(0xd4),born.file,rw,"Occupied.txt").is_err());
    assert!(issue(&src,&raw,op(0xd5),born.file,rw,"Target.txt").is_err());
    assert_eq!(src.at(|db,c|Place::current(db,&c,born.file)).unwrap(),initial);
    issue(&src,&raw,op(0xd6),born.file,rw,"report.txt").unwrap();
    assert!(issue(&src,&raw,op(0xd6),born.file,rw,"Another.txt").is_err());
    assert!(issue(&src,&raw,op(0xd7),born.file,rw,"Another.txt").is_err());
    for name in ["Report.txt","report.txt"]{
        assert_eq!(tree.occupied_name(name,false).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    }
    assert!(physical("Report.txt").unwrap().exists()&&!physical("report.txt").unwrap().exists());
    let independent=InitialReader::open(&s,pool).unwrap();
    let web=independent.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let old=web.share_setting(rw).unwrap();
    web.set_share_mode(op(0xd8),rw,old.op,Mode::ReadOnly).unwrap();
    assert!(finish(&raw,op(0xd6)).is_err());
    web.set_share_mode(op(0xd9),rw,op(0xd8),Mode::ReadWrite).unwrap();
    assert!(finish(&raw,op(0xd6)).is_err());
    changed(&raw,format!("INSERT INTO file_rename_abort SELECT X'{}',setting_op,auth_op,2
        FROM live_ordinary_writer WHERE share_id=X'{}' RETURNING rename_op",hex(&op(0xd6).0),hex(&rw.0)),op(0xd6)).unwrap();
    assert!(finish(&raw,op(0xd6)).is_err());
    assert_eq!(src.at(|db,c|Place::current(db,&c,born.file)).unwrap(),initial);
    let renewed=session.connect(rw).unwrap();
    issue(&src,&raw,op(0xda),born.file,rw,"report.txt").unwrap();
    let (material,expected)=src.at(|db,c|{
        let placed=Place::current(db,&c,born.file)?.bind(db,&c)?;
        let expected=work_record::members(db,&c,&placed.birth)?;Ok((placed.birth,expected))
    }).unwrap();
    let real=RenameInstall::hold(&material,&parent,"Report.txt","report.txt",RenameStage::Reserved,
        |m|expected.require(m)).unwrap();
    let (installed,())=real.install(||Ok(()),||{
        assert!(renewed.occupied_name("report.txt",false).is_err());Ok(())
    },|_|finish(&raw,op(0xda))).unwrap();
    drop(installed);
    let open=renewed.open_current("report.txt").unwrap();
    let mut found=Vec::new();open.range_to(0,9,&mut found).unwrap();
    assert_eq!(found,b"same FILE");assert_eq!(open.native_key().unwrap(),file_id);
    assert_eq!(std::fs::metadata(physical("report.txt").unwrap()).unwrap().ino(),inode);
    assert_eq!(renewed.occupied_name("Report.txt",false).unwrap(),None);
    let current=src.at(|db,c|Place::current(db,&c,born.file)).unwrap();
    assert_eq!((current.op,current.generation,current.backing,current.file),(op(0xda),2,None,born.file));
}

#[test]
#[ignore="source only: actual FILE/session fixture and two SQLite observers; no SMB daemon"]
fn actual_rename_issuer_keeps_the_original_reader_and_reports_a_lost_final_reply_as_unknown(){
    use super::file_rename::Cut;
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,_,rw,_)=ordinary_pool(&tmp);
    let src=InitialReader::open(&s,pool).unwrap();
    let initial=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let tree=initial.connect(rw).unwrap();
    let file=tree.create_file(op(0xbb),"First.txt").unwrap();
    let mut raw=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600)
        .open(tmp.0.join("same-original")).unwrap();
    raw.write_all(b"old bytes").unwrap();raw.sync_all().unwrap();
    tree.save_first(op(0xbc),"First.txt",file.file,&raw).unwrap();
    let mut current=tree.open_current("First.txt").unwrap();
    let delayed_open=tree.open_current("First.txt").unwrap(); // storage cut already returned, native entry not admitted
    let old_key=current.native_key().unwrap();
    let observed=InitialReader::open(&s,pool).unwrap();
    let changed_name=std::cell::Cell::new(false);
    current.rename_native(op(0xbd),"Renamed.txt",&||Ok(()),|cut|{
        if cut==Cut::NameChanged{
            changed_name.set(true);
            observed.at(|db,c|{
                let p=Place::current(db,&c,file.file)?;
                assert_eq!(p.name,"First.txt");
                for n in ["First.txt","Renamed.txt"]{
                    assert_eq!(Place::named(db,&c,p.volume,p.parent,n).unwrap_err().kind(),io::ErrorKind::WouldBlock);
                }
                Ok(())
            })?;
        }
        Ok(())
    }).unwrap();
    assert!(changed_name.get());
    assert!(delayed_open.smb_read_window::<()>(0,0,|_|panic!("stale CREATE proof completed")).is_err());
    current.smb_read_window(0,0,|b|{assert!(b.is_empty());Ok(())}).unwrap();
    drop(delayed_open);
    assert_eq!(current.native_key().unwrap(),old_key);
    assert_eq!(current.working_original(),Some(op(0xbc)));
    let mut old=Vec::new();current.range_to(0,9,&mut old).unwrap();assert_eq!(old,b"old bytes");
    assert_eq!(current.rename_native(op(0xbe),"Last.txt",&||Ok(()),|cut|
        if cut==Cut::Confirmed{Err(io::ErrorKind::ConnectionAborted.into())}else{Ok(())}
    ).unwrap_err().kind(),io::ErrorKind::ConnectionAborted);
    let confirmed=observed.at(|db,c|Place::current(db,&c,file.file)).unwrap();
    assert_eq!((confirmed.op,confirmed.name.as_str(),confirmed.generation),(op(0xbe),"Last.txt",3));
    let cold=tree.open_current("Last.txt").unwrap();
    assert_eq!(cold.native_key().unwrap(),old_key);
    assert_eq!(cold.working_original(),Some(op(0xbc)));
}

#[test]
#[ignore="source only: original durable rename, lost holder, independent cold reader and changed grant"]
fn recover_the_recorded_name_after_the_old_handle_is_gone_and_never_regrant_a_revoked_ticket(){
    use super::file_rename::{Cut,recover_for_test};
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,f,rw,_)=ordinary_pool(&tmp);
    let src=InitialReader::open(&s,pool).unwrap();
    let initial=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let tree=initial.connect(rw).unwrap();
    let started=tree.create_file(op(0xc8),"Old.txt").unwrap();
    let mut member=tree.open_current("Old.txt").unwrap();
    let prior=member.native_key().unwrap();
    let interrupt=|cut|if cut==Cut::NameChanged{
        Err(io::ErrorKind::ConnectionAborted.into())
    }else{Ok(())};
    let untouched=op(0xcd);
    let reached=std::cell::Cell::new(false);
    let cut=member.rename_native(untouched,"Not switched.txt",&||Ok(()),|cut|{
        if cut==Cut::Reserved{
            reached.set(true);return Err(io::ErrorKind::ConnectionAborted.into());
        }
        Ok(())
    });
    assert!(reached.get()&&cut.unwrap_err().kind()==io::ErrorKind::ConnectionAborted);drop(member);
    let interrupted=InitialReader::open(&s,pool).unwrap();
    let unchanged=interrupted.at(|db,c|Place::current(db,&c,started.file)).unwrap();
    let granted=|_:&Connection,_:&Catalog,share:ShareId,volume|if (share,volume)==(rw,unchanged.volume){Ok(())}else{Err(denied())};
    assert!(tree.open_current("Old.txt").is_err()); // never answer positively while both names are reserved
    recover_for_test(&interrupted,untouched,started.file,&granted).unwrap();
    interrupted.at(|db,c|{
        let q=db.prepare(c"SELECT a.reason,o.state FROM main.file_rename_abort a JOIN main.operation o ON o.op_id=a.rename_op WHERE a.rename_op=?1")?;
        q.bind_bytes(1,&untouched.0)?;assert!(work_record::start_one(&q)?);
        assert_eq!((q.byte(0)?,q.byte(1)?),(1,0));q.done()?;
        assert_eq!(Place::current(db,&c,started.file)?,unchanged);
        Ok(())
    }).unwrap();
    let never=Birth::placed_container("Not switched.txt",started.file).unwrap();
    assert!(!f.join(never.to_str().unwrap()).exists());
    let mut member=tree.open_current("Old.txt").unwrap();
    assert_eq!(member.native_key().unwrap(),prior);
    let want=op(0xc9);
    assert_eq!(member.rename_native(want,"New.txt",&||Ok(()),interrupt).unwrap_err().kind(),io::ErrorKind::ConnectionAborted);
    drop(member);
    let original=InitialReader::open(&s,pool).unwrap();
    let p=original.at(|db,c|Place::current(db,&c,started.file)).unwrap();
    let exact=|_:&Connection,_:&Catalog,share:ShareId,volume|if (share,volume)==(rw,p.volume){Ok(())}else{Err(denied())};
    for n in ["Old.txt","New.txt"]{
        assert_eq!(tree.open_current(n).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    }
    assert!(recover_for_test(&original,want,started.file,|_,_,_,_|Err(denied())).is_err());
    recover_for_test(&original,want,started.file,&exact).unwrap();
    let target=original.at(|db,c|Place::current(db,&c,started.file)).unwrap();
    assert_eq!((target.op,target.name.as_str()),(want,"New.txt"));
    assert_eq!(tree.open_current("New.txt").unwrap().native_key().unwrap(),prior);
    let mut source=tree.open_current("New.txt").unwrap();
    let revoked=op(0xca);
    assert_eq!(source.rename_native(revoked,"Gone.txt",&||Ok(()),interrupt).unwrap_err().kind(),io::ErrorKind::ConnectionAborted);
    drop(source);
    let proposed=Birth::placed_container("Gone.txt",started.file).unwrap();
    let physical=f.join(proposed.to_str().unwrap());
    assert!(physical.is_dir());
    let web=original.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let setting=web.share_setting(rw).unwrap();
    web.set_share_mode(op(0xcb),rw,setting.op,Mode::ReadOnly).unwrap();
    assert!(recover_for_test(&original,revoked,started.file,&exact).is_err());
    assert!(physical.is_dir());
    web.set_share_mode(op(0xcc),rw,op(0xcb),Mode::ReadWrite).unwrap();
    recover_for_test(&original,revoked,started.file,&exact).unwrap();
    assert!(!physical.exists());
    assert!(recover_for_test(&original,revoked,started.file,&exact).is_err());
    assert_eq!(original.at(|db,c|Place::current(db,&c,started.file)).unwrap(),target);
    let active=initial.connect(rw).unwrap();
    assert_eq!(active.open_current("New.txt").unwrap().native_key().unwrap(),prior);
    assert!(active.open_current("Gone.txt").is_err());
}

#[test]
#[ignore="source only: real distinct F/R, original retirement guard, and independent SQLite cuts"]
fn same_file_after_restore_can_rename_while_the_old_retained_read_delays_only_its_retirement(){
    use super::{file_transfer::Handoff,retained_read::KeepReader,retirement};
    use crate::system::reader::tests::transfer_pool;
    use std::path::Path;

    let local=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
    let r=other.0.join("actual-R");let (s,pool,f,rw)=transfer_pool(&local,&r);
    let source=InitialReader::open(&s,pool).unwrap();let cold=InitialReader::open(&s,pool).unwrap();
    let smb=SmbSession::fixture_authenticated(&source,"files.smb").unwrap();let tree=smb.connect(rw).unwrap();
    let admin=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let file=tree.create_file(op(0x81),"Draft.txt").unwrap().file;
    let mut input=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600)
        .open(local.0.join("actual-body")).unwrap();
    input.write_all(b"same data").unwrap();input.sync_all().unwrap();
    tree.open_writer("Draft.txt",file).unwrap().save(op(0x82),None,Some(0),9,&input).unwrap();
    let frozen=admin.freeze(op(0x83),admin.manual_target(file).unwrap()).unwrap();
    let position=||cold.at(|db,c|Place::current(db,&c,file)).unwrap();
    let deleted=tree.transfer(&Handoff::Delete(position()),op(0x84),||Ok(())).unwrap();
    let native=|root:&Path,transfer:OperationId|root.join(Birth::placement_parent(transfer).unwrap().to_str().unwrap());
    let old_pocket=native(&r,deleted.op);
    let retained=smb.retained().unwrap();
    retirement::one_for_test(&cold,|_|Ok(())).unwrap(); // original F, before taking the R read lease
    let (retained_handle,native_id)=retained.open(deleted.op,"Draft.txt",KeepReader::ShareDelete).unwrap();
    let restored=tree.transfer(&Handoff::RestoreBound{source:deleted.to,target:"Return.txt".into()},
        op(0x85),||Ok(())).unwrap();
    let pocket=native(&f,restored.op);
    let physical=|name|pocket.join(Birth::placed_container(name,file).unwrap().to_str().unwrap());
    let before=physical("Return.txt");let inode=std::fs::metadata(&before).unwrap().ino();
    let header=std::fs::metadata(before.join("FILE")).unwrap().ino();
    let mut holder=tree.open_current("Return.txt").unwrap();
    assert_eq!(holder.native_key().unwrap(),native_id);
    holder.rename_native(op(0x86),"Renamed.txt",&||Ok(()),|_|Ok(())).unwrap();
    let selected=position();
    assert_eq!((selected.file,selected.op,selected.backing,selected.name.as_str()),
        (file,op(0x86),Some(restored.op),"Renamed.txt"));
    assert!(!before.exists());assert!(tree.open_current("Return.txt").is_err());
    assert_eq!(std::fs::metadata(physical("Renamed.txt")).unwrap().ino(),inode);
    assert_eq!(std::fs::metadata(physical("Renamed.txt").join("FILE")).unwrap().ino(),header);
    assert_eq!(holder.native_key().unwrap(),native_id);drop(holder);
    assert_eq!(retained_handle.smb_read_window(0,9,|body|Ok(body.to_vec())).unwrap(),b"same data");
    assert_eq!(retirement::one_for_test(&cold,|_|Ok(())).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert!(old_pocket.is_dir());drop(retained_handle);
    retirement::one_for_test(&cold,|_|Ok(())).unwrap();
    assert!(!old_pocket.exists());
    let reopened=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let tree=reopened.connect(rw).unwrap();
    assert_eq!(tree.open_current("Renamed.txt").unwrap().native_key().unwrap(),native_id);
    let mut version=Vec::new();
    tree.open_version("Renamed.txt",frozen.node).unwrap().range_to(0,9,&mut version).unwrap();
    assert_eq!(version,b"same data");
}

#[test]
#[ignore="source only: real unselected work role and confirmed versions survive the same physical rename"]
fn same_backing_rename_keeps_superseded_work_names_without_making_them_readable(){
    use super::work_change::Cut as WorkCut;
    use std::fs::File;
    use std::os::unix::fs::FileExt;

    let t=PrivateTemp::under(&std::env::temp_dir());let (s,pool,f,rw,_)=ordinary_pool(&t);
    let src=InitialReader::open(&s,pool).unwrap();let cold=InitialReader::open(&s,pool).unwrap();
    let auth=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let tree=auth.connect(rw).unwrap();
    let admin=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let initial="Report";let file=tree.create_file(op(0x91),initial).unwrap().file;
    let stock=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600)
        .open(t.0.join("confirmed-input")).unwrap();
    stock.write_all_at(b"ABCD",0).unwrap();stock.sync_all().unwrap();
    let first=tree.open_writer(initial,file).unwrap().save(op(0x92),None,Some(0),4,&stock).unwrap();
    let older=admin.freeze(op(0x93),admin.manual_target(file).unwrap()).unwrap();
    let dead=op(0x94);let good=op(0x95);
    let body=|name|f.join(Birth::placed_container(name,file).unwrap().to_str().unwrap());
    let [unselected,_,_,_]=crate::ordinary::working_round::names(dead);
    let mut materialized=false;
    let change=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600)
        .open(t.0.join("selected-input")).unwrap();
    change.write_all_at(b"Z",0).unwrap();change.sync_all().unwrap();
    {
        let lock=tree.open_writer(initial,file).unwrap();
        let interrupted=lock.save_after(dead,Some(first.key.save),Some(0),4,&change,|cut|{
            if cut!=WorkCut::Reserved{return Ok(());}
            let mut raw=OpenOptions::new().create_new(true).write(true).mode(0o600)
                .open(body(initial).join(unselected.to_str().unwrap()))?;
            raw.write_all(b"?")?;raw.sync_all()?;File::open(body(initial))?.sync_all()?;
            materialized=true;Err(io::ErrorKind::ConnectionAborted.into())
        });
        assert!(interrupted.is_err()&&materialized);
    }
    let settled=tree.open_writer(initial,file).unwrap().save(good,Some(first.key.save),Some(0),4,&change).unwrap();
    assert_eq!((settled.key.round,settled.key.revision),(good,1));
    cold.at(|db,_|{
        let q=db.prepare(c"SELECT previous_pending,retained_mask FROM main.work_edit WHERE edit_op=?1")?;
        q.bind_bytes(1,&good.0)?;assert!(work_record::start_one(&q)?);
        assert_eq!((q.array::<16>(0)?,q.byte(1)?),(dead.0,1));q.done()
    }).unwrap();
    let renamed="reviewed";
    let mut original=tree.open_current(initial).unwrap();
    original.rename_native(op(0x96),renamed,&||Ok(()),|_|Ok(())).unwrap();
    drop(original);
    assert_eq!(std::fs::read(body(renamed).join(unselected.to_str().unwrap())).unwrap(),b"?");
    assert!(!body(initial).exists());
    let owns=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();let cold_tree=owns.connect(rw).unwrap();
    let mut selected=Vec::new();
    cold_tree.open_current(renamed).unwrap().range_to(0,4,&mut selected).unwrap();
    assert_eq!(selected,b"ZBCD");
    let fresh=cold_tree.open_writer(renamed,file).unwrap();
    assert!(fresh.receipt(dead).is_err()); // old position never re-authorizes the unconfirmed SAVE
    assert_eq!(fresh.save(op(0x97),Some(good),Some(2),4,&change).unwrap().key.revision,2);drop(fresh);
    let newest=admin.freeze(op(0x98),admin.manual_target(file).unwrap()).unwrap();
    assert_eq!(newest.target.parent,Some(older.original));
    let mut prior=Vec::new();cold_tree.open_version(renamed,older.node).unwrap().range_to(0,4,&mut prior).unwrap();
    let mut newer=Vec::new();cold_tree.open_version(renamed,newest.node).unwrap().range_to(0,4,&mut newer).unwrap();
    assert_eq!(prior,b"ABCD");assert_eq!(newer,b"ZBZD");
}
