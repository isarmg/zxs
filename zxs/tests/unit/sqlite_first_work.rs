//! No listener or CLI and never an existing volume. Only the completed
//! Samba-login proof is supplied by cfg(test); SQL, locks, FILEs, block
//! digests, sockets' potential short writes/flush and cold reopens are not
//! replaced by a catalog model. Nothing in this file was executed here.
use super::*;
use crate::{FileId,identity::ShareId,system::reader::{account::share_mode::Mode,smb::{SmbSession,first_work::Cut}}};
use std::os::unix::fs::{FileExt,MetadataExt};
fn input(t:&PrivateTemp,name:&str,body:&[u8])->File{
    let f=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(t.0.join(name)).unwrap();
    f.write_all_at(body,0).unwrap();f.sync_all().unwrap();f
}
fn enrollment(t:&PrivateTemp)->(PathBuf,PoolId,PathBuf,ShareId,ShareId){
    let(s,p)=publication::first(t);let src=InitialReader::open(&s,p).unwrap();let f=create_file::open_root(&src);
    let(rw,ro)=create_file::configure(&src);(s,p,f,rw,ro)
}
const NAME:&str="方案.xlsx";
const SAVE:OperationId=OperationId([0xb2;16]);
fn pending(s:&InitialReader,op:OperationId)->Option<i64>{
    s.at(|db,_|{let q=db.prepare(c"SELECT state FROM main.operation WHERE kind=6 AND op_id=?1")?;
        q.bind_bytes(1,&op.0)?;match unsafe{sqlite3_step(q.0)}{101=>Ok(None),100=>{let x=q.integer(0)?;q.done()?;Ok(Some(x))},
            rc=>{check(rc)?;Err(invalid("bad fixture working result"))}}
    }).unwrap()
}
#[test]
#[ignore="separate real SQLite+filesystem source qualification; no network or user media"]
fn first_manual_save_reads_by_name_after_all_handles_close_and_retry_returns_only_the_old_working_revision(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let data=b"ZXS-real-binary-\0\xff\x00end";let body=input(&t,"incoming-good",data);
    let bad=input(&t,"incoming-different",b"not identical");let zero=input(&t,"empty",b"");
    let(first,made)={
        let src=InitialReader::open(&s,p).unwrap();let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let writer=smb.connect(rw).unwrap();let reader=smb.connect(ro).unwrap();
        let made=writer.create_file(OperationId([0xb1;16]),NAME).unwrap();
        let before=reader.open_current(NAME).unwrap();assert_eq!((before.file(),before.size(),before.working_revision()),(made.file,0,0));
        let mut none=Vec::new();before.range_to(0,0,&mut none).unwrap();assert!(none.is_empty());drop(before);
        assert!(reader.save_first(SAVE,NAME,made.file,&body).is_err());assert_eq!(pending(&src,SAVE),None);
        let result=writer.save_first(SAVE,NAME,made.file,&body).unwrap();
        assert_eq!((result.file,result.save,result.revision,result.bytes,result.sha256),(made.file,SAVE,1,data.len() as u64,crate::hash::digest(data).unwrap()));
        assert!(writer.save_first(SAVE,NAME,made.file,&bad).is_err());
        assert!(writer.save_first(OperationId([0xb3;16]),NAME,made.file,&body).is_err());
        assert_eq!(writer.first_save_receipt(SAVE,NAME,made.file).unwrap(),Some(result));
        assert_eq!(pending(&src,SAVE),Some(1));
        // Even if the display name equals this actual, now nonempty FILE
        // terminal, the two business entries are separate.
        let leaf=format!("方案-{}.xlsx",made.file);let another=writer.create_file(OperationId([0xb4;16]),&leaf).unwrap();
        assert_ne!(another.file,made.file);
        let z=writer.save_first(OperationId([0xb5;16]),&leaf,another.file,&zero).unwrap();
        assert_eq!((z.revision,z.bytes),(1,0));let h=reader.open_current(&leaf).unwrap();assert_eq!(h.working_revision(),1);
        h.range_to(0,0,&mut none).unwrap();(result,made)
    };
    let cold=InitialReader::open(&s,p).unwrap();let smb=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let tree=smb.connect(ro).unwrap();assert!(tree.open_current("方案.XLSX").is_err());
    let h=tree.open_current(NAME).unwrap();assert_eq!((h.file(),h.size(),h.working_revision()),(made.file,first.bytes,1));
    let mut all=Vec::new();h.range_to(0,h.size(),&mut all).unwrap();assert_eq!(all,data);
    let mut tail=Vec::new();h.range_to(3,4,&mut tail).unwrap();assert_eq!(tail,&data[3..7]);
    assert!(h.range_to(first.bytes,1,Vec::new()).is_err());drop(h);
    assert_eq!(smb.connect(rw).unwrap().save_first(SAVE,NAME,made.file,&body).unwrap(),first);
    let d=f.join(format!("方案-{}.xlsx",made.file));let raw=d.join(format!("change-{SAVE}-00000000"));
    let pin=d.join(format!("pin-data-{SAVE}-00000000"));assert_eq!(std::fs::metadata(&raw).unwrap().ino(),std::fs::metadata(&pin).unwrap().ino());
    assert_eq!(std::fs::metadata(&raw).unwrap().nlink(),2);assert_eq!(std::fs::read_dir(d).unwrap().count(),5);
    assert!(!cold.snapshot().unwrap().rows()[1].path.exists());
}

