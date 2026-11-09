//! Temporary-root recovery cases, not publication or hardware crash claims.
use super::*;

const RECORD: &str = "file-history-move-decision-inputs/ordinary-installations.json";

fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, Vec<u8>)> {
    fn visit(path: &Path, all: &mut BTreeMap<PathBuf, (u64, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        all.insert(path.to_owned(), (metadata.ino(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), all);
            }
        }
    }
    let mut all = BTreeMap::new();
    visit(root, &mut all);
    all
}

fn assert_tree_unchanged(root: &Path, before: &BTreeMap<PathBuf, (u64, Vec<u8>)>) {
    let after = tree(root);
    assert_eq!(after.len(), before.len(), "retained resource count changed");
    for (path, original) in before {
        assert!(
            after.get(path) == Some(original),
            "retained resource changed: {}",
            path.display()
        );
    }
}

fn pending_disk(root: &Path) -> PathBuf {
    let mut found = Vec::new();
    for generation in fs::read_dir(root.join(".zxs-layout/database-installs")).unwrap() {
        for attempt in fs::read_dir(generation.unwrap().path()).unwrap() {
            let path = attempt.unwrap().path();
            let Ok(bytes) = fs::read(path.join("intent.json")) else {
                continue;
            };
            let intent: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
            if intent["source"]["scope"]["kind"] == "disk"
                && fs::metadata(path.join("payload"))
                    .is_ok_and(|metadata| Some(metadata.ino()) == intent["new"]["inode"].as_u64())
            {
                found.push(path);
            }
        }
    }
    assert_eq!(found.len(), 1);
    found.pop().unwrap()
}

fn refuses_without_writing(
    fixture: &mut Fixture,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) {
    let before = tree(fixture.temp.path());
    assert!(
        fixture
            .engine
            .read_file_history_move_ordinary(
                projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_tree_unchanged(fixture.temp.path(), &before);
    assert!(
        fixture
            .engine
            .prepare_file_history_move_ordinary(
                projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_tree_unchanged(fixture.temp.path(), &before);
    assert!(
        fixture
            .engine
            .inspect_file_history_move_joint_choice()
            .is_some()
    );
    assert!(
        !fixture
            .engine
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
}

#[test]
fn one_original_record_allows_only_exact_replica_completion() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let first = prepare(&mut fixture.engine, &projection, &recovery);
    let record = fs::read(fixture.temp.path().join("system").join(RECORD)).unwrap();
    for disk in &fixture.engine.config().disks {
        fs::remove_file(disk.root.join(RECORD)).unwrap();
    }
    let before = tree(fixture.temp.path());
    assert!(
        fixture
            .engine
            .read_file_history_move_ordinary(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_tree_unchanged(fixture.temp.path(), &before);
    let resumed = prepare(&mut fixture.engine, &projection, &recovery);
    assert_eq!(
        resumed.file_database_intents(),
        first.file_database_intents()
    );
    assert_eq!(
        resumed.disk_database_intents(),
        first.disk_database_intents()
    );
    for disk in &fixture.engine.config().disks {
        assert_eq!(fs::read(disk.root.join(RECORD)).unwrap(), record);
    }
    let after = tree(fixture.temp.path());
    for (path, entry) in before {
        assert!(
            after.get(&path) == Some(&entry),
            "retained resource changed: {}",
            path.display()
        );
    }
}

#[test]
fn surviving_binding_forbids_recreating_a_wholly_missing_installation() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let generation = pending_disk(&root).parent().unwrap().to_owned();
    fs::remove_dir_all(&generation).unwrap();
    refuses_without_writing(&mut fixture, &projection, &recovery);
    assert!(!generation.exists());
}

#[test]
fn rewritten_local_intent_and_equal_bytes_do_not_replace_a_frozen_pair() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let attempt = pending_disk(&root);
    let payload = attempt.join("payload");
    let old = root.join("retained-original-pending-database");
    fs::rename(&payload, &old).unwrap();
    fs::write(&payload, fs::read(&old).unwrap()).unwrap();
    fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();
    let mut intent: json_codec::Value =
        storage_contract::decode_strict(&fs::read(attempt.join("intent.json")).unwrap()).unwrap();
    intent["new"]["inode"] = json_codec::json!(fs::metadata(&payload).unwrap().ino());
    fs::write(
        attempt.join("intent.json"),
        storage_contract::canonical_json(&intent).unwrap(),
    )
    .unwrap();
    refuses_without_writing(&mut fixture, &projection, &recovery);
    assert_eq!(fs::read(payload).unwrap(), fs::read(old).unwrap());
}

#[test]
fn prematurely_exchanged_database_is_never_reported_as_pending() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let attempt = pending_disk(&root);
    fs::rename(
        root.join(".zxs-disk.sqlite"),
        root.join("retained-old-current"),
    )
    .unwrap();
    fs::rename(attempt.join("payload"), root.join(".zxs-disk.sqlite")).unwrap();
    refuses_without_writing(&mut fixture, &projection, &recovery);
    assert!(root.join("retained-old-current").is_file());
}
