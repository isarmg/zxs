use super::*;
use std::collections::BTreeSet;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("scrub fixture").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    (temp, engine, repository)
}

fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) -> String {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "scrub fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine.list_tree(&repository.main_branch_id).unwrap()[0]
        .version_id
        .clone()
}

fn scrub(engine: &mut Engine) -> IntegrityScrubReport {
    engine.scrub(&IntegrityScrubCancellation::new()).unwrap()
}

fn current_candidate(engine: &Engine) -> Candidate {
    let member = &engine.members()[0];
    let record = scan(member)
        .unwrap()
        .records
        .into_iter()
        .rev()
        .find(|(record, _)| record.record_type == RecordType::Publication)
        .unwrap()
        .0;
    load_object(member, "candidate", &record.candidate_hash).unwrap()
}

fn page(engine: &Engine, field: &str) -> String {
    let snapshot: Snapshot = load_object(
        &engine.members()[0],
        "snapshot",
        engine.snapshot.as_ref().unwrap(),
    )
    .unwrap();
    snapshot.pages[field][0].clone()
}

fn payload_path(engine: &Engine, member: &DiskConfig, version: &str) -> std::path::PathBuf {
    let version = &engine.state.versions[version];
    crate::container_storage::content_path(
        &version_dir(member, &version.history_tree_id, &version.version_id),
        version,
    )
    .unwrap()
}

#[test]
fn healthy_scrub_reads_unique_retained_metadata_and_all_required_copies_without_writes() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"complete independent replicas");
    let state_before = canonical_json(engine.state()).unwrap();
    let snapshot_before = engine.snapshot.clone();
    let mut expected_objects = 0;
    let mut expected_publications = 0;
    let mut journals = Vec::new();
    for member in engine.members() {
        journals.push((log_path(&member), fs::read(log_path(&member)).unwrap()));
        let mut addresses = BTreeSet::new();
        for (record, _) in scan(&member).unwrap().records {
            if record.record_type != RecordType::Publication {
                continue;
            }
            expected_publications += 1;
            addresses.insert(("candidate", record.candidate_hash.clone()));
            addresses.insert(("publication", record.object_hash.clone()));
            let candidate: Candidate =
                load_object(&member, "candidate", &record.candidate_hash).unwrap();
            let publication: Publication =
                load_object(&member, "publication", &record.object_hash).unwrap();
            addresses.insert((
                "finalize",
                object_hash("finalize", &publication.finalize).unwrap(),
            ));
            addresses.insert(("snapshot", candidate.snapshot_hash.clone()));
            let snapshot: Snapshot =
                load_object(&member, "snapshot", &candidate.snapshot_hash).unwrap();
            for hash in snapshot.pages.values().flatten() {
                addresses.insert(("state-page", hash.clone()));
            }
            for object in crate::history_package::bound_history_objects(
                std::slice::from_ref(&member),
                &candidate,
                None,
            )
            .unwrap()
            {
                addresses.insert((object.kind(), object.hash().to_owned()));
            }
        }
        expected_objects += addresses.len();
    }
    let report = scrub(&mut engine);
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.metadata_members_checked, 3);
    assert_eq!(report.metadata_publications_verified, expected_publications);
    assert_eq!(report.metadata_objects_verified, expected_objects);
    assert_eq!(report.payload_copies_required, 3);
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(report.sequence, engine.state.sequence);
    assert_eq!(report.snapshot_hash, snapshot_before);
    assert_eq!(canonical_json(engine.state()).unwrap(), state_before);
    assert_eq!(engine.snapshot, snapshot_before);
    for (path, bytes) in journals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn state_only_metadata_corruption_is_not_hidden_by_healthy_version_inventory() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"payload");
    let hash = page(&engine, "operation_principals");
    let path = object_path(&engine.members()[0], "state-page", &hash);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] = b'!';
    fs::write(&path, &bytes).unwrap();
    assert!(engine.check().unwrap().issues.is_empty());
    let report = scrub(&mut engine);
    assert!(!report.is_healthy());
    assert_eq!(report.metadata_failures, 1);
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(report.payload_copies_unavailable, 0);
    assert_eq!(fs::read(path).unwrap(), bytes, "scrub must never repair");
}