#[test]
#[ignore="real working bytes synced before SQL, exclusive per-FILE recovery and append-only verified prefix"]
fn synced_not_yet_confirmed_remains_logically_empty_and_same_original_can_recover_through_an_independent_open(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let input=input(&t,"incoming",b"abcdefghijk");let id={
        let src=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
        let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let tree=smb.connect(rw).unwrap();
        let sb=SmbSession::fixture_authenticated(&peer,"files.smb").unwrap();let other=sb.connect(rw).unwrap();
        let id=tree.create_file(OperationId([0xb1;16]),NAME).unwrap().file;
        let mut saw_uncommitted_page=false;
        assert!(tree.save_first_after(SAVE,NAME,id,&input,|cut|{
            if cut==Cut::Reserved{assert_eq!(pending(&peer,SAVE),Some(0));
                other.create_file(OperationId([0xb3;16]),"sibling")?; // no volume flock or SQL writer held
                let d=f.join(format!("方案-{id}.xlsx"));assert_eq!(std::fs::read_dir(d)?.count(),1);}
            if cut==Cut::BytesSynced{
                let visible=other.root_files()?.next(16)?.into_iter().find(|x|x.file==id).unwrap();
                assert_eq!((visible.revision,visible.size),(0,0)); // real synced bytes are still private
                saw_uncommitted_page=true;
                return Err(io::Error::other("interrupt after named sync before SQL"));
            }Ok(())
        }).is_err());assert!(saw_uncommitted_page,"must have observed the original FILE at BytesSynced");
        assert_eq!(pending(&src,SAVE),Some(0));
        assert_eq!(other.open_current("sibling").unwrap().working_revision(),0);id
    };
    let cold=InitialReader::open(&s,p).unwrap();let smb=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let tree=smb.connect(rw).unwrap();let peer=smb.connect(ro).unwrap();let previous=peer.open_current(NAME).unwrap();
    assert_eq!((previous.file(),previous.size(),previous.working_revision()),(id,0,0));drop(previous);
    let raw=f.join(format!("方案-{id}.xlsx/change-{SAVE}-00000000"));
    let file=OpenOptions::new().read(true).write(true).open(&raw).unwrap();
    file.write_all_at(b"Z",0).unwrap();file.sync_all().unwrap();
    assert!(tree.save_first(SAVE,NAME,id,&input).is_err());assert_eq!(std::fs::read(&raw).unwrap(),b"Zbcdefghijk");
    assert_eq!(pending(&cold,SAVE),Some(0));assert_eq!(tree.first_save_receipt(SAVE,NAME,id).unwrap(),None);
    // Simulate a reachable pending prefix: do not rewrite or truncate it;
    // the worker may append only its missing exact suffix.
    file.write_all_at(b"a",0).unwrap();file.set_len(3).unwrap();file.sync_all().unwrap();drop(file);
    assert_eq!(tree.save_first(SAVE,NAME,id,&input).unwrap().revision,1);
    let h=peer.open_current(NAME).unwrap();let mut bytes=Vec::new();h.range_to(0,h.size(),&mut bytes).unwrap();assert_eq!(bytes,b"abcdefghijk");
}

