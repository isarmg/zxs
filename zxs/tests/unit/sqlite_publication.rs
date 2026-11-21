//! Authenticated publication against one actual fresh SQLite database. No
//! network, mounted F/R, default ACL, media writer or routing fixture.
use super::*;
use crate::VolumeId;

pub(super) fn first(t:&PrivateTemp)->(PathBuf,PoolId){
    first_with_recycle(t,None)
}
pub(super) fn first_with_recycle(t:&PrivateTemp,recycle:Option<PathBuf>)->(PathBuf,PoolId){
    first_with_storage(t,recycle,None)
}
pub(super) fn first_with_storage(t:&PrivateTemp,recycle:Option<PathBuf>,second:Option<&Path>)->(PathBuf,PoolId){
    let (s,p)=record_with_storage(t,InitState::Committed,"",recycle,second);
    let selected=HeldDir::trusted_system(&s).unwrap();selected.exclusive().unwrap();
    let writer=Builder::existing(&s.join("SYSTEM.sqlite3"));
    let [pair,web,pending,activate]=first_account::rows();
    writer.exec("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; BEGIN IMMEDIATE;");
    writer.exec(&pair);writer.exec(&web);writer.exec(&pending);writer.exec("COMMIT");
    // Synthetic SQL evidence only: this fixture does not create or open a native SAM.
    writer.exec("BEGIN IMMEDIATE");writer.exec(&activate);writer.exec("COMMIT");
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
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t);
    let op=OperationId([0x81;16]);let f=VolumeId([0x13;16]);
    let original={
        let online=InitialReader::open(&s,p).unwrap();
        let cat=online.snapshot().unwrap();assert!(!cat.rows()[1].path.exists()&&!cat.rows()[2].path.exists());
        let m=online.authenticate_web("first.admin","correct horse battery staple").unwrap();
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
    let m=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    assert_eq!(m.publish_root(op,f,"报告").unwrap(),original);
    assert_eq!(share_count(&cold),3);assert!(!cold.snapshot().unwrap().rows()[2].path.exists());
}
#[test]
#[ignore="source-only two independent online sessions with real SQLite connections"]
fn writer_revalidates_schema_and_original_action_under_the_write_transaction(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t);
    let online=InitialReader::open(&s,p).unwrap();let peer=InitialReader::open(&s,p).unwrap();
    let m=online.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let n=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
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
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t);
    let marker=s.join(OsStr::from_bytes(ROOT_MARK.to_bytes()));
    let original=std::fs::read(&marker).unwrap();assert_eq!(original.len(),101);
    let op=OperationId([0x81;16]);let f=VolumeId([0x13;16]);
    {
        let online=InitialReader::open(&s,p).unwrap();
        let _m=online.authenticate_web("first.admin","correct horse battery staple").unwrap();
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
    let m=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    assert_eq!(m.publish_root(op,f,"报告").unwrap().volume,f);assert_eq!(share_count(&cold),1);
}

#[test]
#[ignore="source-only genuine SQLite/Argon2 Web manager; no SMB or mounted F needed"]
fn a_published_share_starts_disabled_and_has_exactly_one_independent_mode_history(){
    use crate::system::reader::account::share_mode::Mode;
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=first(&t);
    let a=InitialReader::open(&s,p).unwrap();let b=InitialReader::open(&s,p).unwrap();
    let admin=a.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let other=b.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let f=VolumeId([0x13;16]);let root=admin.publish_root(OperationId([0xa0;16]),f,"reports").unwrap();
    let alt=admin.publish_root(OperationId([0xa1;16]),f,"readonly").unwrap();
    let off=admin.share_setting(root.share).unwrap();
    assert_eq!((off.op,off.revision,off.mode),(root.op,0,Mode::Disabled));
    assert!(admin.set_share_mode(OperationId([0xa2;16]),root.share,root.op,Mode::Disabled).is_err());
    assert!(admin.set_share_mode(OperationId([0xa2;16]),root.share,alt.op,Mode::ReadWrite).is_err());
    let one=admin.set_share_mode(OperationId([0xa2;16]),root.share,root.op,Mode::ReadWrite).unwrap();
    assert_eq!(other.share_setting(root.share).unwrap(),one);
    assert_eq!(other.share_setting(alt.share).unwrap().mode,Mode::Disabled);
    assert!(other.set_share_mode(one.op,root.share,root.op,Mode::ReadOnly).is_err());
    assert!(other.set_share_mode(OperationId([0xa3;16]),root.share,root.op,Mode::ReadOnly).is_err());
    let two=other.set_share_mode(OperationId([0xa3;16]),root.share,one.op,Mode::ReadOnly).unwrap();
    assert_eq!((two.revision,two.mode),(2,Mode::ReadOnly));
    let three=admin.set_share_mode(OperationId([0xa4;16]),root.share,two.op,Mode::Disabled).unwrap();
    assert_eq!(three.mode,Mode::Disabled);
    drop(other);drop(admin);drop(b);drop(a);
    let cold=InitialReader::open(&s,p).unwrap();
    assert!(cold.authenticate_web("files.smb","correct horse battery staple").is_err());
    let fresh=cold.authenticate_web("first.admin","correct horse battery staple").unwrap();
    // Old response lost: return the earlier exact original without resurrecting
    // RW. Independent settings for a second definition of the same F remain off.
    assert_eq!(fresh.set_share_mode(one.op,root.share,root.op,Mode::ReadWrite).unwrap(),one);
    assert_eq!(fresh.share_setting(root.share).unwrap(),three);
    let reenabled=fresh.set_share_mode(OperationId([0xa5;16]),root.share,three.op,Mode::ReadWrite).unwrap();
    assert_ne!(reenabled.op,one.op);assert_eq!(reenabled.revision,4);
    assert_eq!(fresh.share_setting(alt.share).unwrap().revision,0);
    assert!(!cold.snapshot().unwrap().rows()[2].path.exists());
}