#[test]
fn retained_historical_page_missing_is_found_even_when_current_snapshot_is_complete() {
    let (_temp, mut engine, repository) = fixture();
    let version = save(&mut engine, &repository, b"retained history");
    let file = engine.state.versions[&version].file_id.clone();
    engine
        .set_file_note(
            &file,
            None,
            &Note::parse("old note".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let old_page = page(&engine, "file_notes");
    engine
        .set_file_note(
            &file,
            None,
            &Note::parse("new note".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert_ne!(old_page, page(&engine, "file_notes"));
    let member = engine.members()[0].clone();
    let path = object_path(&member, "state-page", &old_page);
    fs::remove_file(&path).unwrap();
    crate::publication_evidence::verified_snapshot(
        std::slice::from_ref(&member),
        engine.snapshot.as_ref().unwrap(),
    )
    .unwrap();
    assert!(engine.check().unwrap().issues.is_empty());
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
    assert!(!path.exists());
}

#[test]
fn absent_history_root_is_reported_without_reencoding_from_state() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"history closure");
    let candidate = current_candidate(&engine);
    let hash = &candidate.history_manifest_hashes[&repository.history_tree_id];
    let path = object_path(&engine.members()[0], "history-manifest", hash);
    fs::remove_file(&path).unwrap();
    assert!(engine.check().unwrap().issues.is_empty());
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert!(!path.exists());
}

#[test]
fn missing_all_page_replicas_never_uses_state_or_sqlite_to_guess_repair() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"readable payload");
    let hash = page(&engine, "operation_principals");
    let paths: Vec<_> = engine
        .members()
        .iter()
        .map(|member| object_path(member, "state-page", &hash))
        .collect();
    for path in &paths {
        fs::remove_file(path).unwrap();
    }
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 3, "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn equal_length_payload_damage_is_separate_from_metadata_integrity() {
    let (_temp, mut engine, repository) = fixture();
    let version = save(&mut engine, &repository, b"good bytes");
    let path = payload_path(&engine, &engine.members()[0], &version);
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    fs::write(&path, b"evil bytes").unwrap();
    fs::set_permissions(&path, original_permissions).unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 0, "{report:?}");
    assert_eq!(report.payload_copies_required, 3);
    assert_eq!(report.payload_copies_verified, 2);
    assert_eq!(report.payload_copies_unavailable, 1);
    assert_eq!(report.payload_copies_unverified, 0);
    assert!(!report.is_healthy());
    assert_eq!(fs::read(path).unwrap(), b"evil bytes");
}

#[test]
fn missing_local_proof_is_metadata_failure_not_a_claim_about_payload_bytes() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"untouched bytes");
    let version = &engine.state.versions[&id];
    let directory = version_dir(&engine.members()[0], &version.history_tree_id, &id);
    fs::remove_file(directory.join("publication-proof.json")).unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.payload_copies_unverified, 1);
    assert_eq!(report.payload_copies_unavailable, 0);
    assert_eq!(report.payload_copies_verified, 2);
}

#[test]
fn pending_branch_missing_source_stays_selectable_after_payload_only_scrub_failure() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"source later missing");
    let file = engine.state.versions[&id].file_id.clone();
    let pending = engine
        .create_file_branch(
            &file,
            &id,
            &BranchName::parse("pending").unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    for member in engine.members() {
        fs::remove_file(payload_path(&engine, &member, &id)).unwrap();
    }
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 0, "{report:?}");
    assert_eq!(report.payload_copies_unavailable, 3);
    assert_eq!(
        report.payload_copies_required, 3,
        "pending branch has no invented own version"
    );
    engine
        .select_file_branch(&file, &pending, "admin", OperationId::generate())
        .unwrap();
}

#[test]
fn purged_payloads_require_no_body_but_present_tombstones_must_authenticate() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"old");
    save(&mut engine, &repository, b"new");
    let purge = engine
        .create_purge_plan(&repository.history_tree_id, vec![old.clone()], "admin")
        .unwrap();
    engine
        .commit_plan(&purge.plan_id, "admin", OperationId::generate())
        .unwrap();
    let report = scrub(&mut engine);
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.payload_copies_required, 3);
    assert_eq!(report.tombstones_verified, 3);
    let version = &engine.state.versions[&old];
    let path =
        version_dir(&engine.members()[0], &version.history_tree_id, &old).join("purged.json");
    fs::write(path, b"not a tombstone").unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1);
    assert_eq!(report.payload_copies_unavailable, 0);
}

#[test]
fn pre_cancelled_scrub_has_an_explicit_partial_result_and_performs_no_disk_probe() {
    let (temp, mut engine, _) = fixture();
    fs::rename(&engine.config.disks[0].root, temp.path().join("offline")).unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    cancellation.cancel();
    let report = engine.scrub(&cancellation).unwrap();
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert!(!report.is_healthy());
    assert_eq!(report.identity_failures, 0);
    assert_eq!(report.metadata_objects_verified, 0);
}

#[test]
fn detached_preparation_is_rejected_before_scan_and_releases_admission_on_drop() {
    let (_temp, mut engine, repository) = fixture();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"prepared")
        .unwrap();
    let preparation = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "prepare".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let error = engine
        .scrub(&IntegrityScrubCancellation::new())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CommitPreparationBusy);
    drop(preparation);
    assert!(scrub(&mut engine).is_healthy());
}

