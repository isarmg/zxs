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
        prepare_snapshot_incremental(state, &next, &prepared.reuse_basis().unwrap(), MAX_METADATA)
            .unwrap();
    (next, hot)
}

#[test]
fn raw_page_digest_is_the_original_canonical_domain_digest_not_a_semantic_load() {
    let (_temp, config, state, prepared) = fixture(12);
    let hash = &prepared.descriptor.pages["file_notes"][0];
    let bytes = read_published_page(&config.disks, hash, MAX_METADATA).unwrap();
    let page: json_codec::Value = decode_strict(&bytes).unwrap();
    let (canonical, digest) = encode_hashed_object("state-page", &page).unwrap();
    assert_eq!(bytes, canonical);
    assert_eq!(digest, *hash);
    assert_ne!(object_hash("history-index", &page).unwrap(), *hash);
    let noncanonical = [b" ".as_slice(), bytes.as_slice()].concat();
    for disk in &config.disks {
        fs::write(object_path(disk, "state-page", hash), &noncanonical).unwrap();
    }
    // A generic semantic load accepts harmless whitespace; the reuse fast path
    // must not mistake that for the originally published exact byte string.
    let _: json_codec::Value = load_object(&config.disks[0], "state-page", hash).unwrap();
    assert_eq!(
        read_published_page(&config.disks, hash, MAX_METADATA)
            .unwrap_err()
            .code(),
        "OBJECT_CORRUPT"
    );
    let (next, hot) = heat(&state, &prepared);
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        "OBJECT_COLLISION"
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
    let maximum = prepared.fields["file_notes"].max_page_bytes;
    assert!(prepared.descriptor.pages["file_notes"].len() > 50);
    let next = next(&state);
    snapshot::take_encoding_counts();
    let hot = prepare_snapshot_incremental(
        &state,
        &next,
        &prepared.reuse_basis().unwrap(),
        MAX_METADATA,
    )
    .unwrap();
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    REUSED_PAGE_PEAK_BYTES.with(|peak| peak.set(0));
    hot.store(&config.disks, &next).unwrap();
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    assert_eq!(REUSED_PAGE_PEAK_BYTES.with(|peak| peak.get()), maximum);
    for disk in &config.disks {
        assert_eq!(load_snapshot(disk, &hot.hash).unwrap(), next);
    }
}

#[test]
fn missing_pages_reconstruct_the_field_once_with_the_identical_reference_sequence() {
    let (_temp, config, state, prepared) = fixture(512);
    snapshot::take_encoding_counts();
    prepare_snapshot(&state, MAX_METADATA).unwrap();
    let expected_serializations = snapshot::take_encoding_counts()["file_notes"];
    let (next, hot) = heat(&state, &prepared);
    for hash in prepared.descriptor.pages["file_notes"].iter().skip(1) {
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
        hot.descriptor.pages["file_notes"],
        prepared.descriptor.pages["file_notes"]
    );
    for disk in &config.disks {
        assert_eq!(load_snapshot(disk, &hot.hash).unwrap(), next);
    }
}

