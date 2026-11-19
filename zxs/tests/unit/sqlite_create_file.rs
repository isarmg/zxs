//! Source fixtures for the storage business boundary. An isolated real SQL
//! pool + F are used; the successful Samba login event is supplied ONLY by
//! cfg(test). No Windows client, smbd listener, production credential or R.
use super::*;
use crate::{FileId,VolumeId,identity::ShareId,system::reader::{account::share_mode::Mode,smb::{SmbSession,file_create::Point}}};
use std::os::unix::fs::{DirBuilderExt,MetadataExt};
const F:VolumeId=VolumeId([0x13;16]);
pub(super) fn open_root(src:&InitialReader)->PathBuf{
    let c=src.snapshot().unwrap();let f=&c.rows()[2];
    std::fs::DirBuilder::new().mode(0o700).create(&f.path).unwrap();
    let mut m=OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(f.path.join(OsStr::from_bytes(ROOT_MARK.to_bytes()))).unwrap();
    m.write_all(&c.expected_marker(2).unwrap()).unwrap();m.sync_all().unwrap();
    File::open(&f.path).unwrap().sync_all().unwrap();f.path.clone()
}
pub(super) fn configure(src:&InitialReader)->(ShareId,ShareId){
    let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let a=web.publish_root(OperationId([0x81;16]),F,"writable").unwrap();
    let b=web.publish_root(OperationId([0x82;16]),F,"view").unwrap();
    let smb=SmbSession::fixture_authenticated(src,"files.smb").unwrap();
    assert!(smb.connect(a.share).is_err());assert!(SmbSession::fixture_authenticated(src,"first.admin").is_err());
    web.set_share_mode(OperationId([0x83;16]),a.share,a.op,Mode::ReadWrite).unwrap();
    web.set_share_mode(OperationId([0x84;16]),b.share,b.op,Mode::ReadOnly).unwrap();(a.share,b.share)
}
fn state(src:&InitialReader,op:OperationId)->Option<(i64,FileId,OperationId)>{
    src.at(|db,_|{let s=db.prepare(c"SELECT o.state,e.file_id,e.ticket FROM main.operation o JOIN main.root_entry e ON e.create_op=o.op_id WHERE o.op_id=?1")?;
        s.bind_bytes(1,&op.0)?;
        match unsafe{sqlite3_step(s.0)}{101=>Ok(None),100=>{let b=(s.integer(0)?,FileId(s.array(1)?),OperationId(s.array(2)?));s.done()?;Ok(Some(b))},
        rc=>{check(rc)?;Err(invalid("unexpected test row"))}}
    }).unwrap()
}
#[test]
#[ignore="source-only qualification, non-root protected local SQLite and F; no actual network authentication"]
fn same_real_file_cold_reopens_across_authorized_shares_and_lost_response_keeps_one_birth(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=publication::first(&t);
    let op=OperationId([0x91;16]);let (rw,ro,one,two,root)={
        let src=InitialReader::open(&s,p).unwrap();let f=open_root(&src);let(rw,ro)=configure(&src);
        let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let a=session.connect(rw).unwrap();let b=session.connect(ro).unwrap();
        assert!(b.create_file(op,"Report.xlsx").is_err());assert_eq!(state(&src,op),None);
        let one=a.create_file(op,"Report.xlsx").unwrap();let two=a.create_file(OperationId([0x92;16]),"report.xlsx").unwrap();
        assert_ne!(one.file,two.file);assert_eq!(b.newborn("Report.xlsx",one.file).unwrap(),one);
        assert_eq!(b.newborn("report.xlsx",two.file).unwrap(),two);assert!(a.newborn("REPORT.xlsx",one.file).is_err());
        assert!(a.newborn("report.xlsx",one.file).is_err());assert!(a.create_file(op,"other").is_err());
        let second_web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let old=second_web.share_setting(ro).unwrap();second_web.set_share_mode(OperationId([0x85;16]),ro,old.op,Mode::ReadWrite).unwrap();
        let another=session.connect(ro).unwrap();assert!(another.create_file(OperationId([0x93;16]),"Report.xlsx").is_err());
        assert!(state(&src,OperationId([0x93;16])).is_none());assert!(b.newborn("Report.xlsx",one.file).is_err());
        (rw,ro,one,two,f)
    };
    let src=InitialReader::open(&s,p).unwrap();
    let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let a=session.connect(rw).unwrap();
    assert_eq!(a.create_file(op,"Report.xlsx").unwrap(),one);assert_eq!(state(&src,op).unwrap().0,1);
    assert_eq!(session.connect(ro).unwrap().newborn("report.xlsx",two.file).unwrap(),two);
    let container=root.join(format!("Report-{}.xlsx",one.file));let primary=container.join("FILE");
    let witness=root.join(format!(".zxs-new-{op}-7"));let old=std::fs::metadata(&witness).unwrap();
    assert_eq!(old.nlink(),2);assert_eq!(old.ino(),std::fs::metadata(&primary).unwrap().ino());
    // One-sided, byte-identical replacement of a header is not the selected original.
    let copy=container.join("copy-fixture");std::fs::copy(&primary,&copy).unwrap();std::fs::rename(&copy,&primary).unwrap();
    assert!(a.newborn("Report.xlsx",one.file).is_err());assert_eq!(a.create_file(op,"Report.xlsx").unwrap(),one);
    assert!(!src.snapshot().unwrap().rows()[1].path.exists());
}
#[test]
#[ignore="source-only real SQL reservation, NOREPLACE installation, interrupted and fresh sessions"]
fn reservation_precedes_names_and_only_the_exact_synced_header_pair_can_confirm(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=publication::first(&t);let op=OperationId([0x94;16]);
    let(rw,id,root)={
        let src=InitialReader::open(&s,p).unwrap();let root=open_root(&src);let(rw,_)=configure(&src);
        let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let a=session.connect(rw).unwrap();
        assert!(a.create_after(op,"方案.xlsx",|cut|if cut==Point::Reserved{Err(io::Error::other("stop before mkdir"))}else{Ok(())}).is_err());
        let(pending,id,_)=state(&src,op).unwrap();assert_eq!(pending,0);assert_eq!(std::fs::read_dir(&root).unwrap().count(),1);
        assert!(a.newborn("方案.xlsx",id).is_err());
        assert!(a.create_after(op,"方案.xlsx",|cut|if cut==Point::Installed{Err(io::Error::other("stop before SQL commit"))}else{Ok(())}).is_err());
        assert_eq!(state(&src,op).unwrap().0,0);assert!(a.newborn("方案.xlsx",id).is_err());
        assert!(root.join(format!("方案-{id}.xlsx/FILE")).is_file());(rw,id,root)
    };
    let cold=InitialReader::open(&s,p).unwrap();let session=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let a=session.connect(rw).unwrap();let outcome=a.create_file(op,"方案.xlsx").unwrap();assert_eq!(outcome.file,id);
    assert_eq!(a.newborn("方案.xlsx",id).unwrap(),outcome);
    // An unrelated incomplete operation: its name is reserved, not adoptable.
    let partial=OperationId([0x95;16]);
    assert!(a.create_after(partial,"pending",|cut|if cut==Point::Reserved{Err(io::Error::other("stop"))}else{Ok(())}).is_err());
    let(_,_,ticket)=state(&cold,partial).unwrap();let slot=root.join(format!(".zxs-new-{ticket}-0"));
    std::fs::DirBuilder::new().mode(0o700).create(&slot).unwrap();std::fs::write(slot.join("foreign"),b"KEEP").unwrap();
    assert!(a.create_file(partial,"pending").is_err());assert_eq!(state(&cold,partial).unwrap().0,0);
    assert_eq!(std::fs::read(slot.join("foreign")).unwrap(),b"KEEP");
    assert!(a.create_file(OperationId([0x96;16]),"sibling").is_ok());assert!(!cold.snapshot().unwrap().rows()[1].path.exists());
}
#[test]
#[ignore="source-only cut between actual installed data and the final authorized transaction"]
fn disabling_an_inflight_create_does_not_publish_or_turn_the_old_tree_into_a_new_grant(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=publication::first(&t);let src=InitialReader::open(&s,p).unwrap();
    let root=open_root(&src);let(rw,ro)=configure(&src);let peer=InitialReader::open(&s,p).unwrap();
    let manager=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let a=session.connect(rw).unwrap();
    let op=OperationId([0x97;16]);let mut stopped=false;
    assert!(a.create_after(op,"never-disclose",|cut|{if cut==Point::Installed{
        let s=manager.share_setting(rw)?;manager.set_share_mode(OperationId([0x98;16]),rw,s.op,Mode::Disabled)?;stopped=true;
    }Ok(())}).is_err());
    assert!(stopped);let(pending,id,_)=state(&src,op).unwrap();assert_eq!(pending,0);
    assert!(root.join(format!("never-disclose-{id}/FILE")).is_file());assert!(a.create_file(op,"never-disclose").is_err());
    assert!(session.connect(ro).unwrap().newborn("never-disclose",id).is_err());
    let s=manager.share_setting(rw).unwrap();manager.set_share_mode(OperationId([0x99;16]),rw,s.op,Mode::ReadWrite).unwrap();
    assert!(a.create_file(op,"never-disclose").is_err());assert!(session.connect(rw).unwrap().create_file(op,"never-disclose").is_err());
}

