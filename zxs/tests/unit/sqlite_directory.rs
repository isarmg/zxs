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
    let ws=SmbSession::fixture_authenticated(&write,"files.smb").unwrap();let w=ws.connect(rw).unwrap();
    let rs=SmbSession::fixture_authenticated(&read,"files.smb").unwrap();let r=rs.connect(ro).unwrap();
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
    let ws=SmbSession::fixture_authenticated(&write,"files.smb").unwrap();let w=ws.connect(rw).unwrap();
    let rs=SmbSession::fixture_authenticated(&read,"files.smb").unwrap();let r=rs.connect(ro).unwrap();
    let web=write.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let a=w.create_file(OperationId([0xe1;16]),"a").unwrap();
    let c=w.create_file(OperationId([0xe2;16]),"c").unwrap();
    let a_selected=r.root_files().unwrap().next(1).unwrap().remove(0);
    let mut l=r.root_files().unwrap();
    let witness=f.join(format!(".zxs-new-{}-7",a.created));let keep=f.join("held-aside");
    assert!(l.next_after(1,||std::fs::rename(&witness,&keep)).is_err());
    std::fs::rename(&keep,&witness).unwrap();assert_eq!(l.next(1).unwrap()[0].file,a.file);
    let res=l.next_after(1,||{w.create_file(OperationId([0xe3;16]),"b")?;Ok(())}).unwrap_err();
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
