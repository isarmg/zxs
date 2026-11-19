//! Authenticated publication against one actual fresh SQLite database. No
//! network, mounted F/R, default ACL, media writer or routing fixture.
use super::*;
use crate::VolumeId;

fn first(t:&PrivateTemp,profile:u8)->(PathBuf,PoolId){
    let (s,p)=record(t,InitState::Committed,"");
    let selected=HeldDir::trusted_system(&s).unwrap();selected.exclusive().unwrap();
    let writer=Builder::existing(&s.join("SYSTEM.sqlite3"));
    let(a,pw,receipt)=first_account::rows(profile);
    writer.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
    writer.exec(&a);writer.exec(&pw);writer.exec(&receipt);writer.exec("COMMIT");
    (s,p)
}
fn share_count(online:&InitialReader)->i64{
    online.at(|db,_|{let s=db.prepare(c"SELECT count(*) FROM main.root_share")?;
        assert_eq!(unsafe{sqlite3_step(s.0)},100);let n=s.integer(0)?;s.done()?;Ok(n)
    }).unwrap()
}
#[test]
#[ignore="source-only qualification using an unprivileged isolated SYSTEM and the real SQLite/Argon2 ABI"]
fn exact_original_publication_is_reusable_without_looking_at_unmounted_filesystems(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t,1);
    let op=OperationId([0x81;16]);let f=VolumeId([0x13;16]);
    let original={
        let online=InitialReader::open(&s,p).unwrap();
        let cat=online.snapshot().unwrap();assert!(!cat.rows()[1].path.exists()&&!cat.rows()[2].path.exists());
        let m=online.authenticate_first("first.admin","correct horse battery staple").unwrap();
        for wrong in [VolumeId([0x11;16]),VolumeId([0x12;16]),VolumeId([0x99;16])]{
            assert!(m.publish_root(op,wrong,"报告").is_err());
        }
        for bad in ["","a/b","wrong\nname"]{assert!(m.publish_root(op,f,bad).is_err());}
        assert!(m.publish_root(OperationId([0x44;16]),f,"报告").is_err()); // first-account operation, different kind
        assert_eq!(share_count(&online),0);
        let a=m.publish_root(op,f,"报告").unwrap();
        let recorded=online.at(|db,_|{
            let s=db.prepare(c"SELECT request_sha FROM main.operation WHERE op_id=?1 AND kind=3")?;
            s.bind_bytes(1,&op.0)?;assert_eq!(unsafe{sqlite3_step(s.0)},100);
            let sha=s.array::<32>(0)?;s.done()?;Ok(sha)
        }).unwrap();
        assert_eq!(hex(&recorded),"d2385e10d25e4681b83926ab28134d2c3fa82751845c480d6b4c4f5481016d06");
        assert_eq!(m.publish_root(op,f,"报告").unwrap(),a);
        assert!(m.publish_root(op,f,"Reports").is_err());
        assert!(m.publish_root(OperationId([0x82;16]),f,"报告").is_err()); // name never overwritten
        let upper=m.publish_root(OperationId([0x83;16]),f,"Reports").unwrap();
        let lower=m.publish_root(OperationId([0x84;16]),f,"reports").unwrap();
        assert_ne!(upper.share,lower.share);assert_eq!(share_count(&online),3);a
    };
    // Consumer lost the first response; all connections and the original
    // manager have dropped. A new authenticated reader recovers the same ID.
    let cold=InitialReader::open(&s,p).unwrap();
    let m=cold.authenticate_first("first.admin","correct horse battery staple").unwrap();
    assert_eq!(m.publish_root(op,f,"报告").unwrap(),original);
    assert_eq!(share_count(&cold),3);assert!(!cold.snapshot().unwrap().rows()[2].path.exists());
}
#[test]
#[ignore="source-only two independent online sessions with real SQLite connections"]
fn writer_revalidates_schema_and_original_action_under_the_write_transaction(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t,2);
    let online=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
    let m=online.authenticate_first("first.admin","correct horse battery staple").unwrap();
    let n=peer.authenticate_first("first.admin","correct horse battery staple").unwrap();
    let f=VolumeId([0x13;16]);let op=OperationId([0x85;16]);
    let writer=Builder::existing(&s.join("SYSTEM.sqlite3"));
    writer.exec("BEGIN IMMEDIATE");
    assert_eq!(m.publish_root(op,f,"A").unwrap_err().kind(),io::ErrorKind::WouldBlock);
    writer.exec("ROLLBACK");assert_eq!(share_count(&online),0);
    // A DB writer cannot commit a naked share row or stamp its receipt as
    // another account. This is a synthetic fixture, not an account issuer.
    writer.exec("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE");
    writer.exec(&format!("INSERT INTO root_share VALUES(X'{}',X'{}',X'{}',X'{}',X'41')",
        hex(&[0x91;16]),hex(&op.0),hex(&[0x44;16]),hex(&f.0)));
    assert!(writer.try_exec(&format!("INSERT INTO operation VALUES(X'{}',3,X'{}',zeroblob(32),1)",
        hex(&op.0),hex(&[0x92;16]))).is_err());
    assert!(writer.try_exec("COMMIT").is_err());writer.exec("ROLLBACK");assert_eq!(share_count(&online),0);
    assert!(m.publish_root_after_selection(op,f,"A",||{
        writer.exec("CREATE TABLE unexpected(x INTEGER) STRICT;");Ok(())
    }).is_err());
    writer.exec("DROP TABLE unexpected;");assert_eq!(share_count(&online),0);
    let mut competitor=None;
    let result=m.publish_root_after_selection(op,f,"A",||{
        competitor=Some(n.publish_root(op,f,"A")?);Ok(())
    }).unwrap();
    assert_eq!(Some(result),competitor);assert_eq!(share_count(&online),1);
}

