//! Native fixtures require prepared private HELPER_TEST_PORTABLE_DATA_PARENT and
//! HELPER_TEST_PORTABLE_RECYCLE_PARENT directories and the restricted worker;
//! missing prerequisites are failures, not skips. Never format, mount, chown,
//! change process privileges, or modify pre-existing data. The name-limit check needs no fixture.
use super::*;
use sha2::{Digest,Sha256};
use std::{fs::{self,OpenOptions},io::Write,os::unix::fs::{OpenOptionsExt,PermissionsExt},path::PathBuf};
use tempfile::{Builder,TempDir};

struct Fixture {
    _data:TempDir, _recycle:TempDir,
    names:Vec<PathBuf>, origins:Vec<File>, host_root:File, uid:u32,
    request:Vec<u8>, image:Vec<u8>, zero:Vec<u8>, last_marker:Vec<u8>,
}
impl Fixture {
    fn new(same_fs:bool) -> Self {
        let parent=|key|PathBuf::from(std::env::var_os(key).expect("supply explicit private portable test mount parents"));
        let data=parent("HELPER_TEST_PORTABLE_DATA_PARENT");
        let recycle=if same_fs { data.clone() } else {parent("HELPER_TEST_PORTABLE_RECYCLE_PARENT")};
        let data=Builder::new().prefix("helper-portable-").tempdir_in(data).unwrap();
        let recycle=Builder::new().prefix("helper-recycle-").tempdir_in(recycle).unwrap();
        let names=vec![data.path().join("system"),recycle.path().join("recycle"),data.path().join("a"),data.path().join("z")];
        let origins:Vec<_>=names.iter().map(|p| {
            fs::create_dir(p).unwrap(); fs::set_permissions(p,fs::Permissions::from_mode(0o700)).unwrap();
            File::open(p).unwrap()
        }).collect();
        let uid=origins[0].metadata().unwrap().uid();
        assert_ne!(uid,0,"the chosen restricted non-root storage worker is required");
        let mut request=b"ZXS-PORTABLE-1/GENESIS\0".to_vec();
        for id in [1u8,2,3] { request.extend_from_slice(&[id;16]); }
        request.extend_from_slice(&uid.to_be_bytes()); request.extend_from_slice(&4u16.to_be_bytes());
        for (i,(p,f)) in names.iter().zip(&origins).enumerate() {
            request.push(i.min(2) as u8); request.extend_from_slice(&[i as u8+10;16]);
            request.push(helper_linux_evidence::portable_component_limit(f).unwrap());
            let raw=p.as_os_str().as_bytes(); request.extend_from_slice(&u16::try_from(raw.len()).unwrap().to_be_bytes());
            request.extend_from_slice(raw);
        }
        let digest:[u8;32]=Sha256::digest(&request).into();
        let mut image=request.clone(); image.extend_from_slice(&digest);
        let mut zero=b"ZXS-PORTABLE-1/CONFIRMED\0".to_vec();
        zero.extend_from_slice(&digest); zero.extend_from_slice(&[0;16]); zero.extend_from_slice(&digest);
        let mut last_marker=b"ZXS-PORTABLE-1/VOLUME\0".to_vec(); last_marker.push(2);
        for id in [1,13,2] { last_marker.extend_from_slice(&[id;16]); }
        last_marker.extend_from_slice(&digest);
        let v=Self {_data:data,_recycle:recycle,names,origins,host_root:File::open("/").unwrap(),uid,request,image,zero,last_marker};
        put(&v.at(0,PENDING),&v.image);
        v
    }
    fn at(&self,i:usize,n:&[u8])->PathBuf { self.names[i].join(OsStr::from_bytes(n)) }
    fn choices(&self)->Vec<(&Path,&File)> {
        self.names.iter().map(PathBuf::as_path).zip(&self.origins).collect()
    }
    fn open(&self)->HeldPending { HeldPending::open(&self.origins[0],&self.names[0],self.uid,&self.request).unwrap().0 }
    fn order<'a>(&'a self,origins:&'a [(&'a Path,&'a File)],worker:&'a WorkerPolicy)->FloorOrder<'a>{
        // Test models the otherwise unavailable, trusted per-operation handoff.
        FloorOrder { request:&self.request, origins, host_root:&self.host_root, worker }
    }
}
fn put(path:&Path,bytes:&[u8]) {
    let mut f=OpenOptions::new().create_new(true).write(true).mode(0o600).open(path).unwrap(); f.write_all(bytes).unwrap();
}
fn busy(f:&File) {
    assert_eq!(flock(f,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
}

#[test]
fn every_volume_must_fit_the_exact_mandatory_name_before_floor_creation() {
    assert_eq!(ROOT_SLOT.len(),43);
    assert_eq!(require_marker_space(42,42).unwrap_err().kind(),io::ErrorKind::Unsupported);
    assert!(require_marker_space(43,43).is_ok());
    assert!(require_marker_space(255,255).is_ok());
    assert_eq!(require_marker_space(43,42).unwrap_err().kind(),io::ErrorKind::InvalidData);
    assert_eq!(require_marker_space(42,43).unwrap_err().kind(),io::ErrorKind::InvalidData);
}

#[test]
fn actual_cold_request_creates_only_zero_floor_and_resyncs_the_same_file_on_retry() {
    let a=Fixture::new(false); let choices=a.choices(); let worker=WorkerPolicy::fixture();
    let reg=File::open(a.at(0,PENDING)).unwrap(); let original=Stamp::on(&reg).unwrap();
    let group=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap();
    busy(&reg); for fd in &a.origins { busy(fd); }
    let synced=group.persist_zero().unwrap(); synced.recheck().unwrap();
    assert_eq!(fs::read(a.at(0,FLOOR)).unwrap(),a.zero);
    busy(&reg); for fd in &a.origins { busy(fd); }
    assert!(original==Stamp::on(&reg).unwrap());
    assert!(a.names.iter().all(|d|!d.join(OsStr::from_bytes(ROOT_SLOT)).exists()));
    assert!(!a.at(0,b"REGISTRATION").exists());
    drop(synced);
    let floor=File::open(a.at(0,FLOOR)).unwrap();let first=Stamp::on(&floor).unwrap();
    let second=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap().persist_zero().unwrap();
    second.recheck().unwrap();
    assert!(first==Stamp::on(&File::open(a.at(0,FLOOR)).unwrap()).unwrap());
    assert_eq!(fs::read(a.at(0,PENDING)).unwrap(),a.image);
}

#[test]
fn unresolved_last_root_or_unclaimed_ancestor_prevents_the_first_floor_write() {
    let a=Fixture::new(false); let choices=a.choices(); let worker=WorkerPolicy::fixture();
    let mut wrong=choices.clone(); wrong[3].1=choices[2].1;
    assert!(LockedGroup::acquire(a.open(),a.order(&wrong,&worker)).is_err());
    let other=File::open(&a.names[3]).unwrap(); flock(&other,FlockOperation::NonBlockingLockShared).unwrap();
    assert_eq!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).err().unwrap().kind(),io::ErrorKind::WouldBlock);
    drop(other);
    put(&a.at(3,ROOT_SLOT),&a.last_marker); // individually correct, but no FLOOR and an impossible hole
    assert!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).is_err());
    assert_eq!(fs::read(a.at(3,ROOT_SLOT)).unwrap(),a.last_marker);
    fs::remove_file(a.at(3,ROOT_SLOT)).unwrap();
    let parent_slot=a._data.path().join(OsStr::from_bytes(ROOT_SLOT)); put(&parent_slot,b"unknown");
    assert!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).is_err());
    assert!(!a.at(0,FLOOR).exists()); assert_eq!(fs::read(parent_slot).unwrap(),b"unknown");
    assert_eq!(fs::read(a.at(0,PENDING)).unwrap(),a.image);
}

