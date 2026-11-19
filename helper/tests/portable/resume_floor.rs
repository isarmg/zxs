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
    request:Vec<u8>, image:Vec<u8>, zero:Vec<u8>,
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
        let v=Self {_data:data,_recycle:recycle,names,origins,host_root:File::open("/").unwrap(),uid,request,image,zero};
        put(&v.at(0,PENDING),&v.image);
        v
    }
    fn at(&self,i:usize,n:&[u8])->PathBuf { self.names[i].join(OsStr::from_bytes(n)) }
    // Construct the expectation separately from the implementation under test.
    fn claim(&self, row:usize)->Vec<u8> {
        let mut out=b"ZXS-PORTABLE-1/VOLUME\0".to_vec();
        out.push(row.min(2) as u8);
        for id in [1,10+u8::try_from(row).unwrap(),2] { out.extend_from_slice(&[id;16]); }
        out.extend_from_slice(&Sha256::digest(&self.request));
        out
    }
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
    put(&a.at(3,ROOT_SLOT),&a.claim(3)); // individually correct, but no FLOOR and an impossible hole
    assert!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).is_err());
    assert_eq!(fs::read(a.at(3,ROOT_SLOT)).unwrap(),a.claim(3));
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

#[test]
fn cold_marker_steps_start_at_system_and_preserve_each_existing_claim_and_lock() {
    let a=Fixture::new(false);
    let choices=a.choices();
    let worker=WorkerPolicy::fixture();
    let reg=File::open(a.at(0,PENDING)).unwrap();
    let get_floor=||LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap().persist_zero().unwrap();
    let run=get_floor().markers();
    assert_eq!(run.synced_rows(),0);
    let first=run.sync_next().unwrap();
    assert_eq!(first.synced_rows(),1);
    let original_s=File::open(a.at(0,ROOT_SLOT)).unwrap();
    let stamp_s=Stamp::on(&original_s).unwrap();
    assert_eq!(fs::read(a.at(0,ROOT_SLOT)).unwrap(),a.claim(0));
    assert!((1..4).all(|i| !a.at(i,ROOT_SLOT).exists()));
    busy(&reg); for root in &a.origins { busy(root); }
    drop(first); // original PENDING, FLOOR and precisely the first real marker survive

    let mut cold=get_floor().markers();
    assert_eq!(cold.synced_rows(),0); // the visible S marker was not credited as synced
    for row in 0..4 {
        cold=cold.sync_next().unwrap();
        assert_eq!(cold.synced_rows(),row+1);
        for i in 0..=row { assert_eq!(fs::read(a.at(i,ROOT_SLOT)).unwrap(),a.claim(i)); }
        for i in row+1..4 { assert!(!a.at(i,ROOT_SLOT).exists()); }
        assert!(Stamp::on(&File::open(a.at(0,ROOT_SLOT)).unwrap()).unwrap()==stamp_s);
        busy(&reg); for fd in &a.origins { busy(fd); }
    }
    assert_eq!(fs::read(a.at(0,PENDING)).unwrap(),a.image);
    assert_eq!(fs::read(a.at(0,FLOOR)).unwrap(),a.zero);
    assert!(!a.at(0,b"REGISTRATION").exists());
    assert!(cold.sync_next().is_err()); // no fifth row; never performs READY publication
}

#[test]
fn out_of_order_or_foreign_feedback_blocks_the_first_marker_without_touching_it() {
    let a=Fixture::new(false);
    let choices=a.choices();
    let worker=WorkerPolicy::fixture();
    let run=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap().persist_zero().unwrap().markers();
    let wrong=a.at(3,ROOT_SLOT);
    put(&wrong,&a.claim(1)); // correct length and pool, but a later F carries R's claim
    let original=File::open(&wrong).unwrap();
    let before=Stamp::on(&original).unwrap();
    assert!(run.sync_next().is_err());
    assert_eq!(fs::read(&wrong).unwrap(),a.claim(1));
    assert!(Stamp::on(&File::open(&wrong).unwrap()).unwrap()==before);
    assert!((0..3).all(|i| !a.at(i,ROOT_SLOT).exists()));
    assert_eq!(fs::read(a.at(0,FLOOR)).unwrap(),a.zero);
}

#[test]
fn a_byte_identical_replacement_of_synced_system_claim_does_not_allow_the_recycler_step() {
    let a=Fixture::new(false);
    let choices=a.choices();
    let worker=WorkerPolicy::fixture();
    let run=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap()
        .persist_zero().unwrap().markers().sync_next().unwrap();
    let slot=a.at(0,ROOT_SLOT);
    let kept=File::open(&slot).unwrap();
    let moved=a._data.path().join("original-marker");
    fs::rename(&slot,&moved).unwrap();
    put(&slot,&a.claim(0));
    assert_ne!(Entity::of(&kept.metadata().unwrap()).unwrap(),
               Entity::of(&File::open(&slot).unwrap().metadata().unwrap()).unwrap());
    assert!(run.sync_next().is_err());
    assert_eq!(fs::read(&slot).unwrap(),a.claim(0));
    assert_eq!(fs::read(moved).unwrap(),a.claim(0));
    assert!((1..4).all(|i| !a.at(i,ROOT_SLOT).exists()));
}

#[test]
fn full_visible_prefix_with_one_foreign_operation_cannot_be_resumed_or_repaired() {
    let a=Fixture::new(false);
    let choices=a.choices(); let worker=WorkerPolicy::fixture();
    let mut run=LockedGroup::acquire(a.open(),a.order(&choices,&worker)).unwrap().persist_zero().unwrap().markers();
    for _ in 0..3 { run=run.sync_next().unwrap(); }
    drop(run);
    let mut foreign=a.claim(3);
    foreign[55]^=1; // same role, volume, pool, G and width, not the original acquisition Op
    let slot=a.at(3,ROOT_SLOT);
    put(&slot,&foreign);
    let fd=File::open(&slot).unwrap(); let stamp=Stamp::on(&fd).unwrap();
    assert!(LockedGroup::acquire(a.open(),a.order(&choices,&worker)).is_err());
    assert!(Stamp::on(&File::open(&slot).unwrap()).unwrap()==stamp);
    assert_eq!(fs::read(&slot).unwrap(),foreign);
    for i in 0..3 { assert_eq!(fs::read(a.at(i,ROOT_SLOT)).unwrap(),a.claim(i)); }
    assert!(!a.at(0,b"REGISTRATION").exists());
}
