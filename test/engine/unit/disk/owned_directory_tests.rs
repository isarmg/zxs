//! Real libc fsync failures in isolated libtest children, never a power-loss model.
use super::*;
use crate::journal::PreparedMetadataObject;

fn fixture() -> (PathBuf, DiskConfig, PreparedMetadataObject, PathBuf) {
    let root = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_IO_ROOT").unwrap());
    let disk = Config::development([root.clone()]).disks.remove(0);
    let object = PreparedMetadataObject::new(
        "publication",
        &json_codec::json!({"directory_barrier_probe": "fixed input"}),
        MAX_METADATA,
    )
    .unwrap();
    let target = root
        .join("metadata/objects/publication")
        .join(&object.hash[..2])
        .join(format!("{}.json", object.hash));
    (root, disk, object, target)
}

fn phase(root: &Path, phase: &str) {
    fs::write(root.join("phase"), phase).unwrap();
}

fn events(root: &Path) -> String {
    fs::read_to_string(root.join("events")).unwrap()
}

fn assert_eio(error: &Error) {
    let mut source: &(dyn std::error::Error + 'static) = error;
    loop {
        if source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.raw_os_error() == Some(5))
        {
            return;
        }
        source = source.source().expect("injected EIO retains its source");
    }
}

fn fail_first(
    root: &Path,
    held: &DiskRoot,
    disk: &DiskConfig,
    object: &PreparedMetadataObject,
    target: &Path,
) {
    phase(root, "first");
    fs::write(root.join("observe"), b"").unwrap();
    let error = object.store_on(held, disk).unwrap_err();
    assert_eio(&error);
    assert!(root.join("metadata/objects/publication").is_dir());
    assert!(!target.exists(), "the failed directory barrier precedes file creation");
    let events = events(root);
    let child = events.find("first dir metadata/objects/publication success\n").unwrap();
    let parent = events.find("first dir metadata/objects injected-EIO\n").unwrap();
    assert!(child < parent, "child sync precedes the failed parent sync");
    assert!(!events.contains("first file "), "no object file was synced before admission");
}

fn retry_and_reuse(
    root: &Path,
    held: &DiskRoot,
    disk: &DiskConfig,
    object: &PreparedMetadataObject,
    target: &Path,
) {
    phase(root, "retry");
    object.store_on(held, disk).unwrap();
    assert_eq!(fs::read(target).unwrap(), object.bytes());
    let log = events(root);
    let parent = log.find("retry dir metadata/objects success\n").expect(
        "a visible directory from a failed create still owes its parent barrier",
    );
    let file = log.find("retry file ").expect("the object file was really synced");
    assert!(parent < file, "complete ancestor durability before installing the object");
    assert!(!log.contains("retry dir metadata/objects injected-EIO\n"));

    phase(root, "stable");
    object.store_on(held, disk).unwrap();
    let log = events(root);
    assert!(!log.contains("stable dir metadata/objects success\n"));
    assert!(!log.contains("stable dir metadata/objects/publication success\n"));
    assert!(log.contains("stable file "), "immutable same-byte comparison retains its file barrier");
    let shard = target.parent().unwrap().strip_prefix(root).unwrap().to_str().unwrap();
    assert!(log.contains(&format!("stable dir {shard} success\n")));
    fs::write(root.join("completed"), b"retry-and-reuse").unwrap();
    println!("\nZXS_DIRECTORY_IO_COMPLETE retry-and-reuse");
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_retry_in_isolated_child() {
    let (root, disk, object, target) = fixture();
    private_dir(&root.join("metadata/objects")).unwrap();
    let held = DiskRoot::open(&root).unwrap();
    fail_first(&root, &held, &disk, &object, &target);
    retry_and_reuse(&root, &held, &disk, &object, &target);
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_first_failure_in_isolated_child() {
    let (root, disk, object, target) = fixture();
    private_dir(&root.join("metadata/objects")).unwrap();
    let held = DiskRoot::open(&root).unwrap();
    fail_first(&root, &held, &disk, &object, &target);
    fs::write(root.join("completed"), b"first-failure").unwrap();
    println!("\nZXS_DIRECTORY_IO_COMPLETE first-failure");
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_reopen_in_isolated_child() {
    let (root, disk, object, target) = fixture();
    assert_eq!(fs::read(root.join("completed")).unwrap(), b"first-failure");
    let held = DiskRoot::open(&root).unwrap();
    retry_and_reuse(&root, &held, &disk, &object, &target);
}
