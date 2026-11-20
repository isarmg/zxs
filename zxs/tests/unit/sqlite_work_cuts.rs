//! Exercises only the SQL deferred-row and operation interleaving gates.
//! The genesis SAVE is real; the other constant commitments intentionally
//! model statements, not qualified release or disk material.
use super::*;
use crate::system::reader::smb::SmbSession;

#[test]
#[ignore="source-only isolated real SQLite fixture; no network or test data from users"]
fn explicit_node_cannot_publish_from_a_staged_old_view_or_ignore_a_deferred_edit() {
    let t=PrivateTemp::under(&std::env::temp_dir());
    let (site,pool,_,rw,_)=ordinary_pool(&t);
    let src=InitialReader::open(&site,pool).unwrap();
    let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let tree=smb.connect(rw).unwrap();
    let born=tree.create_file(OperationId([0xa1;16]),"round.xlsx").unwrap();
    let initial=OperationId([0xa2;16]);
    let mut one=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600)
        .open(t.0.join("packed")).unwrap();
    one.write_all(b"x").unwrap();one.sync_all().unwrap();
    assert_eq!(tree.save_first(initial,"round.xlsx",born.file,&one).unwrap().revision,1);

    let q=Builder::existing(&site.join("SYSTEM.sqlite3"));
    q.exec("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;");
    let file=hex(&born.file.0);let prior=hex(&initial.0);let sharing=hex(&rw.0);
    let edit=hex(&[0xa3;16]);let node_op=hex(&[0xa4;16]);
    let node=hex(&[0xa5;16]);let version=hex(&[0xa6;16]);let nonce=hex(&[0xa7;16]);
    let add_child=format!(
        "INSERT INTO work_edit VALUES(X'{edit}',X'{file}',X'{sharing}',
           (SELECT setting_op FROM live_ordinary_writer WHERE share_id=X'{sharing}'),
           (SELECT auth_op FROM live_ordinary_writer WHERE share_id=X'{sharing}'),
           (SELECT position_op FROM file_location WHERE file_id=X'{file}'),X'{nonce}',1,
           X'{prior}',X'{prior}',NULL,2,1,1,2,159,318,163,481,zeroblob(32),zeroblob(32),zeroblob(32),NULL,0);");
    let attach_and_confirm=format!(
        "INSERT INTO operation VALUES(X'{edit}',10,(SELECT smb_id FROM account_pair WHERE singleton=1),zeroblob(32),0);
         UPDATE operation SET state=1 WHERE op_id=X'{edit}';");
    let stage_node=format!(
        "INSERT INTO manual_node VALUES(X'{node_op}',X'{file}',X'{prior}',X'{prior}',1,
          (SELECT credential_op FROM active_credential WHERE role=0),NULL,1,X'{node}',X'{version}');");
    let attach_node=format!(
        "INSERT INTO operation VALUES(X'{node_op}',11,(SELECT web_id FROM account_pair WHERE singleton=1),zeroblob(32),1);");
    q.exec("BEGIN IMMEDIATE;");
    q.exec(&stage_node);
    q.exec(&add_child);
    q.exec(&attach_and_confirm);
    assert!(q.try_exec(&attach_node).is_err(),"operation must recheck actual settled head");
    q.exec("ROLLBACK;");

    q.exec("BEGIN IMMEDIATE;");
    q.exec(&add_child); // Its deferred foreign-key parent has not been inserted.
    assert!(q.try_exec(&stage_node).is_err(),"child alone still reserves work");
    q.exec("ROLLBACK;");

    q.exec("BEGIN IMMEDIATE;");
    q.exec(&stage_node);
    q.exec(&attach_node); // Only the settled, original revision can be published.
    q.exec("COMMIT;");
    assert!(q.try_exec(&add_child).is_err(),"the frozen round cannot append");
}

