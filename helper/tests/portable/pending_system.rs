//! Uses only freshly created ordinary files. No external mounts, disk discovery,
//! xattrs or protocol-specific filesystem features. Never opens the public vector's paths.
use super::*;
use sha2::{Digest, Sha256};
use std::{fs::{self, OpenOptions}, io::Write, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::PathBuf};
use tempfile::TempDir;

fn vector(name:&str) -> Vec<u8> {
    let line=include_str!("genesis-v1.txt").lines().find_map(|s| s.split_once('=').filter(|(k,_)| *k==name))
        .expect("missing public vector");
    line.1.as_bytes().chunks_exact(2).map(|p|u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()
}

struct Fixture {
    area:TempDir, path:PathBuf, host:File, uid:u32,
    request:Vec<u8>, reg:Vec<u8>, mark:Vec<u8>, floor:Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let area=tempfile::tempdir().unwrap();
        let path=area.path().join("system"); fs::create_dir(&path).unwrap();
        fs::set_permissions(&path,fs::Permissions::from_mode(0o700)).unwrap();
        let host=File::open(&path).unwrap();
        let uid=host.metadata().unwrap().uid();
        assert_ne!(uid,0,"requires an ordinary non-root test account; not a validation pass when absent");
        let limit=helper_linux_evidence::portable_component_limit(&host).unwrap();
        // Independent chosen request. Neither R nor F entries exist. Successful
        // SYSTEM observation cannot have obtained or certified any of those roots.
        let original=vector("body");
        let mut request=original[..77].to_vec(); request[71..75].copy_from_slice(&uid.to_be_bytes());
        let paths=[path.clone(),area.path().join("unmounted-recycler"),area.path().join("absent-a"),area.path().join("absent-b")];
        for (at,p) in [77usize,103,129,156].into_iter().zip(&paths) {
            request.extend_from_slice(&original[at..at+17]); request.push(limit);
            let raw=p.as_os_str().as_bytes(); request.extend_from_slice(&u16::try_from(raw.len()).unwrap().to_be_bytes());
            request.extend_from_slice(raw);
        }
        let digest: [u8;32]=Sha256::digest(&request).into();
        let mut reg=request.clone(); reg.extend_from_slice(&digest);
        let mut mark=vector("volume_s"); mark[71..].copy_from_slice(&digest);
        let mut floor=vector("zero_floor"); floor[25..57].copy_from_slice(&digest); floor[73..].copy_from_slice(&digest);
        let f=Self {area,path,host,uid,request,reg,mark,floor};
        f.put(PENDING,&f.reg);
        f
    }
    fn entry(&self,name:&[u8])->PathBuf { self.path.join(OsStr::from_bytes(name)) }
    fn put(&self,name:&[u8],bytes:&[u8]) {
        let mut file=OpenOptions::new().write(true).create_new(true).mode(0o600).open(self.entry(name)).unwrap();
        file.write_all(bytes).unwrap();
    }
    fn observe(&self)->io::Result<(HeldPending,SystemPhase)> {
        HeldPending::open(&self.host,&self.path,self.uid,&self.request)
    }
}

#[test]
fn three_real_pending_phases_do_not_publish_or_follow_an_absent_volume() {
    let a=Fixture::new();
    let reg=File::open(a.entry(PENDING)).unwrap(); let before=Stamp::on(&reg).unwrap();
    assert_eq!(a.observe().unwrap().1,SystemPhase::NoFloor);
    a.put(ROOT_SLOT,&a.mark);
    assert!(a.observe().is_err()); // absence of CONFIRMED is not zero
    fs::remove_file(a.entry(ROOT_SLOT)).unwrap();
    a.put(FLOOR,&a.floor);
    assert_eq!(a.observe().unwrap().1,SystemPhase::ZeroFloor);
    a.put(ROOT_SLOT,&a.mark);
    let (held,stage)=a.observe().unwrap();
    assert_eq!(stage,SystemPhase::ZeroFloorAndSystemMarker);
    let mut changed=a.request.clone(); changed[55]^=1; // a different, non-secret approval reference
    assert!(held.recheck(&changed).is_err());
    assert_eq!(held.recheck(&a.request).unwrap(),stage);
    assert!(before==Stamp::on(&reg).unwrap());
    assert!(!a.entry(b"REGISTRATION").exists());
    assert!(!a.area.path().join("unmounted-recycler").exists());
    assert_eq!(fs::read(a.entry(PENDING)).unwrap(),a.reg);
}