#[test]
fn distinct_recycler_directory_on_same_filesystem_and_preexisting_partial_floor_stay_untouched() {
    let a=Fixture::new(true); let choices=a.choices(); let worker=WorkerPolicy::fixture();
    assert_eq!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).err().unwrap().kind(),io::ErrorKind::Unsupported);
    assert!(!a.at(0,FLOOR).exists());
    put(&a.at(0,FLOOR),&a.zero[..33]);
    assert!(HeldPending::open(&a.origins[0],&a.names[0],a.uid,&a.request).is_err());
    assert_eq!(fs::read(a.at(0,FLOOR)).unwrap(),&a.zero[..33]);
}


#[test]
fn new_partial_confirmation_after_selection_cannot_be_overwritten_on_resume() {
    let a=Fixture::new(false); let choices=a.choices(); let worker=WorkerPolicy::fixture();
    let group=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap();
    put(&a.at(0,FLOOR),b"unknown existing bytes"); // simulate non-cooperation after the initial check
    let victim=File::open(a.at(0,FLOOR)).unwrap(); let before=Stamp::on(&victim).unwrap();
    assert!(group.persist_zero().is_err());
    assert!(Stamp::on(&victim).unwrap()==before);
    assert_eq!(fs::read(a.at(0,FLOOR)).unwrap(),b"unknown existing bytes");
    assert!(a.names.iter().all(|d|!d.join(OsStr::from_bytes(ROOT_SLOT)).exists()));
}
