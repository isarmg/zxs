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
