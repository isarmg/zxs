//! Real SQLite file and Linux directory OFDs; no mock connection, legacy
//! REGISTRATION or product CLI. Never run on existing media.
use super::*;
use crate::{Role,fs::HeldDir,system::{ROOT_MARK,init_request},test_support::PrivateTemp};
use std::{ffi::c_void,fs::{File,OpenOptions},io::Write,os::unix::{ffi::OsStrExt,fs::OpenOptionsExt},ptr};

unsafe extern "C" {
    fn sqlite3_open_v2(path:*const c_char, db:*mut *mut Db, flags:c_int, vfs:*const c_char)->c_int;
    fn sqlite3_exec(db:*mut Db,sql:*const c_char,callback:Option<unsafe extern "C" fn(*mut c_void,c_int,*mut *mut c_char,*mut *mut c_char)->c_int>,context:*mut c_void,err:*mut *mut c_char)->c_int;
}
struct Builder(Connection);
impl Builder{
    fn new(path:&Path)->Self{
        let f=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(path).unwrap();
        f.sync_all().unwrap();File::open(path.parent().unwrap()).unwrap().sync_all().unwrap();drop(f);
        Self::existing(path)
    }
    fn existing(path:&Path)->Self{
        let p=CString::new(path.as_os_str().as_bytes()).unwrap();let mut db=ptr::null_mut();
        // Test fixture only. OS reservation already exists; no SQLite CREATE.
        check(unsafe{sqlite3_open_v2(p.as_ptr(),&mut db,2|0x01000000,c"unix".as_ptr())}).unwrap();Self(Connection(db))
    }
    fn try_exec(&self,sql:&str)->io::Result<()>{let q=CString::new(sql).unwrap();
        check(unsafe{sqlite3_exec(self.0.0,q.as_ptr(),None,ptr::null_mut(),ptr::null_mut())})}
    fn exec(&self,sql:&str){self.try_exec(sql).unwrap();}
}
fn hex(b:&[u8])->String{b.iter().map(|v|format!("{v:02x}")).collect()}
fn record(parent:&PrivateTemp,state:InitState,extra:&str)->(PathBuf,PoolId){
    assert_ne!(fs::local_uid(),0,"run this fixture only as the ordinary unprivileged storage principal");
    let s=parent.child("S");let dir=HeldDir::open(&s).unwrap();dir.exclusive().unwrap();
    let r=parent.0.join("unmounted-r");let f=parent.0.join(OsStr::from_bytes(b"unmounted-f-\xff"));
    let paths=[s.as_path(),r.as_path(),f.as_path()];let op=OperationId([1;16]);let pool=PoolId([4;16]);
    let uid=fs::local_uid();let actor=[2;16];let approve=[3;16];
    let sha=init_request(op,actor,approve,uid,&paths).unwrap();let n=ComponentLimit::held(&dir).unwrap().encoded();
    let b=Builder::new(&s.join("SYSTEM.sqlite3"));
    b.exec("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; BEGIN IMMEDIATE;");
    b.exec(SCHEMA);
    b.exec(&format!("INSERT INTO operation VALUES(X'{}',1,X'{}',X'{}',0); INSERT INTO pool_birth VALUES(1,X'{}',X'{}',X'{}',{uid},3);",
        hex(&op.0),hex(&actor),hex(&sha),hex(&pool.0),hex(&op.0),hex(&approve)));
    for(i,p)in paths.iter().enumerate(){
        b.exec(&format!("INSERT INTO volume_claim VALUES(X'{}',{i},{i},X'{}',X'{}',{n},X'{}');",
            hex(&op.0),hex(&[0x11+i as u8;16]),hex(p.as_os_str().as_bytes()),hex(&[0x21+i as u8;32])));
    }
    b.exec("COMMIT;");
    let mark=s.join(OsStr::from_bytes(ROOT_MARK.to_bytes()));
    let mut mf=OpenOptions::new().write(true).create_new(true).mode(0o600).open(mark).unwrap();
    mf.write_all(b"ZXS-SQLITE-1/VOLUME\0\x00").unwrap();
    mf.write_all(&pool.0).unwrap();mf.write_all(&[0x11;16]).unwrap();mf.write_all(&op.0).unwrap();
    mf.write_all(&[0x21;32]).unwrap();mf.sync_all().unwrap();drop(mf);dir.file.sync_all().unwrap();
    if state==InitState::Committed{b.exec("BEGIN IMMEDIATE; UPDATE operation SET state=1; COMMIT;");}
    if !extra.is_empty(){b.exec(extra);}
    drop(b);drop(dir);(s,pool)
}
#[test]
#[ignore="source-only qualification with non-root storage UID and dependable local Linux locks"]
fn actual_database_is_readonly_and_idle_online_ofd_excludes_a_separate_offline_holder(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let online=InitialReader::open(&s,p).unwrap();
    let rows=online.snapshot().unwrap();assert_eq!(rows.rows().iter().map(|x|x.role).collect::<Vec<_>>(),
        vec![Role::System,Role::Recycle,Role::File]);
    assert!(!rows.rows()[1].path.exists()&&!rows.rows()[2].path.exists()); // not opened to read S
    assert!(InitialReader::open(&s,PoolId([99;16])).is_err());
    assert_ne!(unsafe{sqlite3_get_autocommit(online.conn.0)},0);
    let separate=HeldDir::open(&s).unwrap();
    assert_eq!(separate.exclusive().unwrap_err().kind(),io::ErrorKind::WouldBlock);
    let displaced=t.0.join("displaced-S");std::fs::rename(&s,&displaced).unwrap();
    assert!(online.snapshot().is_err());assert!(separate.exclusive().is_err());
    std::fs::rename(&displaced,&s).unwrap();assert!(online.snapshot().is_ok());
    drop(online);separate.exclusive().unwrap();
}
#[test]
#[ignore="source-only SQLite qualification; never name an existing pool"]
fn pending_pool_extra_schema_and_unexpected_wal_are_not_accepted_or_repaired(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::PendingIo,"");
    assert!(InitialReader::open(&s,p).is_err());
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,
        "CREATE TABLE unexpected(value INTEGER) STRICT;");
    assert!(InitialReader::open(&s,p).is_err());
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let extra=s.join("SYSTEM.sqlite3-wal");std::fs::write(&extra,b"unowned").unwrap();
    assert!(InitialReader::open(&s,p).is_err());assert_eq!(std::fs::read(extra).unwrap(),b"unowned");
}