#[test]
fn field_reconstruction_cannot_substitute_different_bytes_or_a_different_page_count() {
    let (_temp, config, state, prepared) = fixture(256);
    let (mut wrong, hot) = heat(&state, &prepared);
    for hash in &prepared.descriptor.pages["file_notes"] {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    wrong.file_notes.clear();
    assert_eq!(
        hot.store(&config.disks, &wrong).unwrap_err().code(),
        "OBJECT_CORRUPT"
    );
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    wrong.file_notes = state.file_notes.clone();
    wrong
        .file_notes
        .insert("file-00000000".into(), "different".into());
    assert_eq!(
        hot.store(&config.disks, &wrong).unwrap_err().code(),
        "OBJECT_CORRUPT"
    );
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    let correct = next(&state);
    hot.store(&config.disks, &correct).unwrap();
    assert_eq!(load_snapshot(&config.disks[0], &hot.hash).unwrap(), correct);
}

#[test]
fn corrupt_large_targets_and_unsafe_targets_are_not_overwritten_or_buffered_to_global_limit() {
    let (temp, config, state, prepared) = fixture(300);
    let (next, hot) = heat(&state, &prepared);
    let hash = &prepared.descriptor.pages["file_notes"][0];
    let path = object_path(&config.disks[0], "state-page", hash);
    let size = prepared.fields["file_notes"].max_page_bytes + 1024 * 1024;
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size as u64)
        .unwrap();
    REUSED_PAGE_PEAK_BYTES.with(|peak| peak.set(0));
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        "OBJECT_COLLISION"
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), size as u64);
    assert!(
        REUSED_PAGE_PEAK_BYTES.with(|peak| peak.get())
            <= prepared.fields["file_notes"].max_page_bytes
    );
    let outside = temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert_eq!(
        hot.store(&config.disks, &next).unwrap_err().code(),
        "UNSAFE_PATH"
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
        .store_with(&config.disks, &next, |disk, hash, bytes| {
            object_storage::write_object_bytes_with(disk, "state-page", hash, bytes, |_, _| {
                barriers += 1;
                Err(Error::new(
                    "INJECTED_SYNC_FAILURE",
                    "parent sync failed after bytes became visible",
                ))
            })
        })
        .unwrap_err();
    assert_eq!(error.code(), "INJECTED_SYNC_FAILURE");
    assert_eq!(barriers, 1);
    assert!(!object_path(&config.disks[0], "snapshot", &hot.hash).exists());
    hot.store_with(&config.disks, &next, |disk, hash, bytes| {
        barriers += 1;
        write_object_bytes(disk, "state-page", hash, bytes)
    })
    .unwrap();
    assert!(barriers > 1);
    assert_eq!(load_snapshot(&config.disks[0], &hot.hash).unwrap(), next);
}

#[test]
fn reused_page_maxima_are_admitted_before_reading_sources_or_reencoding_records() {
    let (_temp, config, state, prepared) = fixture(1024);
    let basis = prepared.reuse_basis().unwrap();
    let maximum = basis.page_maxima["file_notes"];
    assert!(maximum > prepared.objects.last().unwrap().bytes.len());
    for hash in &prepared.descriptor.pages["file_notes"] {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    snapshot::take_encoding_counts();
    let error = prepare_snapshot_incremental(&state, &next(&state), &basis, maximum - 1)
        .err()
        .unwrap();
    assert_eq!(error.code(), "RESOURCE_LIMIT");
    assert_eq!(snapshot::take_encoding_counts().get("file_notes"), None);
    assert!(prepare_snapshot_incremental(&state, &next(&state), &basis, maximum).is_ok());
}

#[test]
fn baseline_memory_limit_disables_only_reuse_not_the_current_snapshot() {
    let state = State {
        control_auth: Some(json_codec::Value::String(
            "x".repeat(MAX_REUSE_DESCRIPTOR_BYTES),
        )),
        ..Default::default()
    };
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert!(prepared.reuse_basis().is_none());
    assert_eq!(
        prepared.hash,
        prepare_snapshot_reference(&state, MAX_METADATA)
            .unwrap()
            .hash
    );
}

#[test]
fn middle_or_last_missing_page_keeps_ordered_visitation_exactly_once() {
    for last in [false, true] {
        let (_temp, config, state, prepared) = fixture(512);
        let (next, hot) = heat(&state, &prepared);
        let expected = &hot.descriptor.pages["file_notes"];
        assert!(expected.len() > 2);
        let missing = if last {
            expected.len() - 1
        } else {
            expected.len() / 2
        };
        for disk in &config.disks {
            fs::remove_file(object_path(disk, "state-page", &expected[missing])).unwrap();
        }
        let mut visited = Vec::new();
        hot.visit_field_pages("file_notes", &config.disks, &next, |hash, _| {
            visited.push(hash.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            &visited, expected,
            "one callback per logical page, including successful prefixes"
        );
        hot.store(&config.disks, &next).unwrap();
        assert_eq!(load_snapshot(&config.disks[0], &hot.hash).unwrap(), next);
    }
}