#[test]
#[ignore="two real SQL connections, original per-FILE lease through a slow sink; no service or media probe"]
fn revoked_before_confirm_or_during_delivery_is_not_a_new_grant_and_bad_blocks_are_not_sent(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let src=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
    let manager=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let a=smb.connect(rw).unwrap();
    let b=smb.connect(ro).unwrap();let id=a.create_file(OperationId([0xb1;16]),NAME).unwrap().file;
    let other=a.create_file(OperationId([0xb9;16]),"other").unwrap().file;
    let block=4*1024*1024;let mut content=vec![23u8;block+2];content[block]=42;content[block+1]=43;
    let incoming=input(&t,"incoming",&content);a.save_first(SAVE,NAME,id,&incoming).unwrap();
    let d=f.join(format!("方案-{id}.xlsx"));let raw=d.join(format!("change-{SAVE}-00000000"));
    let disk=OpenOptions::new().write(true).open(raw).unwrap();disk.write_all_at(&[24],0).unwrap();disk.sync_all().unwrap();
    let h=b.open_current(NAME).unwrap();let mut no_leak=Vec::new();assert!(h.range_to(27,1,&mut no_leak).is_err());assert!(no_leak.is_empty());
    let mut good=Vec::new();h.range_to(block as u64,2,&mut good).unwrap();assert_eq!(good,[42,43]);drop(h);
    disk.write_all_at(&[23],0).unwrap();disk.sync_all().unwrap();drop(disk);
    struct Revoke<'a,F:FnMut()->io::Result<()>>{seen:&'a mut Vec<u8>,revoke:F}
    impl<F:FnMut()->io::Result<()>> Write for Revoke<'_,F>{fn write(&mut self,p:&[u8])->io::Result<usize>{self.seen.extend(p);Ok(p.len())}
        fn flush(&mut self)->io::Result<()>{(self.revoke)()}}
    let selected=b.open_current(NAME).unwrap();let mut seen=Vec::new();
    assert!(selected.range_to(1,2,Revoke{seen:&mut seen,revoke:||{
        let old=manager.share_setting(ro)?;manager.set_share_mode(OperationId([0xbc;16]),ro,old.op,Mode::Disabled)?;
        assert!(HeldDir::trusted_system(&s)?.exclusive().is_err());Ok(())}}).is_err());
    assert_eq!(seen,[23,23]);assert!(selected.range_to(1,1,Vec::new()).is_err());drop(selected);
    assert!(a.save_first_after(OperationId([0xba;16]),"other",other,&incoming,|cut|{
        if cut==Cut::BytesSynced{let old=manager.share_setting(rw)?;manager.set_share_mode(OperationId([0xbb;16]),rw,old.op,Mode::ReadOnly)?;}Ok(())
    }).is_err());assert_eq!(pending(&peer,OperationId([0xba;16])),Some(0));
    let newly=smb.connect(rw).unwrap();let unpublished=newly.open_current("other").unwrap();assert_eq!((unpublished.size(),unpublished.working_revision()),(0,0));
}

#[test]
#[ignore="fresh SQLite, real private origin files and 2 independently authenticated connections; no network"]
fn new_rw_can_supersede_only_a_revoked_unconfirmed_attempt_without_adopting_its_names_or_changing_fileid(){
    for stop in [Cut::Reserved,Cut::BytesSynced]{
        let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
        let src=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
        let manage=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let smb=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let old=smb.connect(rw).unwrap();
        let made=old.create_file(OperationId([0xc0;16]),NAME).unwrap();
        let old_op=OperationId([0xc1;16]);let new_op=OperationId([0xc2;16]);
        let before=input(&t,"old-receiver",b"private candidate");let after=input(&t,"new-receiver",b"new admitted bytes");
        assert!(old.save_first_after(old_op,NAME,made.file,&before,|cut|{
            if cut==stop{return Err(io::Error::other("lost preparer"));}Ok(())}).is_err());
        assert!(old.save_first(new_op,NAME,made.file,&after).is_err()); // still a live original
        assert_eq!(pending(&peer,new_op),None);
        let off=manage.set_share_mode(OperationId([0xc3;16]),rw,manage.share_setting(rw).unwrap().op,Mode::ReadOnly).unwrap();
        let empty=smb.connect(ro).unwrap();let v=empty.open_current(NAME).unwrap();assert_eq!(v.working_revision(),0);drop(v);
        manage.set_share_mode(OperationId([0xc4;16]),rw,off.op,Mode::ReadWrite).unwrap();
        let fresh=smb.connect(rw).unwrap();
        assert!(old.save_first(old_op,NAME,made.file,&before).is_err()); // obsolete handle
        assert!(fresh.save_first(old_op,NAME,made.file,&before).is_err()); // new grant does not revive old op
        let dir=f.join(format!("方案-{}.xlsx",made.file));let prior=dir.join(format!("change-{old_op}-00000000"));
        let marker=dir.join(format!("pin-data-{old_op}-00000000"));
        let count=if stop==Cut::BytesSynced{
            assert_eq!(std::fs::metadata(&prior).unwrap().ino(),std::fs::metadata(&marker).unwrap().ino());
            let old_file=OpenOptions::new().write(true).open(&prior).unwrap();
            old_file.write_all_at(b"X",0).unwrap();old_file.sync_all().unwrap();5
        }else{assert!(!prior.exists());1};
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(),count);
        let receipt=fresh.save_first(new_op,NAME,made.file,&after).unwrap();
        assert_eq!((receipt.file,receipt.save,receipt.revision),(made.file,new_op,1));
        assert_eq!(pending(&peer,old_op),Some(0));assert_eq!(pending(&peer,new_op),Some(1));
        assert!(fresh.first_save_receipt(old_op,NAME,made.file).is_err());
        assert!(fresh.save_first(OperationId([0xc5;16]),NAME,made.file,&before).is_err()); // never supersede a committed revision
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(),count+4);
        if stop==Cut::BytesSynced{assert_eq!(std::fs::read(&prior).unwrap(),b"Xrivate candidate");}
        drop(old);drop(empty);drop(fresh);drop(smb);drop(src);drop(manage);drop(peer);
        let cold=InitialReader::open(&s,p).unwrap();let smb=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
        let reader=smb.connect(ro).unwrap();let v=reader.open_current(NAME).unwrap();let mut got=Vec::new();
        v.range_to(0,v.size(),&mut got).unwrap();assert_eq!(got,b"new admitted bytes");assert_eq!(v.file(),made.file);drop(v);
        if stop==Cut::Reserved{
            // This name was not physically present/charged when that attempt
            // lost its grant. Knowing its spelling now must not permit it.
            let unexpected=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&prior).unwrap();
            assert!(reader.open_current(NAME).is_err());drop(unexpected);std::fs::remove_file(&prior).unwrap();
            assert_eq!(reader.open_current(NAME).unwrap().size(),18);
        }
    }
}

