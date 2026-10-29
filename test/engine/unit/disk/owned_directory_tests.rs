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
    assert!(
        !target.exists(),
        "the failed directory barrier precedes file creation"
    );
    let events = events(root);
    let child_failure = std::env::var("ZXS_DIRECTORY_IO_FAILURE").unwrap() == "child";
    if child_failure {
        assert!(events.contains("first dir metadata/objects/publication injected-EIO\n"));
    } else {
        let child = events
            .find("first dir metadata/objects/publication success\n")
            .unwrap();
        let parent = events
            .find("first dir metadata/objects injected-EIO\n")
            .unwrap();
        assert!(child < parent, "child sync precedes the failed parent sync");
    }
    assert_eq!(events.matches("injected-EIO\n").count(), 1);
    assert!(
        !events.contains("first file "),
        "no object file was synced before admission"
    );
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
    let parent = log
        .find("retry dir metadata/objects success\n")
        .expect("a visible directory from a failed create still owes its parent barrier");
    let file = log
        .find("retry file ")
        .expect("the object file was really synced");
    assert!(
        parent < file,
        "complete ancestor durability before installing the object"
    );
    assert!(!log.contains("retry dir metadata/objects injected-EIO\n"));

    phase(root, "stable");
    object.store_on(held, disk).unwrap();
    let log = events(root);
    assert!(!log.contains("stable dir metadata/objects success\n"));
    assert!(!log.contains("stable dir metadata/objects/publication success\n"));
    assert!(
        log.contains("stable file "),
        "immutable same-byte comparison retains its file barrier"
    );
    let shard = target
        .parent()
        .unwrap()
        .strip_prefix(root)
        .unwrap()
        .to_str()
        .unwrap();
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

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_replacement_in_isolated_child() {
    use std::os::unix::fs::PermissionsExt;
    let (root, disk, object, target) = fixture();
    let held = DiskRoot::open(&root).unwrap();
    phase(&root, "warm");
    fs::write(root.join("observe"), b"").unwrap();
    object.store_on(&held, &disk).unwrap();
    let old_shard = held.directory(target.parent().unwrap()).unwrap();
    let publication = root.join("metadata/objects/publication");
    let retained = root.join("retained-publication");
    fs::rename(&publication, &retained).unwrap();
    fs::create_dir(&publication).unwrap();
    fs::set_permissions(&publication, fs::Permissions::from_mode(0o700)).unwrap();
    // Keep the exact cached leaf inode, but attach it to a different parent.
    fs::rename(
        retained.join(&object.hash[..2]),
        publication.join(&object.hash[..2]),
    )
    .unwrap();
    assert_eq!(
        FileIdentity::from_fd(old_shard).unwrap(),
        FileIdentity::from_fd(held.directory(target.parent().unwrap()).unwrap()).unwrap()
    );
    retry_and_reuse(&root, &held, &disk, &object, &target);
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_parallel_in_isolated_child() {
    let (root, disk, object, _) = fixture();
    let initial = DiskRoot::open(&root).unwrap();
    object.store_on(&initial, &disk).unwrap();
    drop(initial);
    let held = DiskRoot::open(&root).unwrap();
    phase(&root, "parallel");
    fs::write(root.join("observe"), b"").unwrap();
    let gate = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    gate.wait();
                    object.store_on(&held, &disk).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    });
    let log = events(&root);
    assert_eq!(
        log.matches("parallel dir metadata/objects success\n").count(),
        2,
        "one owner proves objects itself and then its publication child once"
    );
    phase(&root, "stable");
    object.store_on(&held, &disk).unwrap();
    assert!(!events(&root).contains("stable dir metadata/objects success\n"));
    assert!(!events(&root).contains("stable dir metadata/objects/publication success\n"));
    fs::write(root.join("completed"), b"parallel").unwrap();
    println!("\nZXS_DIRECTORY_IO_COMPLETE parallel");
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_eviction_in_isolated_child() {
    let (root, disk, object, target) = fixture();
    let held = DiskRoot::open(&root).unwrap();
    phase(&root, "warm");
    fs::write(root.join("observe"), b"").unwrap();
    object.store_on(&held, &disk).unwrap();
    let mut shards = BTreeSet::new();
    for index in 0..4096 {
        let candidate = PreparedMetadataObject::new(
            "directory-probe",
            &json_codec::json!({"index": index}),
            MAX_METADATA,
        )
        .unwrap();
        if shards.insert(candidate.hash[..2].to_owned()) {
            candidate.store_on(&held, &disk).unwrap();
        }
        if shards.len() > MAX_DIRECTORY_RECEIPTS {
            break;
        }
    }
    assert_eq!(shards.len(), MAX_DIRECTORY_RECEIPTS + 1);
    assert!(held.durable_directories.lock().unwrap().len() <= MAX_DIRECTORY_RECEIPTS);
    retry_and_reuse(&root, &held, &disk, &object, &target);
}

#[test]
#[ignore = "requires owned_directory_io/run.sh isolated fsync interposer"]
fn directory_sync_engine_recovery_in_isolated_child() {
    let (root, _, _, _) = fixture();
    let system = root.parent().unwrap().join("engine-system");
    let mut engine = Engine::initialize(&system, Config::development([root.clone()])).unwrap();
    let operation = OperationId::generate();
    phase(&root, "first");
    fs::write(root.join("observe"), b"").unwrap();
    let error = engine
        .create_repository(
            &RepositoryName::parse("directory recovery").unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eio(&error);
    assert!(engine.is_read_only());
    assert!(engine.state().repositories.is_empty());
    assert_eq!(events(&root).matches("injected-EIO\n").count(), 1);
    drop(engine);

    phase(&root, "retry");
    let engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().repositories.len(), 1);
    let result = engine.operation(operation).unwrap();
    assert_eq!(result.operation_id, operation);
    assert_eq!(result.commit_state, CommitState::Published);
    let log = events(&root);
    let barrier = log.find("retry dir metadata/objects success\n").unwrap();
    let file = log.find("retry file ").unwrap();
    assert!(barrier < file);
    let receipt = canonical_json(&result).unwrap();
    drop(engine);
    phase(&root, "second-open");
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().repositories.len(), 1);
    assert_eq!(
        canonical_json(&engine.operation(operation).unwrap()).unwrap(),
        receipt
    );
    fs::write(root.join("completed"), b"engine-recovery").unwrap();
    println!("\nZXS_DIRECTORY_IO_COMPLETE engine-recovery");
}