#[test]
#[ignore="source-only real stage-name collision at the post-rename pre-confirmation cut"]
fn reoccupied_old_stage_blocks_hot_and_cold_confirmation_without_touching_the_sentinel(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=publication::first(&t);let op=OperationId([0x9a;16]);
    let (rw,id,stray)={
        let src=InitialReader::open(&s,p).unwrap();let root=open_root(&src);let(rw,_)=configure(&src);
        let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let tree=session.connect(rw).unwrap();
        let mut stray=None;assert!(tree.create_after(op,"report",|point|{if point==Point::Installed{
            let (pending,_,ticket)=state(&src,op).unwrap();assert_eq!(pending,0);
            let slot=root.join(format!(".zxs-new-{ticket}-0"));std::fs::DirBuilder::new().mode(0o700).create(&slot)?;
            std::fs::write(slot.join("sentinel"),b"preserve")?;stray=Some(slot);
        }Ok(())}).is_err());
        let(z,id,_)=state(&src,op).unwrap();assert_eq!(z,0);(rw,id,stray.unwrap())
    };
    let cold=InitialReader::open(&s,p).unwrap();let session=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
    let tree=session.connect(rw).unwrap();assert!(tree.create_file(op,"report").is_err());assert_eq!(state(&cold,op).unwrap().0,0);
    assert_eq!(std::fs::read(stray.join("sentinel")).unwrap(),b"preserve");
    std::fs::rename(&stray,t.0.join("displaced-stray")).unwrap();
    assert_eq!(tree.create_file(op,"report").unwrap().file,id);assert_eq!(tree.newborn("report",id).unwrap().file,id);
}

