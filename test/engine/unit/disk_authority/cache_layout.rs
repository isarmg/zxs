use super::*;
use crate::disk_authority::rebuild_projection;
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

const MARKER: &[u8] = br#"{"format":"independent-disk-projection"}"#;

fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[derive(Clone, Copy)]
enum UnsafeEntry {
    Symlink,
    HardLink,
    PublicMode,
}

fn unsafe_entry(path: &Path, outside: &Path, bytes: &[u8], kind: UnsafeEntry) {
    private_file(outside, bytes);
    match kind {
        UnsafeEntry::Symlink => symlink(outside, path).unwrap(),
        UnsafeEntry::HardLink => fs::hard_link(outside, path).unwrap(),
        UnsafeEntry::PublicMode => {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
}

fn filesystem_rejected<T>(result: Result<T>) {
    assert!(matches!(result, Err(Error::Filesystem(_))));
}

#[test]
fn projection_rebuild_keeps_file_names_bytes_and_private_modes() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("projection");
    assert!(rebuild_projection(&path, &[]).unwrap().is_empty());
    let mut entries: Vec<_> = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, ["cache.db", "cache.lock", "projection-kind.json"]);
    assert_eq!(fs::read(path.join("projection-kind.json")).unwrap(), MARKER);
    assert_eq!(fs::read(path.join("cache.lock")).unwrap(), b"");
    let cache = fs::read(path.join("cache.db")).unwrap();
    assert_eq!(&cache[..16], b"SQLite format 3\0");
    for name in ["cache.db", "cache.lock", "projection-kind.json"] {
        assert_eq!(fs::metadata(path.join(name)).unwrap().mode() & 0o777, 0o600);
    }
    fs::remove_file(path.join("cache.db")).unwrap();
    assert!(rebuild_projection(&path, &[]).unwrap().is_empty());
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), cache);
    assert_eq!(fs::read(path.join("projection-kind.json")).unwrap(), MARKER);
}

#[test]
fn cache_publication_preserves_staged_bytes_atomic_replacement_and_lock_lifetime() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projection");
    let directory = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("cache.db"), b"previous cache bytes");
    let mut previous = File::open(path.join("cache.db")).unwrap();
    let prepared = prepare(directory, &[]).unwrap();
    let staging = path.join(prepared.staging.as_path());
    let bytes = prepared.bytes.clone();
    assert_eq!(fs::read(&staging).unwrap(), bytes);
    assert_eq!(fs::metadata(&staging).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), b"previous cache bytes");
    let observer = PrivateDirectory::open_existing(&path).unwrap();
    filesystem_rejected(prepare(PrivateDirectory::open_existing(&path).unwrap(), &[]));

    let published = prepared.publish().unwrap();
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), bytes);
    assert!(!staging.exists());
    assert!(published.read().unwrap().is_empty());
    assert_ne!(
        previous.metadata().unwrap().ino(),
        fs::metadata(path.join("cache.db")).unwrap().ino(),
    );
    let mut held_bytes = Vec::new();
    previous.read_to_end(&mut held_bytes).unwrap();
    assert_eq!(held_bytes, b"previous cache bytes");
    assert!(
        AdvisoryLock::acquire(&observer, &layout::PROJECTION_CACHE_LOCK.as_relative()).is_err()
    );
    drop(published);
    let _lock =
        AdvisoryLock::acquire(&observer, &layout::PROJECTION_CACHE_LOCK.as_relative()).unwrap();
    assert_eq!(fs::read(path.join("cache.lock")).unwrap(), b"");
}

#[test]
fn cache_staging_names_keep_decimal_process_and_sequence_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("projection")).unwrap();
    for (process, sequence, expected) in [
        (0, 0, "cache-build-0-0.db"),
        (17, 42, "cache-build-17-42.db"),
        (u32::MAX, u64::MAX, "cache-build-4294967295-18446744073709551615.db"),
    ] {
        let name = layout::projection_cache_staging(process, sequence);
        assert_eq!(name.as_path(), Path::new(expected));
        let file = directory.open_payload(&name, PayloadOpenMode::CreateNew).unwrap();
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(directory.open_payload(&name, PayloadOpenMode::CreateNew).is_err());
    }
}