#[test]
fn actual_identity_replacement_cannot_pass_the_end_of_scan_identity_boundary() {
    let (temp, engine, _) = fixture();
    let disk = &engine.config.disks[0];
    let before = validate_seal(&engine.config, disk).unwrap();
    fs::rename(&disk.root, temp.path().join("old-member")).unwrap();
    private_dir(&disk.root).unwrap();
    let replacement = identity(&engine.config, disk).unwrap();
    write_json(&disk.root.join("disk.json"), &replacement, false).unwrap();
    let after = validate_seal(&engine.config, disk).unwrap();
    assert_eq!(
        stable_identity(&before, &after).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
}

#[test]
fn payload_cancellation_is_observed_between_real_verified_chunks() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, &vec![7; 2 * 1024 * 1024]);
    let version = &engine.state.versions[&id];
    let directory = version_dir(&engine.members()[0], &version.history_tree_id, &id);
    let cancellation = IntegrityScrubCancellation::new();
    let mut checkpoints = 0;
    let error = verify_payload_held_with(&directory, version, || {
        checkpoints += 1;
        if checkpoints == 3 {
            cancellation.cancel();
        }
        cancellation.checkpoint()
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert_eq!(checkpoints, 3);
    assert!(
        scrub(&mut engine).is_healthy(),
        "cancelled verification does not change the pool"
    );
}

#[test]
fn issue_storage_is_bounded_without_discarding_failure_counts() {
    let (_temp, mut engine, _) = fixture();
    let mut report = scrub(&mut engine);
    for _ in 0..MAX_RETAINED_ISSUES + 2 {
        report.issue(
            IntegrityScrubArea::Metadata,
            IntegrityScrubTarget::Disk {
                slot_id: "P01".into(),
            },
            Error::new(ErrorCode::ObjectCorrupt, "test cause"),
        );
    }
    assert_eq!(report.issues.len(), MAX_RETAINED_ISSUES);
    assert_eq!(report.issues_omitted, 2);
    assert_eq!(report.metadata_failures, MAX_RETAINED_ISSUES + 2);
    assert!(!report.is_healthy());
}

#[test]
fn admitted_sparse_available_content_state_is_a_required_copy_not_a_panic() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"sparse available");
    engine
        .mutate(
            "fixture.sparse",
            "admin",
            OperationId::generate(),
            json_codec::json!({}),
            |_, state, _| {
                state.content_states.remove(&id);
                Ok(json_codec::json!({}))
            },
        )
        .unwrap();
    let report = scrub(&mut engine);
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
}

#[test]
fn valid_truncated_journal_prefix_cannot_certify_the_current_snapshot() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"latest snapshot");
    let member = engine.members()[0].clone();
    let original = fs::read(log_path(&member)).unwrap();
    let mut reader = JournalFrameReader::new(std::io::Cursor::new(&original));
    let earlier_prefix = loop {
        let (record, _) = reader.next_record().unwrap().unwrap();
        if record.record_type == RecordType::Publication {
            assert!(record.sequence < engine.state.sequence);
            break reader.valid_bytes as usize;
        }
    };
    fs::write(log_path(&member), &original[..earlier_prefix]).unwrap();
    assert!(!scan(&member).unwrap().torn_tail);
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(
        fs::read(log_path(&member)).unwrap(),
        &original[..earlier_prefix]
    );
}

#[test]
fn a_new_self_consistent_journal_tail_is_not_an_admitted_writer_anchor() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"admitted state");
    let member = engine.members()[0].clone();
    append(
        &member,
        RecordType::Prepare,
        engine.state.sequence + 1,
        &"0".repeat(64),
        &"0".repeat(64),
    )
    .unwrap();
    assert!(!scan(&member).unwrap().torn_tail);
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.unpublished_journal_records, 1);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.source.code() == ErrorCode::LogCorrupt)
    );
    assert!(!report.is_healthy());
}

#[test]
fn journal_checksum_failure_is_observed_without_torn_tail_repair() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"admitted state");
    let member = engine.members()[0].clone();
    let mut bytes = fs::read(log_path(&member)).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(log_path(&member), &bytes).unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(fs::read(log_path(&member)).unwrap(), bytes);
}

#[test]
fn offline_member_is_explicitly_unverified_not_a_healthy_or_corrupt_payload() {
    let (temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"required copies");
    fs::rename(&engine.config.disks[0].root, temp.path().join("offline")).unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.identity_failures, 1, "{report:?}");
    assert_eq!(report.metadata_failures, 1);
    assert_eq!(report.payload_copies_required, 3);
    assert_eq!(report.payload_copies_unverified, 1);
    assert_eq!(report.payload_copies_unavailable, 0);
    assert_eq!(report.payload_copies_verified, 2);
    assert!(!report.is_healthy());
}

#[test]
fn dangling_symlink_ancestor_is_not_a_legitimately_absent_tombstoned_directory() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"descriptor");
    let version = &engine.state.versions[&id];
    let directory = version_dir(&engine.members()[0], &version.history_tree_id, &id);
    let versions = directory.parent().unwrap();
    let retained = versions.with_file_name("retained-versions");
    fs::rename(versions, &retained).unwrap();
    std::os::unix::fs::symlink("missing-versions", versions).unwrap();
    let error = verify_retained_tombstone(&directory, version, ContentState::Purged).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsafePath);
}

#[test]
fn genuinely_missing_tombstoned_history_directory_has_no_new_payload_requirement() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"descriptor");
    let version = &engine.state.versions[&id];
    let directory = version_dir(&engine.members()[0], &version.history_tree_id, &id);
    let versions = directory.parent().unwrap();
    fs::rename(versions, versions.with_file_name("retained-versions")).unwrap();
    assert!(!verify_retained_tombstone(&directory, version, ContentState::Purged).unwrap());
}
