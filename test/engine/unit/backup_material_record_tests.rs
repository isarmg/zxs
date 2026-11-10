use super::*;
use std::os::unix::fs::PermissionsExt;

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    (temp, root)
}

#[test]
fn absent_record_is_only_allowed_during_preflight() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"complete".as_slice().into(),
        maximum: 8,
    };
    record.check_existing(&root).unwrap();
    let expected = root
        .read_bytes(&root.path().join(record.name), record.maximum)
        .unwrap_err();
    let actual = record.require(&root).unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
    assert!(!root.path().join(record.name).exists());
}

#[test]
fn installation_is_idempotent_and_survives_reopen() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"complete".as_slice().into(),
        maximum: 8,
    };
    record.install(&root).unwrap();
    record.install(&root).unwrap();
    record.check_existing(&root).unwrap();
    record
        .require(&DiskRoot::open(root.path()).unwrap())
        .unwrap();
}

#[test]
fn mismatches_keep_preflight_and_observation_error_codes() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"new".as_slice().into(),
        maximum: 3,
    };
    let path = root.path().join(record.name);
    root.write_atomic(&path, b"old", false).unwrap();
    assert_eq!(
        record.check_existing(&root).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(
        record.require(&root).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    assert_eq!(
        record.install(&root).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(root.read_bytes(&path, 3).unwrap(), b"old");
}

#[test]
fn reader_budget_is_not_replaced_by_expected_length() {
    for record in [
        RootRecord {
            name: "completion.json",
            bytes: b"new".as_slice().into(),
            maximum: 8,
        },
        RootRecord::owned("completion.json", 8, b"new".to_vec()),
    ] {
        let (_temp, root) = root();
        let path = root.path().join(record.name);
        root.write_atomic(&path, b"old-long", false).unwrap();
        assert_eq!(
            record.require(&root).unwrap_err().code(),
            ErrorCode::ConfigConflict
        );
        assert_eq!(
            record.check_existing(&root).unwrap_err().code(),
            ErrorCode::ObjectCollision
        );
        let exact = RootRecord {
            maximum: record.bytes.len(),
            ..record
        };
        assert_eq!(
            exact.require(&root).unwrap_err().code(),
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            exact.check_existing(&root).unwrap_err().code(),
            ErrorCode::ResourceLimit
        );
        assert_eq!(root.read_bytes(&path, 8).unwrap(), b"old-long");
    }
}

#[test]
fn unsafe_record_path_is_rejected_without_touching_its_target() {
    let (_temp, root) = root();
    let record = RootRecord {
        name: "completion.json",
        bytes: b"new".as_slice().into(),
        maximum: 3,
    };
    let target = root.path().join("preserved.json");
    root.write_atomic(&target, b"old", false).unwrap();
    let path = root.path().join(record.name);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let expected = root.read_bytes(&path, record.maximum).unwrap_err();
    let actual = record.require(&root).unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
    assert!(record.check_existing(&root).is_err());
    assert!(record.install(&root).is_err());
    assert_eq!(root.read_bytes(&target, 3).unwrap(), b"old");
    assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
}

#[test]
fn preparation_plan_borrows_existing_bytes_and_owns_generated_manifest() {
    let manifest = b"manifest";
    let payload = b"payload";
    let prepared = PreparedFiles::new(
        Namespace::Objects,
        manifest.as_slice(),
        [(PathBuf::from("nested/data"), payload.as_slice())],
    )
    .unwrap();
    assert_eq!(prepared.manifest().as_ptr(), manifest.as_ptr());
    assert_eq!(prepared.entries.len(), 3);
    assert!(matches!(
        prepared.entries[Path::new("nested")],
        Entry::Directory
    ));
    let files: Vec<_> = prepared.data_files().collect();
    assert_eq!(files, [(Path::new("nested/data"), payload.as_slice())]);
    assert_eq!(files[0].1.as_ptr(), payload.as_ptr());
    let owned = PreparedFiles::new(Namespace::Scopes, manifest.to_vec(), []).unwrap();
    assert_eq!(owned.manifest(), manifest);
    assert_eq!(owned.data_files().count(), 0);
}

#[test]
fn preparation_plan_rejects_conflicting_topology_at_construction() {
    for paths in [
        vec!["same", "same"],
        vec!["parent", "parent/file"],
        vec!["parent/file", "parent"],
        vec![MANIFEST],
        vec!["manifest.json/file"],
        vec![""],
        vec!["../outside"],
        vec!["/absolute"],
    ] {
        assert!(
            PreparedFiles::new(
                Namespace::Objects,
                b"manifest".as_slice(),
                paths
                    .into_iter()
                    .map(|path| (PathBuf::from(path), b"body".as_slice())),
            )
            .is_err()
        );
    }
}

#[test]
fn preparation_plan_counts_shared_directories_once_and_keeps_sorted_files() {
    let prepared = PreparedFiles::new(
        Namespace::Objects,
        b"manifest".as_slice(),
        [
            (PathBuf::from("shared/z"), b"z".as_slice()),
            (PathBuf::from("shared/inner/b"), b"b".as_slice()),
            (PathBuf::from("shared/inner/a"), b"a".as_slice()),
        ],
    )
    .unwrap();
    assert_eq!(prepared.entries.len(), 6);
    assert_eq!(
        prepared
            .data_files()
            .map(|(path, _)| path)
            .collect::<Vec<_>>(),
        [
            Path::new("shared/inner/a"),
            Path::new("shared/inner/b"),
            Path::new("shared/z")
        ],
    );
    let (_temp, root) = root();
    let stop = AtomicBool::new(false);
    let overhead = 6 * 8192 + RESERVE;
    assert_eq!(
        required_capacity(&root, &prepared, &stop).unwrap(),
        11 + overhead
    );
    install(&root, &prepared, &stop).unwrap();
    verify(&root, &prepared, &stop).unwrap();
    assert_eq!(
        required_capacity(&root, &prepared, &stop).unwrap(),
        overhead
    );
}

#[test]
fn completed_preparation_plan_rejects_missing_or_unknown_material_without_repair() {
    let prepared = PreparedFiles::new(
        Namespace::Objects,
        b"manifest".as_slice(),
        [(PathBuf::from("nested/data"), b"payload".as_slice())],
    )
    .unwrap();
    let (_temp, root) = root();
    let stop = AtomicBool::new(false);
    install(&root, &prepared, &stop).unwrap();
    let base = root.path().join("object-databases");
    let data = base.join("nested/data");
    root.remove_file_synced(&data).unwrap();
    assert_eq!(
        verify(&root, &prepared, &stop).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    assert!(!data.exists());
    root.write_atomic(&base.join("unknown"), b"retained", false)
        .unwrap();
    assert_eq!(
        verify(&root, &prepared, &stop).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(
        root.read_bytes(&base.join("unknown"), 8).unwrap(),
        b"retained"
    );
    assert_eq!(
        root.read_bytes(&base.join(MANIFEST), 8).unwrap(),
        b"manifest"
    );
}

#[test]
fn shared_file_reference_keeps_the_original_serialized_fields_and_digest() {
    #[derive(Serialize)]
    struct Original {
        bytes: usize,
        digest: String,
    }
    for bytes in [b"".as_slice(), b"one".as_slice(), b"\0\xff\x01".as_slice()] {
        let expected = Original {
            bytes: bytes.len(),
            digest: blake3::hash(bytes).to_hex().to_string(),
        };
        let actual = FileReference::from_bytes(bytes);
        assert_eq!(
            canonical_json(&actual).unwrap(),
            canonical_json(&expected).unwrap()
        );
        assert_eq!(actual.bytes(), bytes.len());
        assert!(actual.matches(bytes));
        assert!(!actual.matches(b"different"));
    }
    assert!(!FileReference::from_bytes(b"one").matches(b"two"));
}

#[test]
fn preparation_plan_matches_the_original_closed_inventory_for_path_combinations() {
    fn original(paths: &[&str]) -> Option<BTreeMap<PathBuf, bool>> {
        let mut files = BTreeSet::new();
        for &path in paths {
            let path = PathBuf::from(path);
            if path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || path == Path::new(MANIFEST)
                || !files.insert(path)
            {
                return None;
            }
        }
        let mut expected = BTreeMap::from([(PathBuf::from(MANIFEST), true)]);
        for path in files {
            if expected.insert(path.clone(), true).is_some() {
                return None;
            }
            for parent in path
                .ancestors()
                .skip(1)
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                if expected.insert(parent.to_owned(), false) == Some(true) {
                    return None;
                }
            }
        }
        Some(expected)
    }
    let paths = [
        "a",
        "b",
        "a/b",
        "a/b/c",
        MANIFEST,
        "manifest.json/child",
        "",
        "../escape",
        "/absolute",
    ];
    for first in paths {
        for second in paths {
            for third in paths {
                let inputs = [first, second, third];
                let expected = original(&inputs);
                let actual = PreparedFiles::new(
                    Namespace::Objects,
                    b"manifest".as_slice(),
                    inputs.map(|path| (PathBuf::from(path), b"body".as_slice())),
                );
                match (expected, actual) {
                    (Some(expected), Ok(actual)) => {
                        let actual: BTreeMap<_, _> = actual
                            .entries
                            .iter()
                            .map(|(path, entry)| (path.clone(), matches!(entry, Entry::File(_))))
                            .collect();
                        assert_eq!(actual, expected, "{inputs:?}");
                    }
                    (None, Err(error)) => assert_eq!(error.code(), ErrorCode::ConfigConflict),
                    (expected, actual) => panic!(
                        "topology admission differs for {inputs:?}: expected valid={}, actual valid={}",
                        expected.is_some(),
                        actual.is_ok()
                    ),
                }
            }
        }
    }
}

#[test]
fn owned_root_record_moves_the_canonical_buffer_without_copying_or_reencoding() {
    let bytes = br#"{"format":"native-completion","targets":2}"#.to_vec();
    let address = bytes.as_ptr();
    let record = RootRecord::owned("completion.json", 128, bytes);
    assert!(matches!(&record.bytes, Cow::Owned(_)));
    assert_eq!(record.bytes.as_ptr(), address);
    assert_eq!(record.maximum, 128);
    let borrowed = RootRecord {
        name: record.name,
        bytes: record.bytes.as_ref().into(),
        maximum: record.maximum,
    };
    assert!(matches!(&borrowed.bytes, Cow::Borrowed(_)));
    assert_eq!(borrowed.bytes.as_ptr(), address);
    let (_temp, root) = root();
    record.check_existing(&root).unwrap();
    assert!(record.require(&root).is_err());
    assert!(!root.path().join(record.name).exists());
    record.install(&root).unwrap();
    borrowed.require(&root).unwrap();
    assert_eq!(
        root.read_bytes(&root.path().join(record.name), record.maximum)
            .unwrap(),
        record.bytes.as_ref()
    );
    record
        .require(&DiskRoot::open(root.path()).unwrap())
        .unwrap();
    root.write_atomic(&root.path().join(record.name), b"changed", true)
        .unwrap();
    for expected in [&record, &borrowed] {
        assert_eq!(
            expected.check_existing(&root).unwrap_err().code(),
            ErrorCode::ObjectCollision
        );
        assert_eq!(
            expected.require(&root).unwrap_err().code(),
            ErrorCode::ConfigConflict
        );
    }
}

#[test]
fn owned_completion_cannot_replace_the_actual_material_readback() {
    let files = PreparedFiles::new(
        Namespace::Objects,
        b"manifest".as_slice(),
        [(PathBuf::from("payload"), b"complete bytes".as_slice())],
    )
    .unwrap();
    let record = RootRecord::owned("completion.json", 8, b"complete".to_vec());
    let (_temp, root) = root();
    let stop = AtomicBool::new(false);
    install(&root, &files, &stop).unwrap();
    record.install(&root).unwrap();
    record.require(&root).unwrap();
    verify(&root, &files, &stop).unwrap();
    let payload = root.path().join("object-databases/payload");
    root.remove_file_synced(&payload).unwrap();
    record.require(&root).unwrap();
    assert_eq!(
        verify(&root, &files, &stop).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    assert!(!payload.exists());
}
