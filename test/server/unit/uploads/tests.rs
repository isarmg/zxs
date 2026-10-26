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
    let guard = StagedFile::open_budgeted(path.clone(), false, reservation).unwrap();
    let running_job = guard.clone();
    drop(guard);
    assert!(budget.reserve("alice", "unpublished.data", 10).is_err());
    running_job.write(b"partial").unwrap();
    drop(running_job);
    assert_eq!(fs::read(path).unwrap(), b"partial");
    assert!(budget.reserve("alice", "unpublished.data", 10).is_ok());
}

#[test]
fn failed_atomic_staged_progress_retains_bytes_and_blocks_admission_until_inventory() {
    let root = private_tempdir();
    let previous = root.path().join("previous.data");
    fs::write(&previous, b"previous").unwrap();
    let budget = test_budget(root.path());
    let path = root.path().join("new.data");
    let reservation = budget.reserve("alice", "new.data", 9).unwrap();
    let guard = StagedFile::open_budgeted(path.clone(), false, reservation).unwrap();
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
        let staged =
            StagedFile::open_budgeted(spool.join("upload.data"), false, reservation).unwrap();
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
