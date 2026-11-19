//! Native source controls only. Use purpose-created files in the private test area.
use super::*;
use crate::test_area::data_directory;
use rustix::fs::{AtFlags, FlockOperation::{NonBlockingLockExclusive as EX, NonBlockingLockShared as SH}, linkat};
use rustix::io::Errno;
use std::{fs::{self, OpenOptions}, os::unix::fs::{OpenOptionsExt, PermissionsExt}};

fn expect_busy(file: &File, op: FlockOperation) {
    assert_eq!(flock(file, op), Err(Errno::WOULDBLOCK), "only an actual competing flock counts");
}

#[test]
fn original_exclusion_survives_a_first_name_rename_and_dup_until_the_last_ofd_closes() {
    let tmp = data_directory();
    let root = File::open(tmp.path()).unwrap();
    let uid = root.metadata().unwrap().uid();
    let reservation = UnpublishedRegisterFlock::create_in_held_system(&root, uid)
        .expect("requires an allowed local mount, exact 0600 creation and O_TMPFILE support");
    let original = Entity::of(&reservation.held.metadata().unwrap()).unwrap();
    assert_eq!(reservation.held.metadata().unwrap().nlink(), 0);
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    // Fixture-only first naming. An unsupported/unauthorized AT_EMPTY_PATH fails
    // this case; it is not an excuse to use a different object or claim success.
    linkat(&reservation.held, c"", &root, c"first", AtFlags::EMPTY_PATH)
        .expect("isolated test needs permission to link its own anonymous inode by descriptor");
    let independent = File::open(tmp.path().join("first")).unwrap();
    assert_eq!(Entity::of(&independent.metadata().unwrap()).unwrap(), original);
    expect_busy(&independent, SH);
    fs::rename(tmp.path().join("first"), tmp.path().join("second")).unwrap();
    let renamed = File::open(tmp.path().join("second")).unwrap();
    assert_eq!(Entity::of(&renamed.metadata().unwrap()).unwrap(), original);
    expect_busy(&renamed, EX);
    // Neither locking the containing directory nor a replacement at the old
    // spelling interferes with this register's original lock.
    let replacement = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .open(tmp.path().join("first")).unwrap();
    assert_ne!(Entity::of(&replacement.metadata().unwrap()).unwrap(), original);
    flock(&replacement, EX).unwrap();
    flock(&root, EX).unwrap();
    expect_busy(&renamed, SH);
    let duplicate = reservation.held.try_clone().unwrap(); // same open description, NOT a rival
    drop(reservation);
    expect_busy(&independent, SH);
    drop(duplicate);
    flock(&independent, SH).unwrap();
    flock(&renamed, SH).unwrap();
    let rival = File::open(tmp.path().join("second")).unwrap();
    expect_busy(&rival, EX);
    drop(independent);
    expect_busy(&rival, EX);
    drop(renamed);
    flock(&rival, EX).unwrap();
}