#[test]
fn independent_online_registration_and_directory_writers_block_only_their_actual_cuts() {
    let a=Fixture::new();
    let old=File::open(a.entry(PENDING)).unwrap();
    flock(&old,FlockOperation::NonBlockingLockShared).unwrap();
    assert_eq!(a.observe().err().unwrap().kind(),io::ErrorKind::WouldBlock);
    drop(old);

    let (held,_)=a.observe().unwrap();
    let reader=File::open(a.entry(PENDING)).unwrap();
    assert_eq!(flock(&reader,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    // The short lock was released, not the retained registration EX.
    let writer=File::open(&a.path).unwrap();
    flock(&writer,FlockOperation::NonBlockingLockExclusive).unwrap();
    assert_eq!(held.recheck(&a.request).unwrap_err().kind(),io::ErrorKind::WouldBlock);
    drop(writer);
    assert_eq!(held.recheck(&a.request).unwrap(),SystemPhase::NoFloor);
    drop(held);
    flock(&reader,FlockOperation::NonBlockingLockShared).unwrap();
}

#[test]
fn byte_identical_replacements_do_not_inherit_held_evidence_or_registration_lock() {
    let a=Fixture::new(); a.put(FLOOR,&a.floor); a.put(ROOT_SLOT,&a.mark);
    let (held,_)=a.observe().unwrap();
    fs::rename(a.entry(FLOOR),a.area.path().join("saved-floor")).unwrap(); a.put(FLOOR,&a.floor);
    assert!(held.recheck(&a.request).is_err());
    drop(held);
    let (held,_)=a.observe().unwrap(); // a fresh byte observation; not proof of historical identity
    let saved=a.area.path().join("saved-register");
    fs::rename(a.entry(PENDING),&saved).unwrap(); a.put(PENDING,&a.reg);
    assert!(held.recheck(&a.request).is_err());
    let old=File::open(saved).unwrap();
    assert_eq!(flock(&old,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    drop(held); flock(&old,FlockOperation::NonBlockingLockShared).unwrap();
}

#[test]
fn missing_changed_or_untrusted_controls_are_not_repaired_or_treated_as_an_empty_pool() {
    let a=Fixture::new();
    let other=File::open(a.area.path()).unwrap();
    assert!(HeldPending::open(&other,&a.path,a.uid,&a.request).is_err());

    // Search-only: a normal directory open of this unrelated path would be
    // denied. It must first be rejected by comparing just the O_PATH identity.
    let unrelated=a.area.path().join("search-only");
    fs::create_dir(&unrelated).unwrap();
    fs::set_permissions(&unrelated,fs::Permissions::from_mode(0o100)).unwrap();
    let denied=HeldPending::open(&a.host,&unrelated,a.uid,&a.request).err().unwrap();
    assert_eq!(denied.kind(),io::ErrorKind::InvalidData);
    assert_eq!(denied.to_string(),"selected SYSTEM differs from supplied host origin");
    fs::set_permissions(&unrelated,fs::Permissions::from_mode(0o700)).unwrap();

    // This also works on case-sensitive filesystems: raw inventory rejects the
    // misspelling BEFORE lookup (NotFound there, or lock conflict on a folding FS).
    let saved=a.area.path().join("saved-original");
    fs::rename(a.entry(PENDING),&saved).unwrap();
    a.put(b"initializing",&a.reg);
    let lookalike=File::open(a.entry(b"initializing")).unwrap();
    flock(&lookalike,FlockOperation::NonBlockingLockExclusive).unwrap();
    let denied=a.observe().err().unwrap();
    assert_eq!(denied.kind(),io::ErrorKind::InvalidData);
    assert_eq!(denied.to_string(),"foreign entry in the initial-registration SYSTEM");
    drop(lookalike);
    fs::remove_file(a.entry(b"initializing")).unwrap();
    fs::rename(saved,a.entry(PENDING)).unwrap();

    a.put(b"REGISTRATION",&a.reg); assert!(a.observe().is_err());
    assert_eq!(fs::read(a.entry(b"REGISTRATION")).unwrap(),a.reg);
    fs::remove_file(a.entry(b"REGISTRATION")).unwrap();
    let mut foreign=a.floor.clone(); foreign[56]^=1; a.put(FLOOR,&foreign);
    assert!(a.observe().is_err()); assert_eq!(fs::read(a.entry(FLOOR)).unwrap(),foreign);
    fs::remove_file(a.entry(FLOOR)).unwrap();
    std::os::unix::fs::symlink(a.entry(PENDING),a.entry(FLOOR)).unwrap();
    assert!(a.observe().is_err());
    fs::remove_file(a.entry(FLOOR)).unwrap();
    let mut appended=OpenOptions::new().append(true).open(a.entry(PENDING)).unwrap();
    appended.write_all(&[0xde,0xad]).unwrap(); drop(appended);
    assert_eq!(a.observe().err().unwrap().kind(),io::ErrorKind::Unsupported);
    assert_eq!(fs::read(a.entry(PENDING)).unwrap(),[a.reg.as_slice(),&[0xde,0xad]].concat());
}

#[test]
fn cold_published_observation_holds_the_same_original_without_touching_other_roots() {
    let a=Fixture::new();
    a.put(FLOOR,&a.floor); a.put(ROOT_SLOT,&a.mark);
    fs::rename(a.entry(PENDING),a.entry(READY)).unwrap();
    let fd=File::open(a.entry(READY)).unwrap();
    let reg=Stamp::on(&fd).unwrap(); let parent=Stamp::on(&a.host).unwrap();
    assert!(a.observe().is_err()); // never recover a published register through PENDING
    let held=HeldReady::observe(&a.host,&a.path,a.uid,&a.request).unwrap();
    held.check_system(&a.request).unwrap();
    assert_eq!(flock(&fd,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    assert!(reg==Stamp::on(&fd).unwrap() && parent==Stamp::on(&a.host).unwrap());
    assert!(!a.area.path().join("unmounted-recycler").exists());
    let mut wrong=a.request.clone(); wrong[55]^=12;
    assert!(held.check_system(&wrong).is_err());
    drop(held);
    flock(&fd,FlockOperation::NonBlockingLockShared).unwrap();
}

#[test]
fn missing_or_dual_names_and_small_or_large_published_tails_are_not_empty_ready() {
    let a=Fixture::new();
    let observe=||HeldReady::observe(&a.host,&a.path,a.uid,&a.request);
    fs::rename(a.entry(PENDING),a.entry(READY)).unwrap();
    a.put(FLOOR,&a.floor); assert!(observe().is_err());
    a.put(ROOT_SLOT,&a.mark); assert!(observe().is_ok());
    a.put(PENDING,&a.reg); assert!(observe().is_err());
    fs::remove_file(a.entry(PENDING)).unwrap();
    let fd=OpenOptions::new().read(true).write(true).open(a.entry(READY)).unwrap();
    for at in [a.reg.len() as u64,MAX_GENESIS as u64] {
        fd.write_all_at(&[0xa5],at).unwrap();
        let before=Stamp::on(&fd).unwrap();
        assert_eq!(observe().err().unwrap().kind(),io::ErrorKind::Unsupported);
        assert!(before==Stamp::on(&fd).unwrap());
        let mut byte=[0]; fd.read_exact_at(&mut byte,at).unwrap(); assert_eq!(byte,[0xa5]);
    }
    assert!(!a.entry(PENDING).exists());
}
