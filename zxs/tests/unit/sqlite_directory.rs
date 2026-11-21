//! A page is produced by the existing authorized SMB business Tree, not by a
//! network listener. Both S and F below are fresh, real fixtures. The missing
//! R and the successful authentication event aren't faked as real SMB I/O.
use super::*;
use crate::system::reader::smb::file_create::Point;

#[test]
#[ignore="source-only; two real SQL connections and new ordinary F, no Windows authentication"]
fn original_shared_namespace_page_is_not_a_native_directory_dump_or_body_check(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let write=InitialReader::open(&s,p).unwrap();let read=InitialReader::open(&s,p).unwrap();
    let ws=SmbSession::fixture_authenticated(&write,"first.admin").unwrap();let w=ws.connect(rw).unwrap();
    let rs=SmbSession::fixture_authenticated(&read,"first.admin").unwrap();let r=rs.connect(ro).unwrap();
    let mut empty=r.root_files().unwrap();assert!(empty.next(3).unwrap().is_empty());
    let a=w.create_file(OperationId([0xd0;16]),"A").unwrap();
    let b=w.create_file(OperationId([0xd1;16]),"a").unwrap();
    let n=w.create_file(OperationId([0xd2;16]),NAME).unwrap();
    let saved=input(&t,"received",b"seven!!");w.save_first(SAVE,NAME,n.file,&saved).unwrap();
    let zero=input(&t,"zero",&[]);w.save_first(OperationId([0xd3;16]),"A",a.file,&zero).unwrap();
    let mut saw_uncommitted_birth=false;
    assert!(w.create_after(OperationId([0xd4;16]),"native-installed",|pt|if pt==Point::Installed{
        saw_uncommitted_birth=true;
        Err(io::Error::other("actual terminal not yet selected by SQL"))
    }else{Ok(())}).is_err());
    assert!(saw_uncommitted_birth,"must have installed the real unconfirmed FILE before listing");
    std::fs::write(f.join("not-a-business-FILE"),b"LOCAL").unwrap();
    assert!(empty.next(3).unwrap().is_empty()); // end is sticky; reopen to see new names
    let mut list=r.root_files().unwrap();assert!(list.next(0).is_err());assert!(list.next(17).is_err());
    let first=list.next(2).unwrap();
    assert_eq!(first.iter().map(|x|(x.name.as_str(),x.file,x.entry,x.revision,x.size)).collect::<Vec<_>>(),
        [("A",a.file,a.entry,1,0),("a",b.file,b.entry,0,0)]);
    let last=list.next(2).unwrap();assert_eq!(last.len(),1);
    assert_eq!((last[0].name.as_str(),last[0].file,last[0].revision,last[0].size),(NAME,n.file,1,7));
    assert!(list.next(2).unwrap().is_empty());assert!(!read.snapshot().unwrap().rows()[1].path.exists());
    // Listing -> current OPEN -> a verified subrange. The selected member
    // survives an alias share; the newly opened tree must supply its own
    // current permission. A name from a different real FILE isn't enough.
    let mut bytes=Vec::new();
    let chosen=r.open_listed(&last[0]).unwrap();assert_eq!(chosen.file(),n.file);
    chosen.range_to(1,4,&mut bytes).unwrap();assert_eq!(bytes,b"even");
    assert_eq!(w.open_listed(&last[0]).unwrap().file(),n.file);
    let mut mismatched=w.root_files().unwrap().next(16).unwrap();
    mismatched[0].name="a".to_owned();assert!(w.open_listed(&mismatched[0]).is_err());
    // File "a" existed but was unsaved in this listing. A new current OPEN
    // observes its subsequently confirmed first SAVE; it isn't version 0.
    w.save_first(OperationId([0xd5;16]),"a",b.file,&saved).unwrap();
    let opened=r.open_listed(&first[1]).unwrap();assert_eq!((opened.file(),opened.working_revision(),opened.size()),(b.file,1,7));
    // The SQL size describes a confirmed revision, not verified file data.
    let raw=f.join(format!("方案-{}.xlsx/change-{SAVE}-00000000",n.file));
    let swapped=OpenOptions::new().write(true).open(raw).unwrap();
    swapped.write_all_at(b"broken!",0).unwrap();swapped.sync_all().unwrap();drop(swapped);
    let again=w.root_files().unwrap().next(16).unwrap();assert_eq!(again.len(),3);assert_eq!(again[2],last[0]);
    let mut none=Vec::new();assert!(r.open_listed(&last[0]).unwrap().range_to(0,1,&mut none).is_err());
    assert!(none.is_empty());
}

