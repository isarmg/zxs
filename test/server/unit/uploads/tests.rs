use super::*;
fn test_budget(root: &Path) -> std::sync::Arc<crate::spool_budget::SpoolBudget> {
    crate::spool_budget::SpoolBudget::recover(
        root,
        crate::SpoolLimits {
            bytes_per_account: 4 * crate::spool_budget::METADATA_ALLOWANCE,
            bytes_total: 4 * crate::spool_budget::METADATA_ALLOWANCE,
            recovery_reserve_bytes: 1,
        },
    )
    .unwrap()
}
#[test]
fn staging_guard_preserves_unpublished_bytes_until_and_after_last_job() {
    let root = private_tempdir();
    let budget = test_budget(root.path());
    let path = root.path().join("unpublished.data");
    let reservation = budget.reserve("alice", "unpublished.data", 10).unwrap();
    let guard = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &guard.directory,
        guard.reservation.directory()
    ));
    let running_job = guard.clone();
    drop(guard);
    assert!(budget.reserve("alice", "unpublished.data", 10).is_err());
    running_job.write(b"partial").unwrap();
    drop(running_job);
    assert_eq!(fs::read(path).unwrap(), b"partial");
    assert!(budget.reserve("alice", "unpublished.data", 10).is_ok());
}

#[test]
fn last_staged_owner_rejects_smaller_replacement_without_discarding_original_bytes() {
    let root = private_tempdir();
    let budget = test_budget(root.path());
    let reservation = budget.reserve("alice", "work.data", 1024).unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    let original = vec![42; 256];
    staged.write(&original).unwrap();
    staged.sync().unwrap();
    let last_job = std::sync::Arc::clone(&staged);
    drop(staged);
    let retained = root.path().join("retained.data");
    fs::rename(root.path().join("work.data"), &retained).unwrap();
    let replacement = EntryName::new("work.data").unwrap();
    last_job
        .directory
        .open_payload(&replacement, PayloadOpenMode::CreateNew)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    drop(last_job);
    // Capacity is ample: refusal must be inventory-required, not quota exhaustion.
    let error = budget.reserve("bob", "next.data", 0).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
    assert_eq!(fs::read(retained).unwrap(), original);
    assert_eq!(fs::read(root.path().join("work.data")).unwrap(), b"x");
    assert!(root.path().join("work.data.owner.json").is_file());
}

#[test]
fn last_staged_owner_during_writer_unwind_retains_bytes_without_a_second_panic() {
    let root = private_tempdir();
    let budget = test_budget(root.path());
    let reservation = budget.reserve("alice", "work.data", 100).unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    staged.write(b"retained").unwrap();
    staged.sync().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = staged;
        let _writer = owner.writer.lock().unwrap();
        // The writer guard poisons first; the final Arc then drops while unwinding.
        panic!("writer failed while holding its file lock");
    }));
    assert!(panic.is_err());
    assert_eq!(
        fs::read(root.path().join("work.data")).unwrap(),
        b"retained"
    );
    assert!(root.path().join("work.data.owner.json").is_file());
    let error = budget.reserve("bob", "next.data", 0).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
}

#[test]
fn generation_retention_rejects_rebound_root_and_preserves_prior_record() {
    let root = private_tempdir();
    let path = root.path().join("spool");
    PrivateDirectory::create(&path).unwrap();
    let budget = test_budget(&path);
    let reservation = budget.reserve("alice", "work.data", 3).unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    staged.write(b"old").unwrap();
    staged.sync().unwrap();
    let mut record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "alice".into(),
        path: "file".into(),
        generation: 1,
        logical_size: 3,
        content_hash: Some(storage_contract::ContentHash::from_digest(blake3::hash(
            b"old",
        ))),
        persisted_bytes: 3,
        persisted_hash: blake3::hash(b"old").to_hex().to_string(),
        state: UploadState::Complete,
        data_file: "work.data".into(),
    };
    staged.retain_generation(&record).unwrap();
    let name = format!(
        "retained-{}-1-{}.json",
        key("ws", "item"),
        blake3::hash(b"work.data").to_hex()
    );
    let previous = fs::read(path.join(&name)).unwrap();
    let moved = root.path().join("retained");
    fs::rename(&path, &moved).unwrap();
    PrivateDirectory::create(&path).unwrap();
    record.generation = 2;
    let error = staged.retain_generation(&record).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert!(budget.reserve("bob", "next.data", 0).is_err());
    drop(staged);
    assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 4);
    assert_eq!(fs::read(moved.join(name)).unwrap(), previous);
    assert_eq!(fs::read(moved.join("work.data")).unwrap(), b"old");
}

