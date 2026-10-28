use super::*;
use std::cell::Cell;

struct CountedValue {
    value: json_codec::Value,
    serializations: Cell<usize>,
}
impl Serialize for CountedValue {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.serializations.set(self.serializations.get() + 1);
        self.value.serialize(serializer)
    }
}

#[test]
fn replicated_metadata_matches_original_bytes_and_hash_with_less_serialization() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let value = CountedValue {
        value: json_codec::json!({"z": "18446744073709551615", "名字": [true, null, "data"], "a": 1}),
        serializations: Cell::new(0),
    };
    // Reproduce the prior snapshot/history-manifest storage algorithm.
    let original_bytes = encode_metadata(&value, MAX_METADATA).unwrap();
    let original_hash = object_hash("state-page", &value).unwrap();
    for disk in &config.disks {
        assert_eq!(
            store_object(disk, "state-page", &value).unwrap(),
            original_hash
        );
    }
    let original_serializations = value.serializations.replace(0);
    let config = Config::development([
        temp.path().join("new-primary"),
        temp.path().join("new-backup"),
        temp.path().join("new-tertiary"),
    ]);
    let hash =
        store_object_replicas_bounded(&config.disks, "state-page", &value, MAX_METADATA).unwrap();
    assert_eq!(hash, original_hash);
    // Canonical encoding validates then serializes: eight encodings become one.
    assert_eq!(original_serializations, 16);
    assert_eq!(value.serializations.get(), 2);
    for disk in &config.disks {
        assert_eq!(
            read_bytes(&object_path(disk, "state-page", &hash), MAX_METADATA).unwrap(),
            original_bytes
        );
        assert_eq!(
            load_object::<json_codec::Value>(disk, "state-page", &hash).unwrap(),
            value.value
        );
    }
    // Existing immutable bytes are still compared rather than overwritten.
    let path = object_path(&config.disks[0], "state-page", &hash);
    let mut damaged = original_bytes.clone();
    damaged[0] = b'!';
    std::fs::write(&path, &damaged).unwrap();
    assert_eq!(
        store_object_replicas_bounded(&config.disks, "state-page", &value, MAX_METADATA)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert_eq!(std::fs::read(path).unwrap(), damaged);
}

#[test]
fn prepared_metadata_serializes_only_at_preflight_and_reuses_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let value = CountedValue {
        value: json_codec::json!({"retained": ["history", "events", "idempotency"]}),
        serializations: Cell::new(0),
    };
    let prepared = PreparedMetadataObject::new("state-page", &value, MAX_METADATA).unwrap();
    assert_eq!(value.serializations.get(), 2);
    let expected_hash = object_hash("state-page", &value.value).unwrap();
    let expected_bytes = encode_metadata(&value.value, MAX_METADATA).unwrap();
    assert_eq!(prepared.hash, expected_hash);
    for _ in 0..2 {
        assert_eq!(prepared.store(&config.disks).unwrap(), expected_hash);
    }
    assert_eq!(
        value.serializations.get(),
        2,
        "replica writes must not encode again"
    );
    for disk in &config.disks {
        assert_eq!(
            read_bytes(
                &object_path(disk, "state-page", &expected_hash),
                MAX_METADATA
            )
            .unwrap(),
            expected_bytes
        );
    }
}

#[test]
fn prepared_publication_objects_encode_once_across_individual_held_member_stores() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    for kind in ["candidate", "publication"] {
        let value = CountedValue {
            value: json_codec::json!({"width": "18446744073709551615", "名字": ["\n\"\\", null]}),
            serializations: Cell::new(0),
        };
        let prepared = PreparedMetadataObject::new(kind, &value, MAX_METADATA).unwrap();
        let expected = encode_metadata(&value.value, MAX_METADATA).unwrap();
        let hash = object_hash(kind, &value.value).unwrap();
        assert_eq!(prepared.hash, hash);
        let members = engine.members();
        for (index, disk) in members.iter().enumerate() {
            assert_eq!(
                prepared
                    .store_at(&engine.disk_resources, std::slice::from_ref(disk))
                    .unwrap(),
                hash
            );
            assert_eq!(
                std::fs::read(object_path(disk, kind, &hash)).unwrap(),
                expected
            );
            for pending in &members[index + 1..] {
                assert!(!object_path(pending, kind, &hash).exists());
            }
        }
        // Canonical encoding validates then serializes once; store_at does neither.
        assert_eq!(value.serializations.get(), 2);
        let path = object_path(&members[1], kind, &hash);
        std::fs::write(&path, b"damaged immutable object").unwrap();
        assert_eq!(
            prepared
                .store_at(&engine.disk_resources, std::slice::from_ref(&members[1]))
                .unwrap_err()
                .code(),
            ErrorCode::ObjectCollision
        );
        assert_eq!(std::fs::read(path).unwrap(), b"damaged immutable object");
        assert_eq!(value.serializations.get(), 2);
    }
}

#[test]
fn replicated_metadata_rejects_oversize_before_creating_any_object() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let value = json_codec::json!({"content": "bounded"});
    let bytes = canonical_json(&value).unwrap();
    let error = store_object_replicas_bounded(&config.disks, "snapshot", &value, bytes.len() - 1)
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(config.disks.iter().all(|disk| !disk.root.exists()));
}