#[test]
#[ignore="source-only native display conflicts at both actual interruption cuts"]
fn late_real_display_directory_blocks_original_create_but_real_terminal_is_not_another_display(){
    for (n,cut) in [Point::Reserved,Point::Installed].into_iter().enumerate(){
        let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=publication::first(&t);
        let op=OperationId([0xa7;16]);
        let(rw,id,root)={
            let src=InitialReader::open(&s,p).unwrap();let root=open_root(&src);let(rw,_)=configure(&src);
            let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();let tree=session.connect(rw).unwrap();
            assert!(tree.create_after(op,"方案.xlsx",|at|{if at==cut{
                let bad=root.join("方案.xlsx");std::fs::DirBuilder::new().mode(0o700).create(&bad)?;std::fs::write(bad.join("keep"),b"X")?;
            }Ok(())}).is_err());
            let (pending,id,_)=state(&src,op).unwrap();assert_eq!(pending,0);
            assert_eq!(root.join(format!("方案-{id}.xlsx")).exists(),n==1);(rw,id,root)
        };
        let cold=InitialReader::open(&s,p).unwrap();let smb=SmbSession::fixture_authenticated(&cold,"files.smb").unwrap();
        let tree=smb.connect(rw).unwrap();assert!(tree.create_file(op,"方案.xlsx").is_err());
        assert_eq!(std::fs::read(root.join("方案.xlsx/keep")).unwrap(),b"X");assert_eq!(state(&cold,op).unwrap().0,0);
        std::fs::rename(root.join("方案.xlsx"),t.0.join("unrelated-dir")).unwrap();
        assert_eq!(tree.create_file(op,"方案.xlsx").unwrap().file,id);
        let derived=format!("方案-{id}.xlsx");let other=tree.create_file(OperationId([0xa8;16]),&derived).unwrap();
        assert_ne!(id,other.file);assert_eq!(tree.newborn(&derived,other.file).unwrap(),other);
        let mark=ROOT_MARK.to_str().unwrap();assert!(tree.create_file(OperationId([0xa9;16]),mark).is_ok());
    }
}