#[test]
fn staged_payload_modes_preserve_retry_bytes_and_open_error_sources() {
    use crate::http_boundary::ApiErrorSource;
    use platform_io::fs_safety::Error;

    let root = private_tempdir();
    let budget = test_budget(root.path());
    let path = root.path().join("retry.data");
    let reservation = budget.reserve("alice", "retry.data", 6).unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    staged.write(b"abc").unwrap();
    staged.sync().unwrap();
    drop(staged);

    let reservation = budget.reserve("alice", "retry.data", 6).unwrap();
    let error = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation)
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
    assert!(matches!(
        error.source.as_deref(),
        Some(ApiErrorSource::Filesystem(Error::Io(source)))
            if source.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(fs::read(&path).unwrap(), b"abc");

    let reservation = budget.reserve("alice", "retry.data", 6).unwrap();
    let retry = StagedFile::open_budgeted(PayloadOpenMode::Existing, reservation).unwrap();
    retry.write(b"def").unwrap();
    retry.sync().unwrap();
    drop(retry);
    assert_eq!(fs::read(&path).unwrap(), b"abcdef");

    let missing = root.path().join("missing.data");
    let reservation = budget.reserve("alice", "missing.data", 6).unwrap();
    let error = StagedFile::open_budgeted(PayloadOpenMode::Existing, reservation)
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
    assert!(matches!(
        error.source.as_deref(),
        Some(ApiErrorSource::Filesystem(Error::Io(source)))
            if source.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(!missing.exists());
    assert!(root.path().join("missing.data.owner.json").is_file());
}

#[test]
fn failed_atomic_staged_progress_retains_bytes_and_blocks_admission_until_inventory() {
    let root = private_tempdir();
    let previous = root.path().join("previous.data");
    fs::write(&previous, b"previous").unwrap();
    let budget = test_budget(root.path());
    let path = root.path().join("new.data");
    let reservation = budget.reserve("alice", "new.data", 9).unwrap();
    let guard = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    guard.write(b"new bytes").unwrap();
    guard.sync().unwrap();
    let record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "alice".into(),
        path: "file".into(),
        generation: 1,
        logical_size: 9,
        content_hash: Some(storage_contract::ContentHash::from_digest(blake3::hash(
            b"new bytes",
        ))),
        persisted_bytes: 9,
        persisted_hash: blake3::hash(b"new bytes").to_hex().to_string(),
        state: crate::uploads::UploadState::Incomplete,
        data_file: "new.data".into(),
    };
    // Reject a real unsafe destination at the AtomicFile boundary. This tests
    // error propagation/admission, not a simulated or physical fsync failure.
    let metadata = root.path().join(format!("{}.json", key("ws", "item")));
    fs::create_dir(&metadata).unwrap();
    assert!(persist_staged(&record, &guard).is_err());
    assert!(metadata.is_dir());
    fs::remove_dir(&metadata).unwrap();
    // Retrying the same held staging file publishes usable progress, but does
    // not silently clear the inventory-required admission state.
    persist_staged(&record, &guard).unwrap();
    let loaded = load(root.path(), "ws", "item").unwrap().unwrap();
    assert_eq!(loaded.data_file, "new.data");
    assert_eq!(loaded.persisted_bytes, 9);
    assert_eq!(loaded.persisted_hash, record.persisted_hash);
    let mut prefix = ExistingPrefix::open(root.path(), &loaded).unwrap().unwrap();
    prefix.check(b"new bytes").unwrap();
    assert!(prefix.complete());
    drop(guard);
    assert_eq!(fs::read(&path).unwrap(), b"new bytes");
    assert_eq!(fs::read(&previous).unwrap(), b"previous");
    assert!(budget.reserve("bob", "next.data", 1).is_err());
    drop(budget);
    let budget = test_budget(root.path());
    assert!(budget.reserve("alice", "new.data", 9).is_ok());
    assert_eq!(fs::read(&path).unwrap(), b"new bytes");
    assert_eq!(fs::read(&previous).unwrap(), b"previous");
}

#[test]
fn interrupted_upload_retains_and_checks_its_persisted_prefix() {
    let dir = private_tempdir();
    fs::write(dir.path().join("data"), b"original partial").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir.path().join("data"), fs::Permissions::from_mode(0o600)).unwrap();
    let record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "alice".into(),
        path: "file".into(),
        generation: 1,
        logical_size: 30,
        content_hash: Some("0".repeat(64).parse().unwrap()),
        persisted_bytes: 16,
        persisted_hash: blake3::hash(b"original partial").to_hex().to_string(),
        state: crate::uploads::UploadState::Incomplete,
        data_file: "data".into(),
    };
    persist(dir.path(), &record).unwrap();
    let loaded = load(dir.path(), "ws", "item").unwrap().unwrap();
    assert_eq!(loaded.persisted_bytes, 16);
    let mut prefix = ExistingPrefix::open(dir.path(), &loaded).unwrap().unwrap();
    assert!(prefix.check(b"different partial").is_err());
    let mut prefix = ExistingPrefix::open(dir.path(), &loaded).unwrap().unwrap();
    prefix.check(b"original partial and more").unwrap();
    assert!(prefix.complete());
    assert_eq!(status(&loaded)["state"], "RECEIVING");
}