#[test]
#[ignore="non-root isolated SQLite S and its own ordinary marker; no existing media"]
fn same_inode_system_marker_edit_after_both_inserts_rolls_back_share_and_receipt(){
    use std::{os::unix::fs::FileExt,time::{Duration,SystemTime}};
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t,2);
    let marker=s.join(OsStr::from_bytes(ROOT_MARK.to_bytes()));
    let original=std::fs::read(&marker).unwrap();assert_eq!(original.len(),101);
    let op=OperationId([0x81;16]);let f=VolumeId([0x13;16]);
    {
        let online=InitialReader::open(&s,p).unwrap();
        let _m=online.authenticate_first("first.admin","correct horse battery staple").unwrap();
        let w=Connection::open(&online.dir,true).unwrap();
        // Exercise the real publication transaction envelope at its last
        // boundary, without adding any pre-COMMIT callback to the product.
        let share=CString::new(format!("INSERT INTO root_share VALUES(X'{}',X'{}',X'{}',X'{}',X'e68aa5e5918a')",
            hex(&[0x91;16]),hex(&op.0),hex(&[0x44;16]),hex(&f.0))).unwrap();
        let receipt=CString::new(format!("INSERT INTO operation VALUES(X'{}',3,X'{}',X'd2385e10d25e4681b83926ab28134d2c3fa82751845c480d6b4c4f5481016d06',1)",
            hex(&op.0),hex(&[0x55;16]))).unwrap();
        let mut after_inserts=false;
        let got=online.transaction(&w,c"BEGIN IMMEDIATE",|db,_|{
            db.fixed(&share)?;db.fixed(&receipt)?;after_inserts=true;
            let fd=OpenOptions::new().write(true).open(&marker)?; // not the DB
            fd.write_all_at(&[original[100]^1],100)?;
            fd.set_modified(SystemTime::UNIX_EPOCH+Duration::from_secs(1))?;
            fd.sync_all()
        });
        assert!(after_inserts && got.is_err());assert!(online.snapshot().is_err());
    }
    // Restore only the synthetic marker, then positively inspect the same DB.
    let fd=OpenOptions::new().write(true).open(&marker).unwrap();
    fd.write_all_at(&original,0).unwrap();fd.sync_all().unwrap();drop(fd);
    let cold=InitialReader::open(&s,p).unwrap();
    assert_eq!(share_count(&cold),0);
    cold.at(|db,_|{let miss=db.prepare(c"SELECT 1 FROM operation WHERE op_id=?1")?;
        miss.bind_bytes(1,&op.0)?;miss.done()
    }).unwrap();
    let m=cold.authenticate_first("first.admin","correct horse battery staple").unwrap();
    assert_eq!(m.publish_root(op,f,"报告").unwrap().volume,f);assert_eq!(share_count(&cold),1);
}
