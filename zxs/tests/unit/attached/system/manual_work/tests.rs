use super::*;
use crate::{system::reader::{ordinary_pool,smb::SmbSession},test_support::PrivateTemp};
use std::{cell::Cell,fs::OpenOptions,io::Write,os::unix::fs::OpenOptionsExt};

#[test]
#[ignore="isolated original FILE and SQLite; last live publication cut source only"]
fn withdrawing_at_the_last_system_cut_publishes_neither_the_node_nor_its_operation(){
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let(s,pool,f,rw,_)=ordinary_pool(&temp);
    let source=InitialReader::open(&s,pool).unwrap();
    let independent=InitialReader::open(&s,pool).unwrap();
    let client=SmbSession::fixture_authenticated(&source,"first.admin").unwrap();let tree=client.connect(rw).unwrap();
    let original=OperationId([0xd1;16]);let save=OperationId([0xd2;16]);let attempt=OperationId([0xd3;16]);
    let name="FinalCut.xlsx";let file=tree.create_file(original,name).unwrap().file;
    let mut material=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(temp.0.join("material")).unwrap();
    material.write_all(b"confirmed draft").unwrap();material.sync_all().unwrap();
    tree.save_first(save,name,file,&material).unwrap();
    let admin=source.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let other=independent.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let expected=admin.manual_target(file).unwrap();assert_eq!(expected.save,save);
    let votes=Cell::new(None);
    let live=||{
        if let Some(n)=votes.get(){votes.set(Some(n+1));if n>=2{return Err(denied());}}
        Ok(())
    };
    // After the real physical proof, the first two votes still allow
    // the final verification and both tentative INSERTs. Only the new
    // terminal vote refuses. No recovery query may ask this dead vote.
    let rejection=admin.freeze_authorized(attempt,expected,live,||{votes.set(Some(0));Ok(())}).unwrap_err();
    assert_eq!(votes.get(),Some(3));assert_eq!(rejection.kind(),io::ErrorKind::PermissionDenied);
    independent.at(|db,_|{
        let q=db.prepare(c"SELECT EXISTS(SELECT 1 FROM main.operation WHERE op_id=?1),
                EXISTS(SELECT 1 FROM main.manual_node WHERE commit_op=?1)")?;
        q.bind_bytes(1,&attempt.0)?;assert!(work_record::start_one(&q)?);
        assert_eq!((q.byte(0)?,q.byte(1)?),(0,0));q.done()
    }).unwrap();
    assert_eq!(other.manual_target(file).unwrap(),expected);
    assert!(other.manual_timeline(file,None).unwrap().rows.is_empty());
    // A newly and genuinely admitted decision can still publish exactly
    // this confirmed draft, and an exact replay keeps that one receipt.
    let later=OperationId([0xd4;16]);let receipt=other.freeze(later,expected).unwrap();
    assert_eq!((receipt.ord,receipt.target.file),(1,file));
    assert_eq!(other.freeze(later,expected).unwrap(),receipt);
    std::fs::rename(&f,temp.0.join("offline-F")).unwrap();
    assert_eq!(other.manual_receipt(later,expected).unwrap(),Some(receipt));
    assert_eq!(other.manual_receipt(attempt,expected).unwrap(),None);
    let wrong=Target{revision:expected.revision+1,..expected};
    assert!(other.manual_receipt(later,wrong).is_err());
}