#[test]
fn cache_lock_rejects_links_and_public_permissions_before_staging() {
    for kind in [UnsafeEntry::Symlink, UnsafeEntry::HardLink, UnsafeEntry::PublicMode] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projection");
        let directory = PrivateDirectory::create(&path).unwrap();
        let outside = temp.path().join("outside");
        unsafe_entry(&path.join("cache.lock"), &outside, b"preserved", kind);
        filesystem_rejected(prepare(directory, &[]));
        assert_eq!(fs::read(&outside).unwrap(), b"preserved");
        assert_eq!(fs::read(path.join("cache.lock")).unwrap(), b"preserved");
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    }
}

#[test]
fn projection_rejects_linked_markers_and_public_directories_before_cache_creation() {
    for kind in [UnsafeEntry::Symlink, UnsafeEntry::HardLink] {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = temp.path().join("projection");
        let _directory = PrivateDirectory::create(&path).unwrap();
        let outside = temp.path().join("outside");
        unsafe_entry(&path.join("projection-kind.json"), &outside, MARKER, kind);
        filesystem_rejected(rebuild_projection(&path, &[]));
        assert_eq!(fs::read(&outside).unwrap(), MARKER);
        assert_eq!(fs::read(path.join("projection-kind.json")).unwrap(), MARKER);
        assert!(!path.join("cache.db").exists());
        assert!(!path.join("cache.lock").exists());
    }
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("projection");
    let _directory = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("projection-kind.json"), MARKER);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    filesystem_rejected(rebuild_projection(&path, &[]));
    assert_eq!(fs::read(path.join("projection-kind.json")).unwrap(), MARKER);
    assert!(!path.join("cache.db").exists());
    assert!(!path.join("cache.lock").exists());
}

#[test]
fn cache_reads_reject_public_permissions_and_publication_rejects_linked_targets() {
    for kind in [UnsafeEntry::Symlink, UnsafeEntry::HardLink] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("projection");
        let directory = PrivateDirectory::create(&path).unwrap();
        let outside = temp.path().join("outside");
        unsafe_entry(&path.join("cache.db"), &outside, b"preserved", kind);
        filesystem_rejected(read_held(&directory));
        let prepared = prepare(directory, &[]).unwrap();
        filesystem_rejected(prepared.publish());
        assert_eq!(fs::read(&outside).unwrap(), b"preserved");
        assert_eq!(fs::read(path.join("cache.db")).unwrap(), b"preserved");
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projection");
    let directory = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("cache.db"), b"preserved");
    fs::set_permissions(path.join("cache.db"), fs::Permissions::from_mode(0o644)).unwrap();
    filesystem_rejected(read_held(&directory));
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), b"preserved");
}

#[test]
fn directory_replacement_keeps_both_cache_files_and_retained_staging_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projection");
    let retained = temp.path().join("retained");
    let directory = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("cache.db"), b"retained cache");
    let prepared = prepare(directory, &[]).unwrap();
    let staging = prepared.staging.clone();
    let bytes = prepared.bytes.clone();
    fs::rename(&path, &retained).unwrap();
    let _replacement = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("cache.db"), b"replacement cache");
    filesystem_rejected(prepared.publish());
    assert_eq!(fs::read(retained.join("cache.db")).unwrap(), b"retained cache");
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), b"replacement cache");
    assert_eq!(fs::read(retained.join(staging.as_path())).unwrap(), bytes);
    assert_eq!(fs::read_dir(path).unwrap().count(), 1);
}

#[test]
fn staging_replacement_is_rejected_before_the_published_cache_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projection");
    let directory = PrivateDirectory::create(&path).unwrap();
    private_file(&path.join("cache.db"), b"previous cache bytes");
    let prepared = prepare(directory, &[]).unwrap();
    let staging = path.join(prepared.staging.as_path());
    fs::remove_file(&staging).unwrap();
    private_file(&staging, b"replacement staging");
    filesystem_rejected(prepared.publish());
    assert_eq!(fs::read(path.join("cache.db")).unwrap(), b"previous cache bytes");
    assert_eq!(fs::read(staging).unwrap(), b"replacement staging");
}
