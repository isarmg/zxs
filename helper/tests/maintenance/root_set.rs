//! Source-only future controls on newly-created private test directories.
use super::*;
use crate::maintenance::register_flock::UnpublishedRegisterFlock;
use crate::test_area::{data_directory, recycle_directory};
use rustix::fs::{FlockOperation, XattrFlags, fgetxattr, fremovexattr, fsetxattr};
use sha2::{Digest, Sha256};
use rustix::io::Errno;
use std::{ffi::OsString, fs, os::unix::fs::MetadataExt, path::{Path, PathBuf}};

fn own(parent: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let path = parent.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap(); path
}
fn selected(system: &Path, recycle: &Path, data: &[&Path]) -> Selection {
    let mut args = vec![OsString::from("--system"), system.into(), "--recycle".into(), recycle.into()];
    for p in data { args.push("--data".into()); args.push((*p).into()); }
    Selection::parse(args.into_iter()).unwrap()
}
fn private_uid(root: &Path) -> u32 { fs::metadata(root).unwrap().uid() }
fn available(root: &Path) {
    let f = File::open(root).unwrap(); flock(&f, NonBlockingLockExclusive).unwrap();
}
fn busy(root: &Path) {
    let f = File::open(root).unwrap();
    assert_eq!(flock(&f, FlockOperation::NonBlockingLockShared), Err(Errno::WOULDBLOCK));
}
fn positive(set: &Selection, uid: u32) -> FreshRootLocks {
    FreshRootLocks::try_lock(set, uid).expect("requires two private local test filesystems with different st_dev/UUID and complete kernel handles, xattrs, NOATIME and flock")
}

#[test]
fn distinct_pools_cannot_claim_one_original_volume_and_failures_release_all_other_locks() {
    let data = data_directory(); let bin = recycle_directory();
    let sys1 = own(data.path(), "s-one"); let sys2 = own(data.path(), "s-two");
    let shared = own(data.path(), "same-file-volume");
    let r1 = own(bin.path(), "r-one"); let r2 = own(bin.path(), "r-two");
    let a = selected(&sys1, &r1, &[&shared]); let b = selected(&sys2, &r2, &[&shared]);
    let uid = private_uid(&sys1);
    let first = positive(&a, uid);
    for p in [&sys1, &r1, &shared] { busy(p); }
    let err = FreshRootLocks::try_lock(&b, uid).err().expect("same real volume is busy");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    available(&sys2); available(&r2); busy(&shared);
    drop(first);
    let second = positive(&b, uid);
    for p in [&sys2, &r2, &shared] { busy(p); }
    drop(second);
    for p in [&sys1, &sys2, &r1, &r2, &shared] {
        available(p); assert_eq!(fs::read_dir(p).unwrap().count(), 0);
    }
}

#[test]
fn even_an_empty_existing_label_nonempty_root_reused_mount_or_changed_name_never_qualifies() {
    let data = data_directory(); let bin = recycle_directory();
    let system = own(data.path(), "system"); let primary = own(data.path(), "data");
    let also_data = own(data.path(), "not-recycle"); let recycle = own(bin.path(), "recycle");
    let selection = selected(&system, &recycle, &[&primary]);
    let uid = private_uid(&system);
    assert!(FreshRootLocks::try_lock(&selected(&system, &also_data, &[&primary]), uid).is_err());
    assert!(FreshRootLocks::try_lock(&selected(&system, &recycle, &[&system]), uid).is_err());
    let child = own(&primary, "child");
    assert!(FreshRootLocks::try_lock(&selected(&system, &recycle, &[&primary, &child]), uid).is_err());
    assert!(child.exists()); fs::remove_dir(child).unwrap();
    let dir = File::open(&primary).unwrap();
    for value in [&b""[..], b"not-ours"] {
        fsetxattr(&dir, c"user.zxs.volume", value, XattrFlags::CREATE).unwrap();
        assert!(FreshRootLocks::try_lock(&selection, uid).is_err());
        let mut observed = [0u8; 32];
        let n = fgetxattr(&dir, c"user.zxs.volume", &mut observed[..]).unwrap();
        assert_eq!(&observed[..n], value);
        for p in [&system, &recycle, &primary] { available(p); }
        fremovexattr(&dir, c"user.zxs.volume").unwrap(); // only our own fixture attribute
    }
    let existing = primary.join("unrelated");
    fs::write(&existing, b"must remain").unwrap();
    assert!(FreshRootLocks::try_lock(&selection, uid).is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"must remain");
    fs::remove_file(&existing).unwrap();
    let locked = positive(&selection, uid);
    let saved = data.path().join("old-data"); fs::rename(&primary, &saved).unwrap();
    own(data.path(), "data"); // distinct inode at the old spelling
    assert!(locked.check_empty_selection().is_err()); busy(&saved); available(&primary);
    drop(locked); available(&saved);
}

