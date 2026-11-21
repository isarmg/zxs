//! Actual SQLite trigger sources for sole placement and original delete
//! rounds. They deliberately do not simulate the physical transfer or SMB.
use super::*;
use crate::{FileId,identity::ShareId,system::reader::{account::share_mode::Mode,smb::SmbSession}};

fn at(src:&InitialReader,file:FileId)->(OperationId,i64,Role,VolumeId,String){
    src.at(|db,_|{
        let q=db.prepare(c"SELECT position_op,generation,role,volume_id,display FROM file_location WHERE file_id=?1")?;
        q.bind_bytes(1,&file.0)?;assert_eq!(unsafe{sqlite3_step(q.0)},100);
        let role=match q.byte(2)?{1=>Role::Recycle,2=>Role::File,_=>panic!("fixture location")};
        let result=(OperationId(q.array(0)?),q.integer(1)?,role,VolumeId(q.array(3)?),
            String::from_utf8(q.blob(4,222)?.to_vec()).unwrap());
        q.done()?;Ok(result)
    }).unwrap()
}
fn ticket(db:&Builder,op:OperationId,file:FileId,where_op:OperationId,share:ShareId,target:VolumeId,
          display:&str,direction:u8)->io::Result<()>{
    ticket_at(db,op,file,where_op,share,target,display,direction,None)
}
fn ticket_at(db:&Builder,op:OperationId,file:FileId,where_op:OperationId,share:ShareId,target:VolumeId,
    display:&str,direction:u8,ordinary_destination:Option<OperationId>)->io::Result<()>{
    let dest=ordinary_destination.map_or_else(||"NULL".into(),|v|format!("X'{}'",hex(&v.0)));
    let place=hex(&where_op.0);let file=hex(&file.0);let mode=hex(&share.0);
    let op=hex(&op.0);let target=hex(&target.0);let name=hex(display.as_bytes());
    // Use the actual original currently eligible writer tuple and view; raw
    // SQL test uses a dummy digest only to check transition/freshness guards.
    let result=db.try_exec(&format!(
        "BEGIN IMMEDIATE;
         INSERT INTO file_transfer(transfer_op,file_id,direction,source_op,source_gen,source_id,source_name,
            target_id,target_name,share_id,setting_op,auth_op,save_op,manual_op,source_parent,target_parent,source_backing,source_terminal,target_setting_op)
         SELECT X'{op}',l.file_id,{direction},l.position_op,l.generation,l.volume_id,l.display,
           X'{target}',X'{name}',a.share_id,a.setting_op,a.auth_op,l.save_op,l.manual_op,l.parent_id,zeroblob(16),l.backing_op,l.flat_terminal,{dest}
         FROM transferable_location l JOIN live_ordinary_writer a ON a.share_id=X'{mode}'
         WHERE l.file_id=X'{file}' AND l.position_op=X'{place}';
         INSERT INTO operation SELECT X'{op}',12,a.actor_id,zeroblob(32),0 FROM live_ordinary_writer a WHERE a.share_id=X'{mode}';
         COMMIT;"));
    if result.is_err(){db.exec("ROLLBACK;");}
    result
}
fn finish(db:&Builder,op:OperationId)->io::Result<()>{
    db.try_exec(&format!("UPDATE operation SET state=1 WHERE op_id=X'{}' AND state=0;",hex(&op.0)))
}
#[test]
#[ignore="source-only real SQLite state/anti-replace fixture; no physical recycle operation or SMB process"]
fn one_current_location_keeps_birth_and_readers_separate_from_a_stale_delete_or_regrant(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,_)=ordinary_pool(&t);
    let src=InitialReader::open(&s,p).unwrap();
    let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let opened=smb.connect(rw).unwrap();
    let b=opened.create_file(OperationId([0xb1;16]),"Report.txt").unwrap();
    // A real writer leaves physically synced but unconfirmed bytes after
    // the last successful SAVE. Move is allowed to select only that older view.
    use crate::system::reader::smb::work_change::Cut;
    let first_save=OperationId([0xc1;16]);let lost_save=OperationId([0xc2;16]);
    let mut input=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(t.0.join("packed")).unwrap();
    input.write_all(b"x").unwrap();input.sync_all().unwrap();
    let writer=opened.open_writer("Report.txt",b.file).unwrap();
    writer.save(first_save,None,Some(0),1,&input).unwrap();
    let mut other=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(t.0.join("packed-unconfirmed")).unwrap();
    other.write_all(b"y").unwrap();other.sync_all().unwrap();
    let mut reached=false;
    assert!(writer.save_after(lost_save,Some(first_save),Some(0),1,&other,|cut|{
        if cut==Cut::BytesSynced{
            reached=true;return Err(io::Error::other("lose the original SAVE before SQL confirmation"));
        }Ok(())
    }).is_err());assert!(reached);drop(writer);
    let f_id=VolumeId([0x13;16]);let r_id=VolumeId([0x12;16]);
    let first=at(&src,b.file);assert_eq!(first,(b.created,1,Role::File,f_id,"Report.txt".into()));
    let raw=Builder::existing(&s.join("SYSTEM.sqlite3"));raw.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;");
    let file=hex(&b.file.0);let deleting=OperationId([0xb2;16]);
    let original_terminal=format!("Report-{}.txt",b.file);
    let alias=opened.create_file(OperationId([0xc6;16]),&original_terminal).unwrap();
    assert_eq!(opened.open_current(&original_terminal).unwrap().file(),alias.file);
    assert!(ticket(&raw,deleting,b.file,b.created,rw,f_id,"Report.txt",1).is_err()); // never recycle to F
    ticket(&raw,deleting,b.file,b.created,rw,r_id,"Report.txt",1).unwrap();
    assert!(ticket(&raw,OperationId([0xb3;16]),b.file,b.created,rw,r_id,"Report.txt",1).is_err());
    assert_eq!(at(&src,b.file),first); // pending alone still the ordinary FILE
    finish(&raw,deleting).unwrap();
    assert_eq!(at(&src,b.file),(deleting,2,Role::Recycle,r_id,"Report.txt".into()));
    assert!(finish(&raw,lost_save).is_err());
    assert_eq!(opened.open_current(&original_terminal).unwrap().file(),alias.file);
    let old_physical=f.join(&original_terminal);let held_as=f.join("original-terminal-fixture");
    std::fs::rename(&old_physical,&held_as).unwrap();
    std::fs::DirBuilder::new().mode(0o700).create(&old_physical).unwrap();
    assert!(opened.open_current(&original_terminal).is_err()); // a same-named substitute is not the old FILE
    std::fs::remove_dir(&old_physical).unwrap();std::fs::rename(&held_as,&old_physical).unwrap();
    assert_eq!(opened.open_current(&original_terminal).unwrap().file(),alias.file);
    src.at(|db,_|{
        let q=db.prepare(c"SELECT o.state,w.save_op,(SELECT count(*) FROM pending_edit WHERE file_id=w.file_id)
             FROM main.selected_work w JOIN main.operation o ON o.op_id=?2 WHERE w.file_id=?1")?;
        q.bind_bytes(1,&b.file.0)?;q.bind_bytes(2,&lost_save.0)?;
        assert_eq!(unsafe{sqlite3_step(q.0)},100);assert_eq!(q.byte(0)?,0);
        assert_eq!(q.array::<16>(1)?,first_save.0);assert_eq!(q.integer(2)?,0);q.done()
    }).unwrap();
    let origin=format!("SELECT file_id,create_op,1,volume_id,2,display,parent_id,NULL,terminal FROM root_entry WHERE file_id=X'{file}';");
    assert!(raw.try_exec(&format!("INSERT OR REPLACE INTO file_location(file_id,position_op,generation,volume_id,role,display,parent_id,backing_op,flat_terminal) {origin}")).is_err());
    assert!(ticket(&raw,OperationId([0xb4;16]),b.file,b.created,rw,r_id,"Report.txt",1).is_err());
    // The old ordinary name is available for a new FileId. The two R
    // rows share a display name but never a lookup or deletion identity.
    let other=opened.create_file(OperationId([0xc3;16]),"Report.txt").unwrap();
    assert_ne!(other.file,b.file);
    let independently_deleted=OperationId([0xc4;16]);
    ticket(&raw,independently_deleted,other.file,other.created,rw,r_id,"Report.txt",1).unwrap();
    finish(&raw,independently_deleted).unwrap();
    src.at(|db,c|{
        use super::super::file_location::Place;
        assert_eq!(Place::retained(db,&c,r_id,deleting,"Report.txt")?.file,b.file);
        assert_eq!(Place::retained(db,&c,r_id,independently_deleted,"Report.txt")?.file,other.file);Ok(())
    }).unwrap();
    let restoring=OperationId([0xb5;16]);ticket(&raw,restoring,b.file,deleting,rw,f_id,"Return.txt",2).unwrap();
    // Separate real authorization connection: revocation and later regrant
    // never revive the pending original token or lose the retained side.
    let second=InitialReader::open(&s,p).unwrap();
    let web=second.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let prior=web.share_setting(rw).unwrap();let off=OperationId([0xb6;16]);let on=OperationId([0xb7;16]);
    web.set_share_mode(off,rw,prior.op,Mode::Disabled).unwrap();
    assert!(finish(&raw,restoring).is_err());
    web.set_share_mode(on,rw,off,Mode::ReadWrite).unwrap();
    assert!(finish(&raw,restoring).is_err());
    let old=hex(&restoring.0);
    raw.exec(&format!("INSERT INTO file_transfer_void
        SELECT X'{old}',setting_op,auth_op,1 FROM live_ordinary_writer WHERE share_id=X'{}';",hex(&rw.0)));
    assert!(finish(&raw,restoring).is_err());
    let retained=(deleting,2,Role::Recycle,r_id,"Report.txt".into());assert_eq!(at(&src,b.file),retained);
    let new=OperationId([0xb8;16]);ticket(&raw,new,b.file,deleting,rw,f_id,"Return.txt",2).unwrap();
    // Even though it is still the original raw role1 position, its name is
    // not an ordinary F slot. A real pending target may not be stolen.
    let current=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let granted=current.connect(rw).unwrap();
    assert!(granted.create_file(OperationId([0xb9;16]),"Return.txt").is_err());
    finish(&raw,new).unwrap();
    assert_eq!(at(&src,b.file),(new,3,Role::File,f_id,"Return.txt".into()));
    assert!(ticket(&raw,OperationId([0xba;16]),b.file,deleting,rw,f_id,"Again.txt",2).is_err());
    let again=OperationId([0xc5;16]);ticket(&raw,again,b.file,new,rw,r_id,"Return.txt",1).unwrap();finish(&raw,again).unwrap();
    src.at(|db,c|{
        use super::super::file_location::Place;
        assert!(Place::retained(db,&c,r_id,deleting,"Report.txt").is_err());
        assert!(Place::retained(db,&c,r_id,deleting,"Return.txt").is_err());
        assert_eq!(Place::retained(db,&c,r_id,again,"Return.txt")?.file,b.file);
        assert_eq!(Place::retained(db,&c,r_id,independently_deleted,"Report.txt")?.file,other.file);Ok(())
    }).unwrap();
    // The fixture never copied, cleaned or reinterpreted these original bytes.
    assert!(f.join(format!("Report-{}.txt/FILE",b.file)).is_file());
}

#[test]
#[ignore="source-only original SQLite state for two real ordinary grants; no mounts or transfer copy"]
fn another_ordinary_volume_needs_two_fresh_same_account_settings_and_one_destination(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());let other=tmp.0.join("zz-ordinary-destination");
    let(s,p)=publication::first_with_storage(&tmp,None,Some(&other));
    let src=InitialReader::open(&s,p).unwrap();
    create_file::open_root(&src);let(rw,_)=create_file::configure(&src);
    let op=|x|OperationId([x;16]);
    let admin=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let dst=VolumeId([0x14;16]);
    let publish=admin.publish_root(op(0xe0),dst,"destination").unwrap();
    let a=admin.share_setting(rw).unwrap();
    let desired=admin.set_share_mode(op(0xe1),publish.share,publish.op,Mode::ReadWrite).unwrap();
    let current=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let source=current.connect(rw).unwrap();
    let file=source.create_file(op(0xe2),"Source").unwrap();
    let source_place=at(&src,file.file);
    let raw=Builder::existing(&s.join("SYSTEM.sqlite3"));
    raw.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;");
    let reserve=|o,volume,setting|ticket_at(&raw,o,file.file,file.created,rw,volume,"Arrival",3,setting);
    assert!(reserve(op(0xe3),dst,None).is_err());
    assert!(reserve(op(0xe4),dst,Some(a.op)).is_err());
    assert!(reserve(op(0xe5),source_place.3,Some(a.op)).is_err());
    assert!(reserve(op(0xe6),VolumeId([0x12;16]),Some(desired.op)).is_err());
    reserve(op(0xe7),dst,Some(desired.op)).unwrap();
    assert_eq!(at(&src,file.file),source_place);
    assert!(!other.exists()); // SQL reservation is not a native destination proof.
    let destination=current.connect(publish.share).unwrap();
    // The ordinary target name, not just a (source, op) pair, is reserved.
    assert_eq!(destination.occupied_name("Arrival",false).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    let second=InitialReader::open(&s,p).unwrap();
    let web=second.authenticate_web("first.admin","correct horse battery staple").unwrap();
    web.set_share_mode(op(0xe8),publish.share,desired.op,Mode::ReadOnly).unwrap();
    assert!(finish(&raw,op(0xe7)).is_err());
    let renewed=web.set_share_mode(op(0xe9),publish.share,op(0xe8),Mode::ReadWrite).unwrap();
    assert!(finish(&raw,op(0xe7)).is_err());
    raw.exec(&format!("INSERT INTO file_transfer_void
       SELECT X'{}',setting_op,auth_op,1 FROM live_ordinary_writer WHERE share_id=X'{}';",hex(&op(0xe7).0),hex(&rw.0)));
    assert_eq!(at(&src,file.file),source_place);
    reserve(op(0xea),dst,Some(renewed.op)).unwrap();
    finish(&raw,op(0xea)).unwrap(); // raw SQL fixture only; production must first sync and verify actual bytes
    assert_eq!(at(&src,file.file),(op(0xea),source_place.1+1,Role::File,dst,"Arrival".into()));
    assert!(finish(&raw,op(0xe7)).is_err());
    assert!(current.connect(publish.share).unwrap().open_current("Arrival").is_err());
    assert!(source.open_current("Source").is_err());
    assert!(!other.exists());
}