#[test]
fn corrupt_or_oversized_progress_is_an_internal_state_error() {
    let dir = private_tempdir();
    let path = dir.path().join(format!("{}.json", key("ws", "item")));
    for bytes in [
        b"{broken".to_vec(),
        vec![b' '; 16_385],
        vec![b' '; 1024 * 1024],
    ] {
        fs::write(&path, bytes).unwrap();
        let error = load(dir.path(), "ws", "item").err().unwrap();
        assert_eq!(error.status, http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
    }
}

#[test]
fn failed_atomic_progress_publication_cleans_up_its_temporary_file() {
    let dir = private_tempdir();
    let record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "alice".into(),
        path: "file".into(),
        generation: 1,
        logical_size: 0,
        content_hash: Some(storage_contract::ContentHash::from_digest(blake3::hash(
            b"",
        ))),
        persisted_bytes: 0,
        persisted_hash: blake3::hash(b"").to_hex().to_string(),
        state: crate::uploads::UploadState::Incomplete,
        data_file: "data".into(),
    };
    // A directory at the destination makes rename fail deterministically,
    // even when the test runs as root and chmod cannot deny writes.
    fs::create_dir(dir.path().join(format!("{}.json", key("ws", "item")))).unwrap();
    assert!(persist(dir.path(), &record).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn replaced_http_payload_or_parent_cannot_be_submitted() {
    use std::os::unix::fs::PermissionsExt;
    for swap_parent in [false, true] {
        let root = private_tempdir();
        let spool = root.path().join("spool");
        fs::create_dir(&spool).unwrap();
        fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
        let budget = test_budget(&spool);
        let reservation = budget.reserve("alice", "upload.data", 8).unwrap();
        let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
        staged.write(b"original").unwrap();
        if swap_parent {
            fs::rename(&spool, root.path().join("moved")).unwrap();
            fs::create_dir(&spool).unwrap();
            fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
        } else {
            fs::rename(spool.join("upload.data"), spool.join("moved.data")).unwrap();
        }
        fs::write(spool.join("upload.data"), b"attacker").unwrap();
        fs::set_permissions(spool.join("upload.data"), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            staged
                .source(
                    8,
                    storage_contract::ContentHash::from_digest(blake3::hash(b"original"))
                )
                .is_err()
        );
    }
}

#[test]
fn upload_progress_and_retry_prefix_reject_links_without_reading_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = private_tempdir();
    let outside = private_tempdir();
    let record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "alice".into(),
        path: "file".into(),
        generation: 1,
        logical_size: 8,
        content_hash: Some(storage_contract::ContentHash::from_digest(blake3::hash(
            b"original",
        ))),
        persisted_bytes: 8,
        persisted_hash: blake3::hash(b"original").to_hex().to_string(),
        state: crate::uploads::UploadState::Incomplete,
        data_file: "data".into(),
    };
    persist(outside.path(), &record).unwrap();
    fs::write(outside.path().join("data"), b"original").unwrap();
    fs::set_permissions(
        outside.path().join("data"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let progress_name = format!("{}.json", key("ws", "item"));
    for hard_link in [false, true] {
        for name in [&progress_name, "data"] {
            if hard_link {
                fs::hard_link(outside.path().join(name), dir.path().join(name)).unwrap();
            } else {
                symlink(outside.path().join(name), dir.path().join(name)).unwrap();
            }
        }
        assert_eq!(
            load(dir.path(), "ws", "item").err().unwrap().status,
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ExistingPrefix::open(dir.path(), &record)
                .err()
                .unwrap()
                .status,
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
        fs::remove_file(dir.path().join(&progress_name)).unwrap();
        fs::remove_file(dir.path().join("data")).unwrap();
    }
    assert_eq!(fs::read(outside.path().join("data")).unwrap(), b"original");
    assert!(load(dir.path(), "ws", "item").unwrap().is_none());
}

fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[test]
fn upload_state_accepts_only_known_string_states() {
    for state in ["incomplete", "checksum_mismatch", "complete"] {
        let decoded: UploadState = json_codec::from_value(json_codec::json!(state)).unwrap();
        assert_eq!(json_codec::to_value(decoded).unwrap(), state);
    }
    for invalid in [
        json_codec::json!("unknown"),
        json_codec::json!({"complete": null}),
        json_codec::json!(null),
    ] {
        assert!(json_codec::from_value::<UploadState>(invalid).is_err());
    }
}

#[test]
fn upload_record_rejects_noncanonical_body_hash_and_retains_progress_evidence() {
    let dir = private_tempdir();
    let path = dir.path().join(format!("{}.json", key("ws", "item")));
    let data = dir.path().join("data");
    fs::write(&data, b"partial").unwrap();
    for hash in ["".to_string(), "0".repeat(63), "A".repeat(64)] {
        let bytes = storage_contract::canonical_json(&json_codec::json!({
            "workspace_id":"ws", "item_id":"item", "principal":"alice", "path":"file",
            "generation":"1", "logical_size":"9", "content_hash":hash,
            "persisted_bytes":"7", "persisted_hash":blake3::hash(b"partial").to_hex().to_string(),
            "state":"incomplete", "data_file":"data"
        }))
        .unwrap();
        fs::write(&path, &bytes).unwrap();
        let error = match load(dir.path(), "ws", "item") {
            Err(error) => error,
            Ok(_) => panic!("invalid hash accepted"),
        };
        assert_eq!(error.code, storage_contract::ErrorCode::UploadStateInvalid);
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(&data).unwrap(), b"partial");
    }
}

#[test]
fn status_projects_each_durable_state_without_private_fields_or_payload_reads() {
    let mut record = UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: "private-owner".into(),
        path: "private-path".into(),
        generation: u64::MAX,
        logical_size: u64::MAX,
        content_hash: Some("0".repeat(64).parse().unwrap()),
        persisted_bytes: u64::MAX,
        persisted_hash: "1".repeat(64),
        state: UploadState::Incomplete,
        // No such file: projecting status must not read or hash payload data.
        data_file: "unavailable-private-file.data".into(),
    };
    for (state, wire) in [
        (UploadState::Incomplete, "RECEIVING"),
        (UploadState::ChecksumMismatch, "CHECKSUM_MISMATCH"),
        (UploadState::Complete, "COMPLETE"),
    ] {
        record.state = state;
        assert_eq!(
            status(&record),
            json_codec::json!({
                "workspace_id":"ws", "item_id":"item", "upload_generation":u64::MAX.to_string(),
                "logical_size":u64::MAX.to_string(), "content_hash":"0".repeat(64),
                "state":wire, "sealed_manifest_hash":null,
                "durable_ranges":[{"offset":"0", "length":u64::MAX.to_string(), "content_hash":"1".repeat(64)}]
            })
        );
    }
    record.persisted_bytes = 0;
    record.logical_size = 0;
    assert_eq!(status(&record)["durable_ranges"], json_codec::json!([]));
}