#[test]
fn full_set_finish_rejects_an_original_removed_after_all_streams_reached_eof() {
    let data = data_directory(); let bin = recycle_directory();
    let system = own(data.path(), "system"); let primary = own(data.path(), "data");
    let recycle = own(bin.path(), "recycle");
    let selection = selected(&system, &recycle, &[&primary]);
    let roots = positive(&selection, private_uid(&system));
    let samples = roots.sample_empty_roots().unwrap();
    roots.finish_empty_roots(&samples).unwrap(); // unchanged controls

    // Same held selection throughout, with the first sampled row removed AFTER EOF.
    fs::remove_dir(&system).unwrap();
    assert_eq!(roots.roots[0].dir.metadata().unwrap().nlink(), 0);
    assert!(roots.finish_empty_roots(&samples).is_err());
    own(data.path(), "system");
    assert_ne!(fs::metadata(&system).unwrap().ino(), roots.roots[0].dir.metadata().unwrap().ino());
    assert!(roots.finish_empty_roots(&samples).is_err());
    available(&system); busy(&primary); busy(&recycle);
    drop(roots); available(&primary); available(&recycle);
}

#[test]
fn claim_appearing_after_eof_on_the_same_original_cannot_be_counted_as_absent() {
    let data = data_directory(); let bin = recycle_directory();
    let system = own(data.path(), "system"); let primary = own(data.path(), "data");
    let recycle = own(bin.path(), "recycle");
    let selection = selected(&system, &recycle, &[&primary]);
    let roots = positive(&selection, private_uid(&system));
    let samples = roots.sample_empty_roots().unwrap();
    let same = File::open(&system).unwrap();
    fsetxattr(&same, c"user.zxs.volume", b"", XattrFlags::CREATE).unwrap();
    assert!(roots.finish_empty_roots(&samples).is_err());
    let mut buf = [0u8; 1];
    assert_eq!(fgetxattr(&same, c"user.zxs.volume", &mut buf[..]).unwrap(), 0);
    busy(&system); drop(roots); available(&system);
}

#[test]
fn an_unselected_ancestor_acquiring_even_an_empty_label_invalidates_the_real_group() {
    let data = data_directory(); let bin = recycle_directory();
    let system = own(data.path(), "system");
    let ancestor = own(data.path(), "unselected");
    let leaf = own(&ancestor, "candidate");
    let recycle = own(bin.path(), "recycle");
    let selected = selected(&system, &recycle, &[&leaf]);
    let uid = private_uid(&system);
    let group = positive(&selected, uid);
    let samples = group.sample_empty_roots().unwrap();
    group.finish_empty_roots(&samples).unwrap();

    let parent = File::open(&ancestor).unwrap();
    fsetxattr(&parent, c"user.zxs.volume", b"", XattrFlags::CREATE).unwrap();
    assert!(group.finish_empty_roots(&samples).is_err());
    busy(&leaf); drop(group);
    assert!(FreshRootLocks::try_lock(&selected, uid).is_err());
    let mut byte = [0u8; 1];
    assert_eq!(fgetxattr(&parent, c"user.zxs.volume", &mut byte[..]).unwrap(), 0);
    assert_eq!(std::fs::read_dir(&leaf).unwrap().count(), 0);
    available(&leaf); available(&system); available(&recycle);
}

