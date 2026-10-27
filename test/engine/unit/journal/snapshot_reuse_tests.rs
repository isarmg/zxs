use super::*;
use std::fs;

fn fixture(count: usize) -> (tempfile::TempDir, Config, State, PreparedSnapshot) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let state = State {
        sequence: 7,
        file_notes: (0..count)
            .map(|n| (format!("file-{n:08}"), "retained payload ".repeat(32)))
            .collect(),
        ..Default::default()
    };
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    prepared.store(&config.disks, &state).unwrap();
    (temp, config, state, prepared)
}
fn next(state: &State) -> State {
    let mut next = state.clone();
    next.begin_tracking();
    next.sequence += 1;
    next
}
fn heat(state: &State, prepared: &PreparedSnapshot) -> (State, PreparedSnapshot) {
    let next = next(state);
    let hot =
        prepare_snapshot_incremental(state, &next, &prepared.reuse_basis(), MAX_METADATA).unwrap();
    (next, hot)
}

fn note_pages(config: &Config, prepared: &PreparedSnapshot) -> Vec<String> {
    map_pages(&config.disks[0], &prepared.descriptor.maps["file_notes"])
}

#[test]
fn raw_page_digest_is_the_original_canonical_domain_digest_not_a_semantic_load() {
    let (_temp, config, state, prepared) = fixture(12);
    let hashes = note_pages(&config, &prepared);
    let hash = &hashes[0];
    let bytes = read_published_object(&config.disks, "state-page", hash, MAX_METADATA, &mut |_| {})
        .unwrap();
    let page: json_codec::Value = decode_strict(&bytes).unwrap();
    let (canonical, digest) = encode_hashed_object("state-page", &page).unwrap();
    assert_eq!(bytes, canonical);
    assert_eq!(digest, *hash);
    assert_ne!(object_hash("metadata-index", &page).unwrap(), *hash);
    let noncanonical = [b" ".as_slice(), bytes.as_slice()].concat();
    for disk in &config.disks {
        fs::write(object_path(disk, "state-page", hash), &noncanonical).unwrap();
    }
    // A generic semantic load accepts harmless whitespace; the reuse fast path
    // must not mistake that for the originally published exact byte string.
    let _: json_codec::Value = load_object(&config.disks[0], "state-page", hash).unwrap();
    assert_eq!(
        read_published_object(&config.disks, "state-page", hash, MAX_METADATA, &mut |_| {})
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    let (next, hot) = heat(&state, &prepared);
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    for disk in &config.disks {
        assert_eq!(
            fs::read(object_path(disk, "state-page", hash)).unwrap(),
            noncanonical
        );
    }
}

#[test]
fn reused_fields_hold_one_page_buffer_and_never_serialize_their_records() {
    let (_temp, config, state, prepared) = fixture(4096);
    let maximum = prepared.fields["file_notes"].maximum_object_bytes();
    assert!(note_pages(&config, &prepared).len() > 16);
    let next = next(&state);
    snapshot::take_encoding_counts();
    let hot =
        prepare_snapshot_incremental(&state, &next, &prepared.reuse_basis(), MAX_METADATA).unwrap();
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    REUSED_OBJECT_PEAK_BYTES.with(|peak| peak.set(0));
    hot.store(&config.disks, &next).unwrap();
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    assert_eq!(REUSED_OBJECT_PEAK_BYTES.with(|peak| peak.get()), maximum);
    for disk in &config.disks {
        assert_eq!(load_raw_snapshot_fixture(disk, &hot.hash).unwrap(), next);
    }
}

#[test]
fn snapshot_reuse_reads_and_installs_only_in_its_admitted_resources_after_rebinding() {
    let (temp, config, state, prepared) = fixture(256);
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let resources = DiskResources::open(&admitted, false).unwrap();
    let (next, hot) = heat(&state, &prepared);
    for (index, disk) in config.disks.iter().enumerate() {
        fs::rename(&disk.root, temp.path().join(format!("held-{index}"))).unwrap();
        private_dir(&disk.root).unwrap();
    }
    snapshot::take_encoding_counts();
    hot.store_at(&resources, &config.disks, &next).unwrap();
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    for disk in &config.disks {
        let root = &resources.get(disk).unwrap().root;
        let descriptor = load_object_at(root, disk, "snapshot", &hot.hash).unwrap();
        let restored = resolve_raw_snapshot(descriptor, |kind, hash| {
            load_authenticated_object_at(root, disk, kind, hash)
        })
        .unwrap()
        .into_codec_fixture();
        assert_eq!(restored, next);
        assert!(!disk.root.join("metadata").exists());
    }
}

#[test]
fn missing_pages_reconstruct_the_field_once_with_the_identical_reference_sequence() {
    let (_temp, config, state, prepared) = fixture(512);
    snapshot::take_encoding_counts();
    prepare_snapshot(&state, MAX_METADATA).unwrap();
    let expected_serializations = snapshot::take_encoding_counts()["file_notes"];
    let (next, hot) = heat(&state, &prepared);
    for hash in note_pages(&config, &prepared).iter().skip(1) {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    snapshot::take_encoding_counts();
    hot.store(&config.disks, &next).unwrap();
    assert_eq!(
        snapshot::take_encoding_counts(),
        BTreeMap::from([("file_notes".into(), expected_serializations)])
    );
    assert_eq!(
        hot.descriptor.maps["file_notes"],
        prepared.descriptor.maps["file_notes"]
    );
    for disk in &config.disks {
        assert_eq!(load_raw_snapshot_fixture(disk, &hot.hash).unwrap(), next);
    }
}

#[test]
fn field_reconstruction_cannot_substitute_different_bytes_or_a_different_page_count() {
    let (_temp, config, state, prepared) = fixture(256);
    let (mut wrong, hot) = heat(&state, &prepared);
    for hash in &note_pages(&config, &prepared) {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    wrong.file_notes.clear();
    assert_eq!(
        hot.store(&config.disks, &wrong).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    wrong.file_notes = state.file_notes.clone();
    wrong
        .file_notes
        .insert("file-00000000".into(), "different".into());
    assert_eq!(
        hot.store(&config.disks, &wrong).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    let correct = next(&state);
    hot.store(&config.disks, &correct).unwrap();
    assert_eq!(
        load_raw_snapshot_fixture(&config.disks[0], &hot.hash).unwrap(),
        correct
    );
}

#[test]
fn corrupt_large_targets_and_unsafe_targets_are_not_overwritten_or_buffered_to_global_limit() {
    let (temp, config, state, prepared) = fixture(300);
    let (next, hot) = heat(&state, &prepared);
    let hashes = note_pages(&config, &prepared);
    let hash = &hashes[0];
    let path = object_path(&config.disks[0], "state-page", hash);
    let size = prepared.fields["file_notes"].maximum_object_bytes() + 1024 * 1024;
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size as u64)
        .unwrap();
    REUSED_OBJECT_PEAK_BYTES.with(|peak| peak.set(0));
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), size as u64);
    assert!(
        REUSED_OBJECT_PEAK_BYTES.with(|peak| peak.get())
            <= prepared.fields["file_notes"].maximum_object_bytes()
    );
    let outside = temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
}

#[test]
fn existing_reused_pages_cannot_bypass_a_failed_directory_barrier() {
    let (_temp, config, state, prepared) = fixture(128);
    let (next, hot) = heat(&state, &prepared);
    let mut barriers = 0;
    let error = hot
        .store_with(&config.disks, &next, |disk, kind, hash, bytes| {
            object_storage::write_object_bytes_with(disk, kind, hash, bytes, |_, _| {
                barriers += 1;
                Err(Error::new(
                    storage_contract::ErrorCode::InjectedFailure,
                    "parent sync failed after bytes became visible",
                ))
            })
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InjectedFailure);
    assert_eq!(barriers, 1);
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    hot.store_with(&config.disks, &next, |disk, kind, hash, bytes| {
        barriers += 1;
        write_object_bytes(disk, kind, hash, bytes)
    })
    .unwrap();
    assert!(barriers > 1);
    assert_eq!(
        load_raw_snapshot_fixture(&config.disks[0], &hot.hash).unwrap(),
        next
    );
}

#[test]
fn reused_page_maxima_are_admitted_before_reading_sources_or_reencoding_records() {
    let (_temp, config, state, prepared) = fixture(1024);
    let basis = prepared.reuse_basis();
    let maximum = basis.fields["file_notes"].maximum_object_bytes();
    assert!(maximum > prepared.objects.last().unwrap().bytes.len());
    for hash in &note_pages(&config, &prepared) {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    snapshot::take_encoding_counts();
    let error = prepare_snapshot_incremental(&state, &next(&state), &basis, maximum - 1)
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    assert!(prepare_snapshot_incremental(&state, &next(&state), &basis, maximum).is_ok());
}

#[test]
fn large_opaque_auth_is_not_copied_into_the_bounded_snapshot_root() {
    let state = State {
        control_auth: Some(json_codec::Value::String("x".repeat(8 * 1024 * 1024))),
        ..Default::default()
    };
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert!(prepared.objects.last().unwrap().bytes.len() < 8192);
    let basis = prepared.reuse_basis();
    assert!(canonical_json(&basis.descriptor).unwrap().len() < 8192);
    let after = next(&state);
    let hot = prepare_snapshot_incremental(&state, &after, &basis, MAX_METADATA).unwrap();
    assert!(
        hot.objects
            .iter()
            .all(|object| object.kind != "state-control-auth")
    );
    assert_eq!(
        hot.hash,
        prepare_snapshot(&after, MAX_METADATA).unwrap().hash
    );
}

#[test]
fn middle_or_last_missing_page_keeps_completed_installation_callbacks_exactly_once() {
    for last in [false, true] {
        let (_temp, config, state, prepared) = fixture(512);
        let (next, hot) = heat(&state, &prepared);
        let expected = map_addresses(&config.disks[0], &hot.descriptor.maps["file_notes"]);
        let pages = note_pages(&config, &prepared);
        assert!(pages.len() > 2);
        let missing = if last {
            pages.len() - 1
        } else {
            pages.len() / 2
        };
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", &pages[missing])).unwrap();
        }
        let mut visited = Vec::new();
        hot.store_with(&config.disks, &next, |disk, kind, hash, bytes| {
            if disk.disk_id == config.disks[0].disk_id
                && expected
                    .iter()
                    .any(|(domain, digest)| *domain == kind && digest == hash)
            {
                visited.push((kind.to_owned(), hash.to_owned()));
            }
            write_object_bytes(disk, kind, hash, bytes)
        })
        .unwrap();
        assert_eq!(
            visited,
            expected
                .into_iter()
                .map(|(kind, hash)| (kind.to_owned(), hash))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            load_raw_snapshot_fixture(&config.disks[0], &hot.hash).unwrap(),
            next
        );
    }
}

#[test]
#[should_panic(expected = "invariant: prepared snapshot ends with its root")]
fn missing_prepared_root_is_an_internal_bug() {
    let mut prepared = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
    prepared.objects.clear();
    prepared.reuse_basis();
}

#[test]
#[should_panic(expected = "invariant: every snapshot field has an installation plan")]
fn missing_prepared_field_plan_is_an_internal_bug() {
    let state = State::default();
    let mut prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    prepared.fields.remove("versions");
    prepared.store(&[], &state).unwrap();
}

#[test]
fn oversized_first_source_reports_its_bound_fault_even_when_a_healthy_replica_succeeds() {
    let (_temp, config, _state, prepared) = fixture(12);
    let pages = note_pages(&config, &prepared);
    let hash = &pages[0];
    let path = object_path(&config.disks[0], "state-page", hash);
    let expected = std::fs::read(&path).unwrap();
    let mut damaged = expected.clone();
    damaged.push(b' ');
    std::fs::write(&path, &damaged).unwrap();
    let mut failures = Vec::new();
    let actual = read_published_object(
        &config.disks,
        "state-page",
        hash,
        expected.len(),
        &mut |error| {
            failures.push(error.code());
        },
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(failures, vec![ErrorCode::ResourceLimit]);
    assert_eq!(
        std::fs::read(path).unwrap(),
        damaged,
        "source selection never repairs or replaces evidence"
    );
}

#[test]
fn newly_encoded_snapshot_objects_never_use_existing_names_as_installation_evidence() {
    let (_temp, config, state, prepared) = fixture(24);
    let duplicate = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_eq!(duplicate.hash, prepared.hash);
    let object = duplicate
        .objects
        .iter()
        .find(|object| object.kind == "state-page")
        .unwrap();
    let path = object_path(&config.disks[2], object.kind, &object.hash);
    fs::write(&path, b"failed earlier installation").unwrap();
    assert_eq!(
        duplicate
            .store_new_objects(&config.disks)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(fs::read(&path).unwrap(), b"failed earlier installation");
    fs::write(&path, &object.bytes).unwrap();
    duplicate.store_new_objects(&config.disks).unwrap();
}