#[test]
#[ignore="source-only; same FILE/header, cursor retry and two real revoke/regrant transactions"]
fn failed_disclosure_never_moves_the_cursor_or_refreshes_the_grant(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,f,rw,ro)=enrollment(&t);
    let write=InitialReader::open(&s,p).unwrap();let read=InitialReader::open(&s,p).unwrap();
    let ws=SmbSession::fixture_authenticated(&write,"first.admin").unwrap();let w=ws.connect(rw).unwrap();
    let rs=SmbSession::fixture_authenticated(&read,"first.admin").unwrap();let r=rs.connect(ro).unwrap();
    let web=write.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let a=w.create_file(OperationId([0xe1;16]),"a").unwrap();
    let c=w.create_file(OperationId([0xe2;16]),"c").unwrap();
    let a_selected=r.root_files().unwrap().next(1).unwrap().remove(0);
    let mut l=r.root_files().unwrap();
    let witness=f.join(format!(".zxs-new-{}-7",a.created));let keep=f.join("held-aside");
    assert!(l.next_after(1,||std::fs::rename(&witness,&keep)).is_err());
    std::fs::rename(&keep,&witness).unwrap();assert_eq!(l.next(1).unwrap()[0].file,a.file);
    let res=l.next_after(1,||{
        assert_ne!(unsafe{crate::system::reader::sqlite3_get_autocommit(read.conn.0)},0,
                   "ordinary name/FILE checks must not keep SYSTEM in a transaction");
        w.create_file(OperationId([0xe3;16]),"b")?;Ok(())
    }).unwrap_err();
    assert_eq!(res.kind(),io::ErrorKind::WouldBlock);assert_eq!(l.next(1).unwrap()[0].name,"b");
    assert!(l.next_after(1,||{
        let was=web.share_setting(ro)?;
        let off=web.set_share_mode(OperationId([0xe4;16]),ro,was.op,Mode::Disabled)?;
        web.set_share_mode(OperationId([0xe5;16]),ro,off.op,Mode::ReadOnly)?;Ok(())
    }).is_err());assert!(l.next(1).is_err());
    assert!(r.open_listed(&a_selected).is_err());
    let new=rs.connect(ro).unwrap();assert_eq!(new.open_listed(&a_selected).unwrap().file(),a.file);let remaining=new.root_files().unwrap().next(16).unwrap();
    assert_eq!(remaining.iter().map(|e|e.name.as_str()).collect::<Vec<_>>(),["a","b","c"]);
    assert_eq!(remaining[2].file,c.file);
}


#[test]
#[ignore="source-only; two business trees and independent cold SQLite/ordinary FILE opens, not SMB locks"]
fn original_number_survives_alias_share_revision_and_reopen(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p,_,rw,ro)=enrollment(&t);
    let op=OperationId([0xeb;16]);
    let (id,key)={
        let src=InitialReader::open(&s,p).unwrap();let sess=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
        let a=sess.connect(rw).unwrap();let b=sess.connect(ro).unwrap();
        let first=a.create_file(op,"invoices").unwrap();let other=a.create_file(OperationId([0xec;16]),"another").unwrap();
        let old=b.open_current("invoices").unwrap();let key=old.native_key().unwrap();
        assert_ne!(key.words().1,b.open_current("another").unwrap().native_key().unwrap().words().1);
        assert_ne!(first.file,other.file);
        let incoming=input(&t,"received-edits",b"same member, new revision");
        a.save_first(OperationId([0xed;16]),"invoices",first.file,&incoming).unwrap();
        assert_eq!(old.working_revision(),0);assert_eq!(old.native_key().unwrap(),key);
        let picked=a.open_current("invoices").unwrap();assert_eq!(picked.working_revision(),1);assert_eq!(picked.native_key().unwrap(),key);
        assert_eq!(a.create_file(op,"invoices").unwrap(),first);(first.file,key)
    };
    let cold=InitialReader::open(&s,p).unwrap();let sess=SmbSession::fixture_authenticated(&cold,"first.admin").unwrap();
    let tree=sess.connect(ro).unwrap();let got=tree.open_current("invoices").unwrap();
    assert_eq!((got.file(),got.native_key().unwrap()),(id,key));
    let web=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let was=web.share_setting(ro).unwrap();web.set_share_mode(OperationId([0xee;16]),ro,was.op,Mode::Disabled).unwrap();
    assert!(got.native_key().is_err());
}

#[test]
#[ignore="source-only; actual SQLite and FILE, test-only authenticated principal, no Windows session"]
fn internal_share_choice_does_not_reuse_an_old_current_grant(){
    use std::io::ErrorKind;
    let t=PrivateTemp::under(&std::env::temp_dir());let (s,p,_,rw,ro)=enrollment(&t);
    let src=InitialReader::open(&s,p).unwrap();let sess=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();
    let w=sess.connect(rw).unwrap();
    let r=sess.connect(ro).unwrap();
    let made=w.create_file(OperationId([0xbd;16]),"reports").unwrap();
    assert_eq!(r.open_current("reports").unwrap().file(),made.file);
    assert!(r.create_file(OperationId([0xbe;16]),"another").is_err());
    let mut unknown=rw;for k in 1..=2{unknown.0[0]=rw.0[0]^k;if unknown!=ro{break;}}
    let e=sess.connect(unknown).err().expect("another internal share is not an authorized choice");
    assert_eq!(e.kind(),ErrorKind::PermissionDenied);
    let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let was=web.share_setting(ro).unwrap();
    let off=web.set_share_mode(OperationId([0xbc;16]),ro,was.op,Mode::Disabled).unwrap();
    let clean=sess.connect(ro).err().expect("disabled share has no grant");
    assert_eq!(clean.kind(),ErrorKind::PermissionDenied);assert!(r.open_current("reports").is_err());
    assert_eq!(w.open_current("reports").unwrap().file(),made.file);
    web.set_share_mode(OperationId([0xbb;16]),ro,off.op,Mode::ReadOnly).unwrap();
    assert!(r.open_current("reports").is_err()); // a new mode does not revive the old Tree
    assert_eq!(sess.connect(ro).unwrap().open_current("reports").unwrap().file(),made.file);
}