#[test]
#[ignore="actual isolated ordinary FILE and SQLite; explicit Web publication source fixture"]
fn manual_publication_freezes_the_actual_view_without_copy_and_replays_only_its_original() {
    use std::os::unix::fs::FileExt;
    let t=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,f,rw,ro)=ordinary_pool(&t);
    let source=InitialReader::open(&s,pool).unwrap();
    let anon=InitialReader::open(&s,pool).unwrap();
    let admin=anon.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let smb=SmbSession::fixture_authenticated(&source,"files.smb").unwrap();
    let a=smb.connect(rw).unwrap();let b=smb.connect(ro).unwrap();
    let op=OperationId([0xb1;16]);let first=OperationId([0xb2;16]);let publish=OperationId([0xb3;16]);
    let file=a.create_file(op,"source.xlsx").unwrap().file;
    let before=b.open_current("source.xlsx").unwrap();assert_eq!(before.size(),0);
    let input=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(t.0.join("sample")).unwrap();
    input.write_all_at(b"first",0).unwrap();input.sync_all().unwrap();
    a.save_first(first,"source.xlsx",file,&input).unwrap();
    let target=admin.manual_target(file).unwrap();
    let names=f.join(format!("source-{file}.xlsx"));
    assert_eq!(std::fs::read_dir(&names).unwrap().count(),5);
    let sums=names.join(format!("checks-{first}-00000000"));
    let tamper=OpenOptions::new().write(true).open(&sums).unwrap();
    let previous=std::fs::read(&sums).unwrap();
    tamper.write_all_at(&[0xff],0).unwrap();tamper.sync_all().unwrap();
    assert!(admin.freeze(publish,target).is_err());
    tamper.write_all_at(&previous[..1],0).unwrap();tamper.sync_all().unwrap();drop(tamper);
    let receipt=admin.freeze(publish,target).unwrap();
    assert_eq!(receipt.ord,1);
    assert_eq!(admin.freeze(publish,target).unwrap(),receipt);
    assert!(admin.freeze(OperationId([0xb4;16]),target).is_err());
    assert!(admin.manual_target(file).is_err());
    assert_eq!(std::fs::read_dir(&names).unwrap().count(),5); // explicit Node did not copy
    let fixed=b.open_version("source.xlsx",receipt.node).unwrap();
    let mut seen=Vec::new();fixed.range_to(0,5,&mut seen).unwrap();assert_eq!(seen,b"first");
    let current=b.open_current("source.xlsx").unwrap();
    let mut still=Vec::new();current.range_to(0,5,&mut still).unwrap();assert_eq!(still,seen);
    before.range_to(0,0,Vec::new()).unwrap();
}