#[test]
#[ignore="synthetic isolated WAL-header cold-open qualification; no existing pool"]
fn readonly_wal_header_without_named_sidecars_is_rejected_without_creating_them(){
    use std::os::unix::fs::FileExt;
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let wal=s.join("SYSTEM.sqlite3-wal");let shm=s.join("SYSTEM.sqlite3-shm");let main=s.join("SYSTEM.sqlite3");
    assert!(!wal.exists()&&!shm.exists());
    // Closed local test DB only; no SQLite connection/statement exists in
    // this fixture. Synthesize the exact two WAL format versions used by the
    // pager, not a genuine WAL migration or an application repair operation.
    let raw=OpenOptions::new().write(true).open(&main).unwrap();raw.write_all_at(&[2,2],18).unwrap();
    raw.sync_all().unwrap();drop(raw);let original=std::fs::read(&main).unwrap();
    assert!(InitialReader::open(&s,p).is_err());assert!(!wal.exists()&&!shm.exists());
    assert_eq!(std::fs::read(main).unwrap(),original);
}

#[test]
#[ignore="source-only schema-change regression with non-root storage UID"]
fn current_database_is_not_authorized_by_an_old_schema_match(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let online=InitialReader::open(&s,p).unwrap();
    // Fault injected via another connection to the SAME SQLite engine, not
    // a raw file opener/closer. No maintenance permission is inferred.
    let other=Builder::existing(&s.join("SYSTEM.sqlite3"));
    other.exec("CREATE TABLE unforeseen(s INTEGER) STRICT;");
    assert!(online.snapshot().is_err());
    other.exec("DROP TABLE unforeseen;");
    assert_eq!(online.snapshot().unwrap().state(),InitState::Committed);
}

#[test]
#[ignore="actual ordinary non-root SQLite/media observation, not a health or disk test"]
fn just_the_declared_ordinary_root_is_observed_with_opaque_linux_path_and_without_recycle(){
    use std::os::unix::fs::DirBuilderExt;
    let t=PrivateTemp::under(&std::env::temp_dir());let(s,p)=record(&t,InitState::Committed,"");
    let online=InitialReader::open(&s,p).unwrap();
    let rows=online.snapshot().unwrap();let f=&rows.rows()[2];let r=&rows.rows()[1];
    assert!(f.path.to_str().is_none());assert!(!r.path.exists());
    assert!(online.ordinary_space(r.volume).is_err());assert!(online.ordinary_space(rows.rows()[0].volume).is_err());
    assert!(online.ordinary_space(VolumeId([0x55;16])).is_err());
    assert!(online.ordinary_space(f.volume).is_err()); // missing registered F is not empty/available
    std::fs::DirBuilder::new().mode(0o700).create(&f.path).unwrap();
    let name=f.path.join(OsStr::from_bytes(ROOT_MARK.to_bytes()));
    let mut mf=OpenOptions::new().write(true).mode(0o600).create_new(true).open(&name).unwrap();
    let mut bytes=rows.expected_marker(2).unwrap();bytes[69]^=1;mf.write_all(&bytes).unwrap();drop(mf);
    assert!(online.ordinary_space(f.volume).is_err());std::fs::write(&name,rows.expected_marker(2).unwrap()).unwrap();
    let observed=online.ordinary_space(f.volume).unwrap();
    assert_eq!(observed.volume,f.volume);assert!(observed.available_bytes<=observed.filesystem_bytes);
    assert!(!r.path.exists());
    let other=Builder::existing(&s.join("SYSTEM.sqlite3"));
    other.exec("CREATE TABLE unforeseen(x);");
    assert!(online.ordinary_space(f.volume).is_err()); // a new sample never reuses an earlier schema/row selection
    other.exec("DROP TABLE unforeseen;");
    assert_eq!(online.ordinary_space(f.volume).unwrap().volume,f.volume);
    let moved=t.0.join("displaced-F");
    let changed=online.ordinary_space_after(f.volume,||{
        std::fs::rename(&f.path,&moved)?;
        std::fs::DirBuilder::new().mode(0o700).create(&f.path)?;
        std::fs::copy(moved.join(OsStr::from_bytes(ROOT_MARK.to_bytes())),&name)?;
        Ok(())
    });
    assert!(changed.is_err()); // exact bytes in a new root do not revive the still-held root
    assert_eq!(std::fs::read(&name).unwrap(),rows.expected_marker(2).unwrap());
    assert_eq!(online.ordinary_space(f.volume).unwrap().volume,f.volume); // fresh claim/host path, not durable physical identity
}

#[path="sqlite_first_account.rs"]
mod first_account;

#[path="sqlite_publication.rs"]
mod publication;