// Encode only an in-memory proposal using independently held private test roots.
// No tag is issued and no bytes are written to REGISTRATION/INITIALIZING.
fn proposal(locked: &FreshRootLocks, allocation: &UnpublishedRegisterFlock<'_>, other_limit: bool) -> Vec<u8> {
    fn object(b: &mut Vec<u8>, original: &ObservedOriginal) {
        let (dev, ino, birth) = original.fields();
        b.extend_from_slice(&dev.to_be_bytes()); b.extend_from_slice(&ino.to_be_bytes());
        b.push(u8::try_from(birth.len()).unwrap()); b.extend_from_slice(birth);
    }
    let mut b = b"ZXS/FILE-POOL/MAINT-1\0".to_vec();
    b.extend_from_slice(&[0x61; 16]); b.extend_from_slice(&[0x62; 16]);
    b.extend_from_slice(&locked.uid.to_be_bytes());
    let (register, _) = allocation.fresh_origins().unwrap(); object(&mut b, &register);
    b.extend_from_slice(&(locked.roots.len() as u16).to_be_bytes());
    for (i, root) in locked.roots.iter().enumerate() {
        b.push(i.min(2) as u8); b.extend_from_slice(&[i as u8 + 1; 16]); object(&mut b, &root.observed);
        let raw = root.name.path().as_os_str().as_bytes();
        let native = root.component_limit;
        b.push(if other_limit && i == 0 { if native == 1 { 2 } else { native - 1 } } else { native });
        b.extend_from_slice(&(raw.len() as u16).to_be_bytes()); b.extend_from_slice(raw);
    }
    let sum = Sha256::digest(&b); b.extend_from_slice(&sum); b
}

#[test]
fn all_locked_roots_and_the_distinct_anonymous_register_check_the_same_in_memory_genesis() {
    let data = data_directory(); let recycle = recycle_directory();
    let system = own(data.path(), "system");
    let z = own(data.path(), "z-first-in-argv"); let a = own(data.path(), "a-first-on-wire");
    let selection = selected(&system, recycle.path(), &[&z, &a]);
    let uid = private_uid(&system);
    let roots = positive(&selection, uid);
    assert_eq!(roots.roots[2].name.path(), a); assert_eq!(roots.roots[3].name.path(), z);
    let register = UnpublishedRegisterFlock::create_in_held_system(&roots.roots[0].dir, uid).unwrap();
    let initial = proposal(&roots, &register, false);
    let decoded = FreshGenesis::decode(&initial).unwrap();
    register.check_image_origins(&decoded).unwrap();
    roots.check_image_roots(&decoded).unwrap();
    let changed = proposal(&roots, &register, true);
    let wrong = FreshGenesis::decode(&changed).unwrap();
    register.check_image_origins(&wrong).unwrap();
    assert!(roots.check_image_roots(&wrong).is_err(), "a valid SHA must not lend a different NAME_MAX");
    for r in &roots.roots { assert_eq!(fs::read_dir(r.name.path()).unwrap().count(), 0); }
}

#[test]
fn a_proposal_holds_the_same_empty_originals_and_never_adopts_a_pending_name() {
    let data = data_directory(); let bin = recycle_directory();
    let system = own(data.path(), "s"); let leaf = own(data.path(), "f"); let recycle = own(bin.path(), "r");
    let selection = selected(&system, &recycle, &[&leaf]);
    let mut group = positive(&selection, private_uid(&system));
    assert!(group.prepare_unpublished([0; 16]).is_err());
    let op = [0x91; 16];
    let proposal = group.prepare_unpublished(op).unwrap();
    proposal.recheck().unwrap();
    for path in [&system, &leaf, &recycle] {
        busy(path);
        assert_eq!(fs::read_dir(path).unwrap().count(), 0, "no registration or other entry was published");
        let f = File::open(path).unwrap(); let mut b = [0u8; 1];
        assert_eq!(fgetxattr(&f, c"user.zxs.volume", &mut b[..]), Err(Errno::NODATA));
    }
    drop(proposal); busy(&system); // leaving the proposal does not unlock the still-held group
    let foreign = system.join("INITIALIZING");
    fs::write(&foreign, b"test-owned existing content").unwrap();
    assert!(group.prepare_unpublished(op).is_err(), "fresh preparation is not resume");
    assert_eq!(fs::read(&foreign).unwrap(), b"test-owned existing content");
    drop(group); available(&system); available(&leaf); available(&recycle);
}