fn stage(t:&PrivateTemp,path:&str,body:&[u8])->File{
    let mut input=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(t.0.join(path)).unwrap();
    input.write_all(body).unwrap();input.sync_all().unwrap();input
}
fn count_manual(s:&InitialReader,file:crate::FileId)->i64{
    s.at(|db,_|{
        let q=db.prepare(c"SELECT count(*) FROM main.manual_node WHERE file_id=?1")?;
        q.bind_bytes(1,&file.0)?;
        if unsafe{sqlite3_step(q.0)}!=100{return Err(invalid("cannot count fixture manual history"));}
        let count=q.integer(0)?;q.done()?;Ok(count)
    }).unwrap()
}
#[test]
#[ignore="static source case for four genuine FILE/SQLite work rounds; no user media or service"]
fn each_manual_node_can_start_the_next_physical_delta_immediately_without_full() {
    use std::os::unix::fs::MetadataExt;
    let t=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,f,rw,ro)=ordinary_pool(&t);
    let source=InitialReader::open(&s,pool).unwrap();let management=InitialReader::open(&s,pool).unwrap();
    let admin=management.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let smb=SmbSession::fixture_authenticated(&source,"files.smb").unwrap();
    let a=smb.connect(rw).unwrap();let b=smb.connect(ro).unwrap();
    let name="Four.xlsx";
    let id=|x:u8|OperationId([x;16]);
    let file=a.create_file(id(0x90),name).unwrap().file;
    let raw=|x:u8|f.join(format!("Four-{file}.xlsx/change-{}-00000000",id(x)));
    let writer=a.open_writer(name,file).unwrap();
    assert_eq!(a.open_writer(name,file).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    let q=writer.save(id(0x91),None,Some(0),5,&stage(&t,"digits",b"ABCDE")).unwrap();
    assert_eq!(q.key.round,id(0x91));
    let inode=std::fs::metadata(raw(0x91)).unwrap().ino();
    let earliest=b.open_current(name).unwrap(); // now hold the original first revision
    let q=writer.save(id(0x92),Some(q.key.save),Some(1),5,&stage(&t,"overlap",b"xy")).unwrap();
    assert_eq!((q.key.round,q.key.revision),(id(0x91),2));
    assert_eq!(std::fs::metadata(raw(0x91)).unwrap().ino(),inode);
    assert_eq!(std::fs::metadata(raw(0x91)).unwrap().len(),7);
    assert_eq!(count_manual(&source,file),0);
    let target=admin.manual_target(file).unwrap();
    assert_eq!(admin.freeze(id(0xa1),target).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    drop(writer);let n1=admin.freeze(id(0xa1),target).unwrap();
    let old_round=b.open_current(name).unwrap();

    let writer=a.open_writer(name,file).unwrap();
    let q=writer.save(id(0x94),Some(q.key.save),Some(4),5,&stage(&t,"round-two",b"Z")).unwrap();
    assert_eq!((q.key.round,q.key.revision),(id(0x94),1));
    let empty=stage(&t,"resize",b"");
    let q=writer.save(id(0x95),Some(q.key.save),None,3,&empty).unwrap();
    let q=writer.save(id(0x96),Some(q.key.save),None,6,&empty).unwrap();
    assert_eq!((q.key.round,q.key.revision,q.size),(id(0x94),3,6));
    assert_eq!(std::fs::metadata(raw(0x94)).unwrap().len(),1);
    let mut old=Vec::new();earliest.range_to(0,5,&mut old).unwrap();assert_eq!(old,b"ABCDE");
    old.clear();old_round.range_to(0,5,&mut old).unwrap();assert_eq!(old,b"AxyDE");
    assert_eq!(count_manual(&source,file),1);
    drop(writer);let n2=admin.freeze(id(0xa2),admin.manual_target(file).unwrap()).unwrap();

    let writer=a.open_writer(name,file).unwrap();
    let q=writer.save(id(0x97),Some(q.key.save),Some(4),6,&stage(&t,"round-three",b"W")).unwrap();
    assert_eq!((q.key.round,q.key.revision),(id(0x97),1));
    assert_eq!(count_manual(&source,file),2);
    drop(writer);let n3=admin.freeze(id(0xa3),admin.manual_target(file).unwrap()).unwrap();

    let writer=a.open_writer(name,file).unwrap();
    let q=writer.save(id(0x98),Some(q.key.save),Some(0),6,&stage(&t,"round-four",b"XY")).unwrap();
    assert_eq!((q.key.round,q.key.revision),(id(0x98),1));
    assert_eq!(count_manual(&source,file),3);
    drop(writer);let n4=admin.freeze(id(0xa4),admin.manual_target(file).unwrap()).unwrap();
    assert_eq!(count_manual(&source,file),4);
    assert_eq!(std::fs::read_dir(f.join(format!("Four-{file}.xlsx"))).unwrap().count(),17);

    let cold=InitialReader::open(&s,pool).unwrap();let session=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let reader=session.connect(ro).unwrap();
    for (node,bytes) in [(n1.node,b"AxyDE".as_slice()),(n2.node,b"Axy\0\0\0"),(n3.node,b"Axy\0W\0"),(n4.node,b"XYy\0W\0")] {
        let found=reader.open_version(name,node).unwrap();let mut all=Vec::new();
        found.range_to(0,found.size(),&mut all).unwrap();assert_eq!(all,bytes);
    }
    let mut all=Vec::new();let latest=reader.open_current(name).unwrap();
    latest.range_to(0,latest.size(),&mut all).unwrap();assert_eq!(all,b"XYy\0W\0");
}
#[test]
#[ignore="source-only real partial append, original FILE EX and two SQLite connections; no fault against user's media"]
fn true_next_writer_preserves_partial_tail_before_raising_the_next_original_window() {
    use std::os::unix::fs::FileExt;
    use crate::system::reader::smb::work_change::Cut;
    let t=PrivateTemp::under(&std::env::temp_dir());
    let (s,pool,f,rw,ro)=ordinary_pool(&t);let src=InitialReader::open(&s,pool).unwrap();
    let independent=InitialReader::open(&s,pool).unwrap();
    let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let a=smb.connect(rw).unwrap();let b=smb.connect(ro).unwrap();
    let id=|x:u8|OperationId([x;16]);let name="Gap";
    let file=a.create_file(id(0xc1),name).unwrap().file;
    let old=a.open_writer(name,file).unwrap();
    let first=old.save(id(0xc2),None,Some(0),4,&stage(&t,"first",b"ABCD")).unwrap();
    let held=b.open_current(name).unwrap();let raw=f.join(format!("Gap-{file}/change-{}-00000000",id(0xc2)));
    let pending=stage(&t,"partial",b"00");
    assert!(old.save_after(id(0xc3),Some(first.key.save),Some(1),4,&pending,|p|{
        assert!(p==Cut::Reserved);
        let fd=OpenOptions::new().write(true).open(&raw)?;
        fd.write_all_at(b"0",4)?; // exact partial raw prefix, not confirmed
        Err(io::Error::other("interrupted before original prepare"))
    }).is_err());
    assert_eq!(old.receipt(id(0xc3)).unwrap(),None);drop(old);
    let next=a.open_writer(name,file).unwrap();
    let q=next.save(id(0xc4),Some(first.key.save),Some(1),4,&stage(&t,"successor",b"z")).unwrap();
    assert_eq!((q.key.round,q.key.revision),(first.key.round,2));
    assert_eq!(std::fs::read(&raw).unwrap(),b"ABCD0z");
    let p=independent.at(|db,_|{
        let q=db.prepare(c"SELECT raw_at FROM work_edit WHERE edit_op=?1")?;q.bind_bytes(1,&q_id())?;
        if unsafe{sqlite3_step(q.0)}!=100{return Err(invalid("missing actual successor"));}
        let v=q.integer(0)?;q.done()?;Ok(v)
    }).unwrap();
    fn q_id()->[u8;16]{[0xc4;16]}
    assert_eq!(p,5);
    let mut seen=Vec::new();held.range_to(0,4,&mut seen).unwrap();assert_eq!(seen,b"ABCD");
    seen.clear();b.open_current(name).unwrap().range_to(0,4,&mut seen).unwrap();assert_eq!(seen,b"AzCD");
    assert_eq!(count_manual(&independent,file),0);
}

#[test]
#[ignore="real two-file append and confirmed SQL source; no SMB listener or client test"]
fn one_flush_uses_one_work_save_for_disjoint_writes_and_a_truncate_expand_prefix() {
    use crate::ordinary::working_round::Span;
    let tip=PrivateTemp::under(&std::env::temp_dir());
    let (s,p,f,rw,ro)=ordinary_pool(&tip);
    let src=InitialReader::open(&s,p).unwrap();let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
    let a=session.connect(rw).unwrap();let b=session.connect(ro).unwrap();
    let name="Packed.txt";let op=|x:u8|OperationId([x;16]);
    let file=a.create_file(op(0xd1),name).unwrap().file;
    let wr=a.open_writer(name,file).unwrap();
    let first=wr.save(op(0xd2),None,Some(0),5,&stage(&tip,"base-input",b"ABCDE")).unwrap();
    let old=b.open_current(name).unwrap();
    let packed=stage(&tip,"changed",b"xyz");let spans=[Span{at:1,len:1},Span{at:4,len:2}];
    assert!(wr.save_packed(op(0xd3),Some(first.key.save),3,7,&[Span{at:6,len:2},spans[0]],&packed).is_err());
    assert_eq!(wr.receipt(op(0xd3)).unwrap(),None);
    let q=wr.save_packed(op(0xd3),Some(first.key.save),3,7,&spans,&packed).unwrap();
    assert_eq!((q.key.round,q.key.revision,q.size),(first.key.round,2,7));
    assert_eq!(wr.save_packed(op(0xd3),Some(first.key.save),3,7,&spans,&packed).unwrap(),q);
    let dir=f.join(format!("Packed-{file}.txt"));
    let raw=dir.join(format!("change-{}-00000000",first.key.round));
    assert_eq!(std::fs::read(raw).unwrap(),b"ABCDExyz");
    assert_eq!(std::fs::read_dir(dir).unwrap().count(),5);
    assert_eq!(count_manual(&src,file),0);
    let mut v=Vec::new();old.range_to(0,5,&mut v).unwrap();assert_eq!(v,b"ABCDE");
    v.clear();b.open_current(name).unwrap().range_to(0,7,&mut v).unwrap();assert_eq!(v,b"AxC\0yz\0");
}

#[test]
#[ignore="source-only two separate original FILE phase windows; no product or media run"]
fn replacing_unconfirmed_new_rounds_keeps_only_names_that_were_actually_created() {
    use crate::system::reader::smb::work_change::Cut;
    let t=PrivateTemp::under(&std::env::temp_dir());let (site,p,f,rw,ro)=ordinary_pool(&t);
    let r=InitialReader::open(&site,p).unwrap();let a=InitialReader::open(&site,p).unwrap();
    let admin=a.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let smb=SmbSession::fixture_authenticated(&r,"files.smb").unwrap();
    let write=smb.connect(rw).unwrap();let read=smb.connect(ro).unwrap();
    let op=|x:u8|OperationId([x;16]);let name="Second";
    let file=write.create_file(op(0xe0),name).unwrap().file;
    let writer=write.open_writer(name,file).unwrap();
    let base=writer.save(op(0xe1),None,Some(0),4,&stage(&t,"forever",b"AAAA")).unwrap();
    drop(writer);let first=admin.freeze(op(0xe2),admin.manual_target(file).unwrap()).unwrap();

    let file_dir=f.join(format!("Second-{file}"));
    let abandoned=|x:u8|file_dir.join(format!("change-{}-00000000",op(x)));
    let change=stage(&t,"one-byte",b"Z");
    let lost=op(0xe3);let partial=op(0xe4);let good=op(0xe5);
    let w=write.open_writer(name,file).unwrap();let mut reserved=false;
    assert!(w.save_after(lost,Some(base.key.save),Some(0),4,&change,|c|{
        assert!(c==Cut::Reserved);reserved=true;Err(io::Error::other("left before any names"))
    }).is_err());assert!(reserved);drop(w);
    assert!(!abandoned(0xe3).exists());

    let w=write.open_writer(name,file).unwrap();let mut physical=false;
    assert!(w.save_after(partial,Some(base.key.save),Some(0),4,&change,|c|{
        assert!(c==Cut::Reserved);
        let mut f=OpenOptions::new().write(true).create_new(true).mode(0o600).open(abandoned(0xe4))?;
        f.write_all(b"t")?;f.sync_all()?;
        File::open(&file_dir)?.sync_all()?;physical=true;
        Err(io::Error::other("disconnected after one real reserved role name"))
    }).is_err());assert!(physical);drop(w);

    let w=write.open_writer(name,file).unwrap();
    let receipt=w.save(good,Some(base.key.save),Some(0),4,&change).unwrap();
    assert_eq!((receipt.key.round,receipt.key.revision),(good,1));drop(w);
    assert_eq!(std::fs::read(&abandoned(0xe4)).unwrap(),b"t");
    assert_eq!(std::fs::read_dir(&file_dir).unwrap().count(),10);
    a.at(|db,_|{
        for (current,old,mask) in [(partial,lost,0),(good,partial,1)]{
            let q=db.prepare(c"SELECT previous_pending,retained_mask FROM main.work_edit WHERE edit_op=?1 AND file_id=?2")?;
            q.bind_bytes(1,&current.0)?;q.bind_bytes(2,&file.0)?;
            if unsafe{sqlite3_step(q.0)}!=100{return Err(invalid("missing exact retained work row"));}
            assert_eq!((q.array::<16>(0)?,q.byte(1)?),(old.0,mask));q.done()?;
        }
        Ok(())
    }).unwrap();
    let second=admin.freeze(op(0xe6),admin.manual_target(file).unwrap()).unwrap();
    assert_eq!(second.target.parent,Some(first.original));
    let cold=InitialReader::open(&site,p).unwrap();
    let selected=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let tree=selected.connect(ro).unwrap();let mut bytes=Vec::new();
    tree.open_version(name,first.node).unwrap().range_to(0,4,&mut bytes).unwrap();assert_eq!(bytes,b"AAAA");
    bytes.clear();tree.open_version(name,second.node).unwrap().range_to(0,4,&mut bytes).unwrap();assert_eq!(bytes,b"ZAAA");
    assert!(read.open_current(name).is_ok());
}