#[test]
fn two_new_anonymous_inodes_are_not_a_pool_wide_claim() {
    let tmp = data_directory();
    let root = File::open(tmp.path()).unwrap();
    let uid = root.metadata().unwrap().uid();
    let a = UnpublishedRegisterFlock::create_in_held_system(&root, uid).unwrap();
    let b = UnpublishedRegisterFlock::create_in_held_system(&root, uid).unwrap();
    assert_ne!(Entity::of(&a.held.metadata().unwrap()).unwrap(), Entity::of(&b.held.metadata().unwrap()).unwrap());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn rejects_wrong_owner_or_directory_mode_and_an_unlinked_ordinary_file_is_not_tmpfile() {
    let tmp = data_directory();
    let root = File::open(tmp.path()).unwrap();
    let uid = root.metadata().unwrap().uid();
    assert!(UnpublishedRegisterFlock::create_in_held_system(&root, uid.wrapping_add(1)).is_err());
    let spelling = tmp.path().join("ours");
    let n = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&spelling).unwrap();
    fs::remove_file(&spelling).unwrap();
    assert_eq!(n.metadata().unwrap().nlink(), 0);
    assert!(check_new(&n, &check_root(&root, uid).unwrap(), uid).is_err());
    assert!(UnpublishedRegisterFlock::create_in_held_system(&n, uid).is_err());
    root.set_permissions(fs::Permissions::from_mode(0o720)).unwrap();
    assert!(UnpublishedRegisterFlock::create_in_held_system(&root, uid).is_err());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

/// This is a RAM-only image for a TWO-origin comparison, not a registered pool.
/// Recycle/FILE rows are explicitly hypothetical; no path or volume is opened from it.
fn image_for(uid: u32, register: (u64, u64, &[u8]), system: (u64, u64, &[u8])) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    fn put(b: &mut Vec<u8>, (dev, ino, birth): (u64, u64, &[u8])) {
        b.extend_from_slice(&dev.to_be_bytes()); b.extend_from_slice(&ino.to_be_bytes());
        b.push(u8::try_from(birth.len()).unwrap()); b.extend_from_slice(birth);
    }
    fn row(b: &mut Vec<u8>, tag: u8, id: u8, obj: (u64, u64, &[u8]), path: &[u8]) {
        b.push(tag); b.extend_from_slice(&[id; 16]); put(b, obj);
        b.push(255); b.extend_from_slice(&u16::try_from(path.len()).unwrap().to_be_bytes());
        b.extend_from_slice(path);
    }
    let mut b = b"ZXS/FILE-POOL/MAINT-1\0".to_vec();
    b.extend_from_slice(&[0xB1; 16]); b.extend_from_slice(&[0xB2; 16]);
    b.extend_from_slice(&uid.to_be_bytes()); put(&mut b, register); b.extend_from_slice(&3u16.to_be_bytes());
    row(&mut b, 0, 1, system, b"/fixture/system");
    let mut unobserved = [3u8, 4, 5].into_iter().filter(|n| [*n; 16] != system.2[..16]);
    for (tag, path) in [(1u8, &b"/fixture/recycle"[..]), (2u8, &b"/fixture/data"[..])] {
        let mut birth = [0u8; 21]; birth[..16].fill(unobserved.next().unwrap());
        row(&mut b, tag, tag + 1, (system.0 ^ u64::from(tag), 1, &birth), path);
    }
    let digest = Sha256::digest(&b); b.extend_from_slice(&digest); b
}

#[test]
fn checked_image_needs_the_exact_held_system_and_its_unnamed_original_including_birth() {
    let tmp = data_directory();
    let system = File::open(tmp.path()).unwrap();
    let uid = system.metadata().unwrap().uid();
    let a = UnpublishedRegisterFlock::create_in_held_system(&system, uid).unwrap();
    let b = UnpublishedRegisterFlock::create_in_held_system(&system, uid).unwrap();
    let (r, s) = a.fresh_origins().expect("requires actual FS_IOC_GETFSUUID and a complete native file handle");
    let (other, _) = b.fresh_origins().unwrap();
    assert!(r.birth != other.birth, "distinct original inodes need distinct complete births");
    assert!(r.birth.filesystem_uuid() == s.birth.filesystem_uuid(), "new registration must be local to its SYSTEM");
    let wire = image_for(uid, r.fields(), s.fields());
    let g = FreshGenesis::decode(&wire).unwrap();
    a.check_image_origins(&g).unwrap();
    assert!(b.check_image_origins(&g).is_err());
    for piece in [
        image_for(uid ^ 1, r.fields(), s.fields()),
        image_for(uid, (r.at.entity.dev, r.at.entity.ino, other.birth.payload()), s.fields()),
        image_for(uid, r.fields(), (s.at.entity.dev, s.at.entity.ino, other.birth.payload())),
    ] {
        let changed = FreshGenesis::decode(&piece).unwrap(); // a valid hash is not proof
        assert!(a.check_image_origins(&changed).is_err());
    }
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0, "no first name or other claimed volume");
}

#[test]
fn an_o_path_directory_is_not_the_real_fd_required_by_the_birth_ioctl() {
    let tmp = data_directory();
    let path = File::from(rustix::fs::open(tmp.path(), OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                                           Mode::empty()).unwrap());
    let err = read_birth(&path, mount_id(&path).unwrap()).err().expect("O_PATH must not be promoted");
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
}
