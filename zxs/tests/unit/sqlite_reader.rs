//! Real SQLite file and Linux directory OFDs; no mock connection, legacy
//! REGISTRATION or product CLI. Never run on existing media.
use super::*;
use crate::{Role,fs::HeldDir,system::{ROOT_MARK,init_request},test_support::PrivateTemp};
use std::{ffi::c_void,fs::{File,OpenOptions},io::Write,os::unix::{ffi::OsStrExt,fs::{DirBuilderExt,OpenOptionsExt}},ptr};

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
    record_with_recycle(parent,state,extra,None)
}
fn record_with_recycle(parent:&PrivateTemp,state:InitState,extra:&str,recycle:Option<PathBuf>)->(PathBuf,PoolId){
    record_with_storage(parent,state,extra,recycle,None)
}
fn record_with_storage(parent:&PrivateTemp,state:InitState,extra:&str,recycle:Option<PathBuf>,
    second:Option<&Path>)->(PathBuf,PoolId){
    assert_ne!(fs::local_uid(),0,"run this fixture only as the ordinary unprivileged storage principal");
    let s=parent.child("S");let dir=HeldDir::open(&s).unwrap();dir.exclusive().unwrap();
    let r=recycle.unwrap_or_else(||parent.0.join("unmounted-r"));let f=parent.0.join(OsStr::from_bytes(b"unmounted-f-\xff"));
    let mut ordinary=vec![f.as_path()];ordinary.extend(second);
    ordinary.sort_by(|a,b|a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
    let mut paths=vec![s.as_path(),r.as_path()];paths.extend(ordinary);
    let op=OperationId([1;16]);let pool=PoolId([4;16]);
    let uid=fs::local_uid();let actor=[2;16];let approve=[3;16];
    let sha=init_request(op,actor,approve,uid,&paths).unwrap();let n=ComponentLimit::held(&dir).unwrap().encoded();
    let b=Builder::new(&s.join("SYSTEM.sqlite3"));
    b.exec("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; BEGIN IMMEDIATE;");
    b.exec(SCHEMA);
    b.exec(&format!("INSERT INTO operation VALUES(X'{}',1,X'{}',X'{}',0); INSERT INTO pool_birth VALUES(1,X'{}',X'{}',X'{}',{uid},{});",
        hex(&op.0),hex(&actor),hex(&sha),hex(&pool.0),hex(&op.0),hex(&approve),paths.len()));
    for(i,p)in paths.iter().enumerate(){
        let role=i.min(2);
        b.exec(&format!("INSERT INTO volume_claim VALUES(X'{}',{i},{role},X'{}',X'{}',{n},X'{}');",
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
#[ignore="source-only explicit previous-version fixture; never open an existing pool"]
fn previous_native_version_is_not_rewritten_or_opened(){
    let t=PrivateTemp::under(&std::env::temp_dir());
    let (s,p)=record(&t,InitState::Committed,"PRAGMA main.user_version=16;");
    let original=s.join("SYSTEM.sqlite3");
    // Builder's SQLite connection has closed; this is only a byte snapshot
    // of the isolated fixture, not an extra checkpoint FD on a live pool.
    let before=std::fs::read(&original).unwrap();
    assert!(InitialReader::open(&s,p).is_err());
    assert_eq!(std::fs::read(&original).unwrap(),before);
    for name in ["SYSTEM.sqlite3-journal","SYSTEM.sqlite3-wal","SYSTEM.sqlite3-shm"]{
        assert!(!s.join(name).exists());
    }
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


fn mark_recycle(src:&InitialReader){
    let c=src.snapshot().unwrap();let r=&c.rows()[1].path;
    let p=r.join(OsStr::from_bytes(ROOT_MARK.to_bytes()));
    let mut stat=OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();
    stat.write_all(&c.expected_marker(1).unwrap()).unwrap();stat.sync_all().unwrap();drop(stat);
    File::open(r).unwrap().sync_all().unwrap();
}
#[test]
#[ignore="source-only uses separate ordinary /tmp and Linux /dev/shm test filesystems; no real volumes or SMB"]
fn only_the_typed_original_recycle_mount_may_back_retained_access(){
    use super::volume::{OrdinaryRoot,RecycleRoot};
    let t=PrivateTemp::under(Path::new("/tmp"));let ram=PrivateTemp::under(Path::new("/dev/shm"));
    let rp=ram.0.join("retained-original");
    let(s,p)=record_with_recycle(&t,InitState::Committed,"",Some(rp.clone()));
    let src=InitialReader::open(&s,p).unwrap();let c=src.snapshot().unwrap();
    let fp=create_file::open_root(&src);assert!(fp.is_dir());
    let ordinary=OrdinaryRoot::for_body(&src,&c,c.rows()[2].volume).unwrap();
    assert!(RecycleRoot::at(&src,&c).is_err());
    // An unavailable R never forces an ordinary F operation to access R.
    assert!(ordinary.space().is_ok());
    std::fs::DirBuilder::new().mode(0o700).create(&rp).unwrap();
    assert!(RecycleRoot::at(&src,&c).is_err()); // never regard mere parent/directory as an empty retained root
    mark_recycle(&src);
    let retained=RecycleRoot::at(&src,&c).unwrap();assert_eq!(retained.volume(),c.rows()[1].volume);
    assert!(retained.separate_from(&ordinary).is_ok());assert_eq!(retained.root_for_read().path,rp);
    let moved=ram.0.join("moved-recycle");std::fs::rename(&rp,&moved).unwrap();
    assert!(retained.require().is_err());assert!(ordinary.space().is_ok());

    let s=PrivateTemp::under(Path::new("/tmp"));let(home,p)=record(&s,InitState::Committed,"");
    let src=InitialReader::open(&home,p).unwrap();let c=src.snapshot().unwrap();
    std::fs::DirBuilder::new().mode(0o700).create(&c.rows()[1].path).unwrap();mark_recycle(&src);
    assert!(RecycleRoot::at(&src,&c).is_err()); // exact-role bytes on a sibling of S do not qualify as separate media
}

#[path="sqlite_first_account.rs"]
mod first_account;

#[path="sqlite_publication.rs"]
mod publication;

#[path="sqlite_create_file.rs"]
mod create_file;

#[path="sqlite_first_work.rs"]
mod first_work;

#[path="sqlite_work_cuts.rs"]
mod manual_cuts;

#[path="sqlite_recycle_location.rs"]
mod recycle_location;

/// Synthetic committed pool and fixed pair for the Web transport fixture.
pub(crate) fn web_pool(t:&PrivateTemp)->(PathBuf,PoolId){publication::first(t)}

pub(crate) fn ordinary_pool(t:&PrivateTemp)->(PathBuf,PoolId,PathBuf,crate::identity::ShareId,crate::identity::ShareId){
    let (s,p)=publication::first(t);let src=InitialReader::open(&s,p).unwrap();
    let f=create_file::open_root(&src);let(rw,ro)=create_file::configure(&src);(s,p,f,rw,ro)
}

pub(super) fn transfer_pool(t:&PrivateTemp,path:&Path)->(PathBuf,PoolId,PathBuf,crate::identity::ShareId){
    let(s,p)=publication::first_with_recycle(t,Some(path.to_owned()));
    let src=InitialReader::open(&s,p).unwrap();let f=create_file::open_root(&src);
    std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();mark_recycle(&src);
    let(rw,_)=create_file::configure(&src);(s,p,f,rw)
}

pub(super) fn ordinary_transfer_pool(t:&PrivateTemp,target:&Path)->
    (PathBuf,PoolId,PathBuf,PathBuf,crate::identity::ShareId,crate::identity::ShareId){
    use super::account::share_mode::Mode;
    let(s,p)=publication::first_with_storage(t,None,Some(target));
    let src=InitialReader::open(&s,p).unwrap();let c=src.snapshot().unwrap();
    let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let mut from=None;let mut to=None;
    for i in 2..4{
        let row=&c.rows()[i];assert_eq!(row.role,Role::File);
        std::fs::DirBuilder::new().mode(0o700).create(&row.path).unwrap();
        let mut mark=OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(row.path.join(OsStr::from_bytes(ROOT_MARK.to_bytes()))).unwrap();
        mark.write_all(&c.expected_marker(i).unwrap()).unwrap();mark.sync_all().unwrap();
        File::open(&row.path).unwrap().sync_all().unwrap();
        let is_target=row.path.as_path()==target;
        let share=web.publish_root(OperationId([0x71+i as u8;16]),row.volume,if is_target{"Destination"}else{"Source"}).unwrap();
        web.set_share_mode(OperationId([0x75+i as u8;16]),share.share,share.op,Mode::ReadWrite).unwrap();
        if is_target{to=Some(share.share);}else{from=Some((row.path.clone(),share.share));}
    }
    let (path,source)=from.unwrap();
    (s,p,path,c.rows()[1].path.clone(),source,to.unwrap())
}

pub(crate) fn stage_managed_file(source:&InitialReader,rw:crate::identity::ShareId,ro:crate::identity::ShareId,name:&str,data:&File)
    ->io::Result<crate::FileId>{
    // The fake native login proof remains inside this reader-only test.
    let session=smb::SmbSession::fixture_authenticated(source,"files.smb")?;
    let tree=session.connect(rw)?;let read=session.connect(ro)?;
    let file=tree.create_file(OperationId([0xbb;16]),name)?.file;
    let writer=tree.open_writer(name,file)?;
    let n=data.metadata()?.len();
    writer.save(OperationId([0xbc;16]),None,Some(0),n,data)?;
    drop(writer);assert_eq!(read.open_current(name)?.size(),n);
    Ok(file)
}
