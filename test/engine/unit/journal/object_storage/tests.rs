use super::*;
use json_codec::{Value, json};
fn config(temp: &tempfile::TempDir) -> Config {
    Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ])
}
#[test]
fn current_shards_preserve_hashes_and_reject_conflicting_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp);
    let disk = &config.disks[0];
    let value = json!({"retained": "历史"});
    let hash = store_object(disk, "publication", &value).unwrap();
    let path = object_path(disk, "publication", &hash);
    assert_eq!(fs::read(&path).unwrap(), canonical_json(&value).unwrap());
    assert_eq!(
        load_object::<Value>(disk, "publication", &hash).unwrap(),
        value
    );
    assert_eq!(store_object(disk, "publication", &value).unwrap(), hash);
    assert_eq!(
        list_object_hashes(disk, "publication").unwrap(),
        BTreeSet::from([hash.clone()])
    );
    fs::write(&path, b"broken").unwrap();
    assert!(load_object::<Value>(disk, "publication", &hash).is_err());
    assert!(store_object(disk, "publication", &value).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"broken");
    for invalid in ["../outside", "é", &"A".repeat(64)] {
        assert!(load_object::<Value>(disk, "publication", invalid).is_err());
    }
    assert!(load_object::<Value>(disk, "../outside", &hash).is_err());
}

#[test]
fn matching_object_retry_cannot_bypass_a_failed_publication_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp);
    let disk = &config.disks[0];
    let value = json!({"publication": "retry"});
    let hash = store_object(disk, "publication", &value).unwrap();
    let bytes = canonical_json(&value).unwrap();
    let mut reached = false;
    let failure = write_object_bytes_with(disk, "publication", &hash, &bytes, |_, actual| {
        reached = true;
        assert_eq!(actual, bytes);
        Err(Error::new(
            storage_contract::ErrorCode::InjectedFailure,
            "parent sync failed",
        ))
    });
    assert!(
        reached,
        "existing bytes cannot replace the publication barrier"
    );
    assert_eq!(
        failure.unwrap_err().code(),
        storage_contract::ErrorCode::InjectedFailure
    );
    assert_eq!(store_object(disk, "publication", &value).unwrap(), hash);
    assert_eq!(
        load_object::<Value>(disk, "publication", &hash).unwrap(),
        value
    );
}

#[test]
fn flat_objects_are_rejected_without_migration_or_deletion() {
    for sharded_copy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let config = config(&temp);
        let disk = &config.disks[0];
        let value = json!({"retained": "old format"});
        let hash = object_hash("publication", &value).unwrap();
        if sharded_copy {
            store_object(disk, "publication", &value).unwrap();
        }
        let flat = flat_path(disk, "publication", &hash);
        write_json(&flat, &value, false).unwrap();
        let bytes = fs::read(&flat).unwrap();
        assert_eq!(
            load_object::<Value>(disk, "publication", &hash)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FormatUnsupported
        );
        assert_eq!(
            store_object(disk, "publication", &value)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FormatUnsupported
        );
        assert_eq!(
            list_object_hashes(disk, "publication").unwrap_err().code(),
            storage_contract::ErrorCode::FormatUnsupported
        );
        assert_eq!(fs::read(flat).unwrap(), bytes);
        assert_eq!(
            object_path(disk, "publication", &hash).exists(),
            sharded_copy
        );
    }
}
#[test]
fn inventory_rejects_wrong_shards_and_symlink_fallbacks() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp);
    let disk = &config.disks[0];
    let value = json!({"v": 1});
    let hash = store_object(disk, "publication", &value).unwrap();
    let path = object_path(disk, "publication", &hash);
    let flat = flat_path(disk, "publication", &hash);
    symlink(&path, &flat).unwrap();
    assert!(load_object::<Value>(disk, "publication", &hash).is_err());
    assert!(store_object(disk, "publication", &value).is_err());
    assert!(list_object_hashes(disk, "publication").is_err());
    fs::remove_file(&flat).unwrap();
    write_json(&flat, &value, false).unwrap();
    fs::remove_file(&path).unwrap();
    fs::remove_dir(path.parent().unwrap()).unwrap();
    symlink(temp.path().join("missing"), path.parent().unwrap()).unwrap();
    assert!(load_object::<Value>(disk, "publication", &hash).is_err());
    assert!(store_object(disk, "publication", &value).is_err());
    assert!(list_object_hashes(disk, "publication").is_err());
    fs::remove_file(path.parent().unwrap()).unwrap();
    let wrong = if hash.starts_with("00") { "01" } else { "00" };
    let wrong_path = flat
        .parent()
        .unwrap()
        .join(wrong)
        .join(format!("{hash}.json"));
    write_json(&wrong_path, &value, false).unwrap();
    assert!(list_object_hashes(disk, "publication").is_err());
}

#[test]
fn immutable_writer_owns_existing_byte_validation() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp);
    let disk = &config.disks[0];
    let value = json!({"publication": "reuse"});
    let hash = store_object(disk, "publication", &value).unwrap();
    let path = object_path(disk, "publication", &hash);
    let bytes = canonical_json(&value).unwrap();
    for corrupt in [
        vec![b'x'; bytes.len()],
        b"short".to_vec(),
        vec![b'x'; bytes.len() + 1],
    ] {
        fs::write(&path, &corrupt).unwrap();
        let mut reached_writer = false;
        let error = write_object_bytes_with(disk, "publication", &hash, &bytes, |path, bytes| {
            reached_writer = true;
            write_atomic(path, bytes, false)
        })
        .unwrap_err();
        assert!(reached_writer, "outer layer must not compare bytes again");
        assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCollision);
        assert_eq!(fs::read(&path).unwrap(), corrupt);
    }
}

#[test]
fn immutable_writer_propagates_unsafe_target_errors_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    let target = temp.path().join("target");
    std::os::unix::fs::symlink(&outside, &target).unwrap();
    assert!(write_atomic(&target, b"replacement", false).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
}

#[test]
fn object_reads_inventory_and_publication_share_the_held_root_after_rebinding() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(&temp);
    let disk = &config.disks[0];
    let original = json!({"origin": "held"});
    let hash = store_object(disk, "publication", &original).unwrap();
    let held = DiskRoot::open(&disk.root).unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&disk.root, &moved).unwrap();
    let replacement = json!({"origin": "replacement"});
    let replacement_hash = store_object(disk, "publication", &replacement).unwrap();
    let next = json!({"origin": "next"});
    let next_hash = store_object_at(&held, disk, "publication", &next).unwrap();

    assert_eq!(
        load_object_at::<Value>(&held, disk, "publication", &hash).unwrap(),
        original
    );
    assert_eq!(
        load_object_at::<Value>(&held, disk, "publication", &next_hash).unwrap(),
        next
    );
    assert_eq!(
        list_object_hashes_at(&held, disk, "publication").unwrap(),
        BTreeSet::from([hash.clone(), next_hash.clone()])
    );
    assert_eq!(
        list_object_hashes(disk, "publication").unwrap(),
        BTreeSet::from([replacement_hash])
    );
    assert!(!object_path(disk, "publication", &hash).exists());
    assert!(!object_path(disk, "publication", &next_hash).exists());

    let original_path = moved.join(
        object_path(disk, "publication", &hash)
            .strip_prefix(&disk.root)
            .unwrap(),
    );
    fs::write(&original_path, b"broken").unwrap();
    assert!(load_object_at::<Value>(&held, disk, "publication", &hash).is_err());
    assert_eq!(
        store_object_at(&held, disk, "publication", &original)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(fs::read(&original_path).unwrap(), b"broken");
}
