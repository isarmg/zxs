//! Real SQLite file and Linux directory OFDs; no mock connection, legacy
//! REGISTRATION, F/R directory or product CLI. Never run on existing media.
use super::*;
use crate::{Role,fs::HeldDir,system::init_request,test_support::PrivateTemp};
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
        let p=CString::new(path.as_os_str().as_bytes()).unwrap();let mut db=ptr::null_mut();
        // Test fixture only. OS reservation already exists; no SQLite CREATE.
        check(unsafe{sqlite3_open_v2(p.as_ptr(),&mut db,2|0x01000000,c"unix".as_ptr())}).unwrap();Self(Connection(db))
    }
    fn exec(&self,sql:&str){let q=CString::new(sql).unwrap();
        check(unsafe{sqlite3_exec(self.0.0,q.as_ptr(),None,ptr::null_mut(),ptr::null_mut())}).unwrap();}
}
fn hex(b:&[u8])->String{b.iter().map(|v|format!("{v:02x}")).collect()}
fn record(parent:&PrivateTemp,state:InitState,extra:&str)->(PathBuf,PoolId){
    assert_ne!(fs::local_uid(),0,"run this fixture only as the ordinary unprivileged storage principal");
    let s=parent.child("S");let dir=HeldDir::open(&s).unwrap();dir.exclusive().unwrap();
    let r=parent.0.join("unmounted-r");let f=parent.0.join("unmounted-f");
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