#[test]
#[ignore="real SQLite and private ordinary files; does not exercise SMB share-access or a Windows client"]
fn compatible_read_snapshots_cross_a_real_first_writer_in_both_directions(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,_,rw,ro)=enrollment(&t);
    let a=InitialReader::open(&s,p).unwrap();let b=InitialReader::open(&s,p).unwrap();
    let sa=SmbSession::fixture_authenticated(&a,"files.smb").unwrap();
    let sb=SmbSession::fixture_authenticated(&b,"files.smb").unwrap();
    let writer=sa.connect(rw).unwrap();let reader=sb.connect(ro).unwrap();let competing=sb.connect(rw).unwrap();
    let id=writer.create_file(OperationId([0xd1;16]),NAME).unwrap().file;
    let file=input(&t,"received",b"never disclose this before its original SQL commit");
    let old=reader.open_current(NAME).unwrap();assert_eq!((old.file(),old.working_revision(),old.size()),(id,0,0));
    let mut cuts=0;
    let at=writer.save_first_after(SAVE,NAME,id,&file,|cut|{
        // The original writer still holds the preparation EX. Even after
        // all four true files have been synced, a second independently
        // opened connection may read the confirmed empty predecessor.
        assert_eq!(pending(&b,SAVE),Some(0));
        let early=reader.open_current(NAME)?;assert_eq!((early.file(),early.size(),early.working_revision()),(id,0,0));
        let mut sent=Vec::new();early.range_to(0,0,&mut sent)?;
        assert!(early.range_to(0,1,&mut sent).is_err());assert!(sent.is_empty());
        old.range_to(0,0,&mut sent)?;
        // Two true writers never prepare the identical Op concurrently.
        assert!(competing.save_first(SAVE,NAME,id,&file).is_err());
        if cut==Cut::BytesSynced{
            assert!(competing.save_first(OperationId([0xd2;16]),NAME,id,&file).is_err());
            assert_eq!(pending(&b,OperationId([0xd2;16])),None);
        }
        cuts+=1;Ok(())
    }).unwrap();
    assert_eq!((cuts,at.revision,pending(&b,SAVE)),(2,1,Some(1)));
    // The still-open older handle does not follow HEAD to new bytes. Its
    // real header/root, source lease and current read authority remain in
    // force. A later independent selection sees the new revision instead.
    let mut old_bytes=Vec::new();old.range_to(0,0,&mut old_bytes).unwrap();
    assert!(old.range_to(0,1,&mut old_bytes).is_err());assert!(old_bytes.is_empty());
    let now=reader.open_current(NAME).unwrap();let mut read=Vec::new();now.range_to(0,now.size(),&mut read).unwrap();
    assert_eq!((now.file(),now.working_revision()),(id,1));assert_eq!(read,b"never disclose this before its original SQL commit");
    let manager=b.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let prior=manager.share_setting(ro).unwrap();manager.set_share_mode(OperationId([0xd3;16]),ro,prior.op,Mode::Disabled).unwrap();
    assert!(old.range_to(0,0,Vec::new()).is_err());assert!(now.range_to(0,1,Vec::new()).is_err());
}

#[path="sqlite_directory.rs"]
mod directory;
