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
    crate::journal::map_pages(&engine.members()[0], &snapshot.maps[field])[0].clone()
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
            addresses.extend(crate::journal::snapshot_addresses(&member, &snapshot));
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
    let catalog = crate::journal::candidate_catalog(&engine.members()[0], &candidate);
    let hash = &catalog[&repository.history_tree_id];
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
fn root_name_replacement_cannot_redirect_scrub_metadata_or_payload_reads() {
    let (temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"retained tombstone");
    save(&mut engine, &repository, b"held resources");
    let purge = engine
        .create_purge_plan(&repository.history_tree_id, vec![old], "admin")
        .unwrap();
    engine
        .commit_plan(&purge.plan_id, "admin", OperationId::generate())
        .unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    for disk in &engine.config.disks {
        fs::rename(
            &disk.root,
            temp.path().join(format!("old-{}", disk.slot_id)),
        )
        .unwrap();
        private_dir(&disk.root).unwrap();
        let replacement = identity(&engine.config, disk).unwrap();
        write_json(&disk.root.join("disk.json"), &replacement, false).unwrap();
    }
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.metadata_members_checked, 3);
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(report.tombstones_verified, 3);
    for disk in &engine.config.disks {
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
    }
}

#[test]
fn renamed_owner_payload_damage_is_not_masked_by_a_replacement_body() {
    let (temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"good bytes");
    let disk = &engine.config.disks[0];
    let path = payload_path(&engine, disk, &id);
    let relative = path.strip_prefix(&disk.root).unwrap();
    let retained = temp.path().join("retained-member");
    fs::rename(&disk.root, &retained).unwrap();
    private_dir(path.parent().unwrap()).unwrap();
    fs::write(&path, b"good bytes").unwrap();
    fs::write(retained.join(relative), b"evil bytes").unwrap();
    let report = scrub(&mut engine);
    assert_eq!(report.identity_failures, 0, "{report:?}");
    assert_eq!(report.metadata_failures, 0);
    assert_eq!(report.payload_copies_unavailable, 1);
    assert_eq!(report.payload_copies_verified, 2);
    assert!(!report.is_healthy());
}

#[test]
fn payload_cancellation_is_observed_between_bounded_reads() {
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
fn missing_held_seal_is_explicitly_unverified_not_a_healthy_or_corrupt_payload() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"required copies");
    fs::remove_file(engine.config.disks[0].root.join("disk.json")).unwrap();
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

#[test]
fn offlock_scan_allows_real_concurrent_publication_and_stops_at_captured_frontiers() {
    use std::sync::{Arc, Mutex, mpsc};
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, &vec![9; 160 * 1024]);
    let file = engine.state.versions[&id].file_id.clone();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let baseline = plan.sequence();
    let expected_frames: usize = engine
        .config
        .metadata_members()
        .map(|member| scan(member).unwrap().records.len())
        .sum();
    assert_eq!(plan.estimated_payload_bytes().unwrap(), 3 * 160 * 1024);
    let engine = Arc::new(Mutex::new(engine));
    let (paused, wait_pause) = mpsc::sync_channel(0);
    let (resume, wait_resume) = mpsc::sync_channel(0);
    let owner = Arc::clone(&engine);
    let handle = std::thread::spawn(move || {
        let mut first = true;
        let mut actual_bytes = 0usize;
        let mut maximum_burst = 0;
        let outcome = plan.run(
            &IntegrityScrubCancellation::new(),
            |plan, id| owner.lock().unwrap().admit_integrity_scrub_batch(plan, id),
            |bytes| {
                actual_bytes += bytes;
                maximum_burst = maximum_burst.max(bytes);
                if first {
                    first = false;
                    paused.send(()).unwrap();
                    wait_resume.recv().unwrap();
                }
            },
            |partial| assert!(!partial.is_healthy(), "partial observation is not final"),
        );
        let report = owner.lock().unwrap().finish_integrity_scrub(outcome);
        (report, actual_bytes, maximum_burst)
    });
    wait_pause.recv().unwrap();
    {
        let mut current = engine
            .try_lock()
            .expect("read callback does not retain Engine lock");
        current
            .set_file_note(
                &file,
                None,
                &Note::parse("concurrent".into()).unwrap(),
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        assert!(current.state.sequence > baseline);
    }
    resume.send(()).unwrap();
    let (report, actual_bytes, maximum_burst) = handle.join().unwrap();
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.sequence, baseline);
    assert_eq!(report.journal_frames_verified, expected_frames);
    assert!(
        actual_bytes > 3 * 160 * 1024,
        "metadata and proof bytes are counted too"
    );
    assert!(maximum_burst <= 64 * 1024);
}

#[test]
fn offlock_purge_before_batch_is_superseded_not_metadata_corruption() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"old body");
    save(&mut engine, &repository, b"current body");
    let purge = engine
        .create_purge_plan(&repository.history_tree_id, vec![old], "admin")
        .unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    engine
        .commit_plan(&purge.plan_id, "admin", OperationId::generate())
        .unwrap();
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Superseded
    ));
    assert_eq!(report.metadata_failures, 0, "{report:?}");
    assert_eq!(report.payload_copies_unavailable, 0);
    assert!(!report.is_healthy());
}

#[test]
fn offlock_active_batch_pins_real_purge_until_cancellation_drains_it() {
    use std::sync::Mutex;
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, &vec![4; 256 * 1024]);
    save(&mut engine, &repository, b"current body");
    let purge = engine
        .create_purge_plan(&repository.history_tree_id, vec![old.clone()], "admin")
        .unwrap();
    let old_path = payload_path(&engine, &engine.members()[0], &old);
    let plan = engine.prepare_integrity_scrub().unwrap();
    let engine = Mutex::new(engine);
    let cancellation = IntegrityScrubCancellation::new();
    let mut attempted = false;
    let outcome = plan.run(
        &cancellation,
        |plan, id| engine.lock().unwrap().admit_integrity_scrub_batch(plan, id),
        |_| {
            let mut current = engine
                .try_lock()
                .expect("all metered reads run outside Engine lock");
            let active = current.reads.lock().unwrap().contains_key(&old);
            if active && !attempted {
                attempted = true;
                let error = current
                    .commit_plan(&purge.plan_id, "admin", OperationId::generate())
                    .unwrap_err();
                assert_eq!(error.code(), ErrorCode::ContentPinned);
                assert!(old_path.exists());
                cancellation.cancel();
            }
        },
        |_| {},
    );
    assert!(attempted);
    let mut engine = engine.into_inner().unwrap();
    let report = engine.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert!(
        engine.reads.lock().unwrap().is_empty(),
        "cancel returns only after the batch drops"
    );
    engine
        .commit_plan(&purge.plan_id, "admin", OperationId::generate())
        .unwrap();
    assert!(
        !old_path.exists(),
        "actual published cleanup can run after the lease drains"
    );
}

#[test]
fn offlock_held_body_survives_actual_container_rename_during_proof_reads() {
    use std::sync::Mutex;
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"rename-safe held body");
    let old_path = payload_path(&engine, &engine.members()[0], &id);
    let plan = engine.prepare_integrity_scrub().unwrap();
    let engine = Mutex::new(engine);
    let mut renamed = false;
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.lock().unwrap().admit_integrity_scrub_batch(plan, id),
        |_| {
            let mut current = engine.try_lock().unwrap();
            if !renamed && current.reads.lock().unwrap().contains_key(&id) {
                current
                    .file_namespace(
                        &repository.repository_id,
                        NamespaceOperation::parse(
                            storage_contract::NamespaceOperationKind::Rename,
                            "file.txt",
                            Some("nested/renamed.txt"),
                        )
                        .unwrap(),
                        "admin",
                        OperationId::generate(),
                    )
                    .unwrap();
                renamed = true;
                assert!(!old_path.exists());
            }
        },
        |_| {},
    );
    let report = engine.into_inner().unwrap().finish_integrity_scrub(outcome);
    assert!(renamed);
    assert!(report.is_healthy(), "{report:?}");
    assert_eq!(report.payload_copies_verified, 3);
}

#[test]
fn offlock_first_observed_issue_is_published_before_remaining_members_are_read() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"body");
    let path = object_path(
        engine.config.metadata_members().next().unwrap(),
        "state-page",
        &page(&engine, "repositories"),
    );
    fs::remove_file(path).unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let mut first_failure = None;
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |partial| {
            if partial.metadata_failures != 0 && first_failure.is_none() {
                first_failure = Some((
                    partial.metadata_members_checked,
                    partial.payload_copies_verified,
                ));
                assert!(matches!(
                    partial.completion,
                    IntegrityScrubCompletion::Running
                ));
            }
        },
    );
    assert_eq!(first_failure, Some((1, 0)));
    assert!(!engine.finish_integrity_scrub(outcome).is_healthy());
}

#[test]
fn offlock_empty_plan_cannot_be_accepted_by_reopened_owner() {
    let (temp, engine, _) = fixture();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no versions"),
        |_| {},
        |_| {},
    );
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    let report = reopened.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Superseded
    ));
    assert!(!report.is_healthy());
}

#[test]
fn offlock_root_rename_after_verified_payloads_keeps_the_same_healthy_owner() {
    let (temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"body");
    let root = engine.config.disks[0].root.clone();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let mut removed = false;
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |partial| {
            if !removed && partial.payload_copies_verified == 3 {
                fs::rename(&root, temp.path().join("lost-after-read")).unwrap();
                removed = true;
            }
        },
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(removed);
    assert_eq!(report.payload_copies_verified, 3);
    assert!(report.is_healthy(), "{report:?}");
}

#[test]
fn held_seal_change_after_verified_payloads_revokes_scrub_coverage() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"body");
    let disk = engine.config.disks[0].clone();
    let mut changed = engine.disk_resources.get(&disk).unwrap().seal.clone();
    changed.binding_generation += 1;
    let epoch = engine.metadata_integrity.capture();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let mut replaced = false;
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |partial| {
            if !replaced && partial.payload_copies_verified == 3 {
                write_json(&disk.root.join("disk.json"), &changed, true).unwrap();
                replaced = true;
            }
        },
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(replaced);
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(report.identity_failures, 1);
    assert_eq!(report.metadata_failures, 1);
    assert!(!epoch.matches(&engine.metadata_integrity));
    assert!(!report.is_healthy());
}

#[test]
fn captured_scrub_resources_do_not_retain_writer_locks_or_admit_a_new_owner() {
    let (temp, engine, _) = fixture();
    let plan = engine.prepare_integrity_scrub().unwrap();
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no versions"),
        |_| {},
        |_| {},
    );
    let report = reopened.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Superseded
    ));
}

#[test]
fn observed_regular_file_reads_charge_actual_bytes_not_reserved_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("actual");
    fs::write(&path, vec![5; 130 * 1024 + 7]).unwrap();
    let mut counts = Vec::new();
    let bytes =
        read_file_bytes_observed(fs::File::open(path).unwrap(), 1024 * 1024, &mut |count| {
            if count != 0 {
                counts.push(count);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(counts.iter().sum::<usize>(), bytes.len());
    assert_eq!(counts, vec![64 * 1024, 64 * 1024, 2 * 1024 + 7]);
}

#[test]
fn offlock_callback_cannot_substitute_an_equal_body_from_a_different_version() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"same body");
    let other_repository = engine
        .create_repository(
            &RepositoryName::parse("other file").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let new = save(&mut engine, &other_repository, b"same body");
    assert_ne!(
        old, new,
        "same-byte writes on one file intentionally reuse its version"
    );
    fs::remove_file(payload_path(&engine, &engine.members()[0], &old)).unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, _| engine.admit_integrity_scrub_batch(plan, &new),
        |_| {},
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(
        matches!(report.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::PlanInvalid),
        "{report:?}"
    );
    assert!(!report.is_healthy());
    assert!(engine.reads.lock().unwrap().is_empty());
}

#[test]
fn cancellation_inside_actual_candidate_read_is_not_reported_as_metadata_damage() {
    let (_temp, engine, _) = fixture();
    let disk = engine.config.metadata_members().next().unwrap();
    let mut prefix = engine
        .config
        .disks
        .iter()
        .map(|disk| fs::metadata(disk.root.join("disk.json")).unwrap().len())
        .sum::<u64>();
    prefix += fs::metadata(disk.root.join("metadata/pool-membership/config.json"))
        .unwrap()
        .len();
    // This fixture has one published candidate. All its frames precede the
    // first candidate-object read, with no read-ahead in the scrub parser.
    prefix += fs::metadata(log_path(disk)).unwrap().len();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut actual = 0u64;
    let outcome = plan.run(
        &cancellation,
        |_, _| panic!("cancel before payload admission"),
        |bytes| {
            actual += bytes as u64;
            if actual > prefix {
                cancellation.cancel();
            }
        },
        |partial| {
            assert_eq!(
                partial.metadata_failures, 0,
                "cancellation is not observed damage"
            )
        },
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(actual > prefix);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert_eq!(report.metadata_failures, 0);
    assert!(report.issues.is_empty());
}

fn move_fixture_into_namespace(engine: &mut Engine, repository: &Repository) {
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "file.txt",
                Some("nested/file.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
}

#[test]
fn actual_namespace_marker_reads_are_charged_after_unlock_before_more_io() {
    use std::{cell::Cell, sync::Mutex};
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"namespace body");
    move_fixture_into_namespace(&mut engine, &repository);
    let version = &engine.state.versions[&id];
    let expected: usize = engine
        .members()
        .iter()
        .map(|disk| {
            let namespace = disk
                .root
                .join("histories")
                .join(&version.history_tree_id)
                .join("file-containers")
                .join(&version.file_id)
                .join("namespace");
            let body = payload_path(&engine, disk, &id);
            let owner = body
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(".zxs-container-owner.json");
            // Current container lookup authenticates the namespace marker on entry
            // and when enumerating its root, then the single selected owner's marker.
            2 * fs::metadata(namespace.join(".zxs-namespace.json"))
                .unwrap()
                .len() as usize
                + fs::metadata(owner).unwrap().len() as usize
        })
        .sum();
    assert!(expected > 0);
    let plan = engine.prepare_integrity_scrub().unwrap();
    let engine = Mutex::new(engine);
    let awaiting_charge = Cell::new(false);
    let charged = Cell::new(false);
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| {
            let batch = engine
                .lock()
                .unwrap()
                .admit_integrity_scrub_batch(plan, id)?;
            let IntegrityScrubBatchAdmission::Ready(ref ready) = batch else {
                panic!("unchanged fixture")
            };
            assert_eq!(ready.layout_bytes_read, expected);
            awaiting_charge.set(true);
            Ok(batch)
        },
        |bytes| {
            if awaiting_charge.replace(false) {
                assert_eq!(bytes, expected);
                assert!(
                    engine.try_lock().is_ok(),
                    "marker read debt is paid outside the Engine lock"
                );
                charged.set(true);
            }
        },
        |_| {},
    );
    assert!(charged.get());
    assert!(
        engine
            .into_inner()
            .unwrap()
            .finish_integrity_scrub(outcome)
            .is_healthy()
    );
}

#[test]
fn oversized_namespace_marker_is_bounded_and_failed_cause_survives_wrong_finish_owner() {
    use std::cell::Cell;
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"namespace body");
    move_fixture_into_namespace(&mut engine, &repository);
    let version = &engine.state.versions[&id];
    let marker = engine.config.disks[0]
        .root
        .join("histories")
        .join(&version.history_tree_id)
        .join("file-containers")
        .join(&version.file_id)
        .join("namespace/.zxs-namespace.json");
    fs::write(&marker, vec![b' '; 70 * 1024]).unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let awaiting_charge = Cell::new(false);
    let observed_failure = Cell::new(false);
    let mut charged = 0;
    let outcome = plan.run(&IntegrityScrubCancellation::new(), |plan, id| {
        let batch = engine.admit_integrity_scrub_batch(plan, id)?;
        awaiting_charge.set(true);
        Ok(batch)
    }, |bytes| {
        if awaiting_charge.replace(false) {
            assert!(observed_failure.get(), "budget failure is published before rate waiting");
            assert!(bytes < 128 * 1024);
            charged = bytes;
        }
    }, |partial| {
        if matches!(partial.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::ResourceLimit) {
            observed_failure.set(true);
        }
    });
    assert!(charged >= 64 * 1024);
    let (_other_temp, other_engine, _) = fixture();
    let report = other_engine.finish_integrity_scrub(outcome);
    assert!(
        matches!(report.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::ResourceLimit),
        "{report:?}"
    );
    assert_eq!(report.metadata_failures, 0);
    assert!(!report.is_healthy());
}

#[test]
fn admitted_missing_body_is_reported_before_any_further_pacer_or_proof_read() {
    use std::cell::Cell;
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"missing body");
    fs::remove_file(payload_path(&engine, &engine.members()[0], &id)).unwrap();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let admitted = Cell::new(false);
    let observed = Cell::new(false);
    let cancellation = IntegrityScrubCancellation::new();
    let outcome = plan.run(
        &cancellation,
        |plan, id| {
            let batch = engine.admit_integrity_scrub_batch(plan, id)?;
            admitted.set(true);
            Ok(batch)
        },
        |_| {
            if admitted.get() {
                assert!(
                    observed.get(),
                    "known body failure must precede further reads/rate waits"
                );
                cancellation.cancel();
            }
        },
        |partial| {
            if admitted.get() && partial.payload_copies_unavailable != 0 {
                observed.set(true);
            }
        },
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(observed.get());
    assert_eq!(report.payload_copies_unavailable, 1);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
}

#[test]
fn offlock_static_extra_tail_bytes_are_not_permanently_ignored() {
    use std::io::Write;
    let (_temp, engine, _) = fixture();
    let disk = engine.config.metadata_members().next().unwrap();
    let path = log_path(disk);
    let mut log = fs::OpenOptions::new().append(true).open(&path).unwrap();
    log.write_all(b"static unexplained bytes").unwrap();
    log.sync_all().unwrap();
    let expected = fs::read(&path).unwrap();
    let mut single_byte_reads = 0;
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no payload versions"),
        |bytes| {
            if bytes == 1 {
                single_byte_reads += 1;
            }
        },
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert_eq!(
        single_byte_reads, 1,
        "one real bounded probe, not chasing extra bytes"
    );
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.source.code() == ErrorCode::LogCorrupt)
    );
    assert!(!report.is_healthy());
    assert_eq!(
        fs::read(path).unwrap(),
        expected,
        "tail probe never repairs or truncates"
    );
}

#[test]
fn extra_tail_probe_accounts_the_read_before_observing_cancellation() {
    use std::io::Write;
    let (_temp, engine, _) = fixture();
    let disk = engine.config.metadata_members().next().unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(log_path(disk))
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut single_byte_reads = 0;
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &cancellation,
        |_, _| panic!("no payload versions"),
        |bytes| {
            if bytes == 1 {
                single_byte_reads += 1;
                cancellation.cancel();
            }
        },
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert_eq!(single_byte_reads, 1);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert_eq!(
        report.metadata_failures, 0,
        "an uncompleted frontier assessment is not invented corruption"
    );
    assert!(!report.is_healthy());
}

#[cfg(feature = "fault-injection")]
#[test]
fn finish_does_not_certify_a_writer_that_became_uncertain_after_probe() {
    let (_temp, mut engine, _) = fixture();
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no payload versions"),
        |_| {},
        |_| {},
    );
    engine
        .inject_journal_fault_once("P01", crate::JournalFault::PartialWrite)
        .unwrap();
    assert!(
        engine
            .create_repository(
                &RepositoryName::parse("failed append").unwrap(),
                "P01",
                3,
                "admin",
                OperationId::generate()
            )
            .is_err()
    );
    assert!(engine.journals.values().any(JournalWriter::needs_recovery));
    let report = engine.finish_integrity_scrub(outcome);
    assert!(
        matches!(report.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::RecoveryRequired),
        "{report:?}"
    );
    assert!(!report.is_healthy());
}

#[test]
fn empty_admitted_journal_frontier_still_probes_unexplained_bytes() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let run = || {
        engine.prepare_integrity_scrub().unwrap().run(
            &IntegrityScrubCancellation::new(),
            |_, _| panic!("empty pool"),
            |_| {},
            |_| {},
        )
    };
    assert!(engine.finish_integrity_scrub(run()).is_healthy());
    fs::OpenOptions::new()
        .append(true)
        .open(log_path(&engine.config.disks[0]))
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let report = engine.finish_integrity_scrub(run());
    assert_eq!(report.sequence, 0);
    assert!(report.snapshot_hash.is_none());
    assert_eq!(report.journal_frames_verified, 0);
    assert_eq!(report.metadata_failures, 1);
    assert!(!report.is_healthy());
}

#[test]
fn missing_writer_at_capture_is_a_member_issue_but_lost_captured_writer_is_incomplete() {
    let (_temp, mut engine, _) = fixture();
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no versions"),
        |_| {},
        |_| {},
    );
    let missing = engine.config.disks[0].disk_id.clone();
    engine.journals.remove(&missing).unwrap();
    let report = engine.finish_integrity_scrub(outcome);
    assert!(
        matches!(report.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::RecoveryRequired)
    );
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |_, _| panic!("no versions"),
        |_| {},
        |_| {},
    );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Complete
    ));
    assert_eq!(report.metadata_failures, 1);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.source.code() == ErrorCode::RecoveryRequired)
    );
    assert!(!report.is_healthy());
}

#[test]
fn batch_from_a_previous_engine_owner_is_not_a_lease_for_the_reopened_pool() {
    let (temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"same exact version");
    let old_plan = engine.prepare_integrity_scrub().unwrap();
    let mut old_batch = Some(engine.admit_integrity_scrub_batch(&old_plan, &id).unwrap());
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    let outcome = reopened.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |_, _| Ok(old_batch.take().unwrap()),
        |_| {},
        |_| {},
    );
    let report = reopened.finish_integrity_scrub(outcome);
    assert!(
        matches!(report.completion, IntegrityScrubCompletion::Failed(ref error) if error.code() == ErrorCode::PlanInvalid)
    );
    assert!(!report.is_healthy());
    assert!(
        old_plan.owner.lock().unwrap().is_empty(),
        "rejected old lease drains"
    );
}

#[test]
fn scrub_member_fault_revokes_the_writer_before_its_progress_callback() {
    let (_temp, mut engine, repository) = fixture();
    save(&mut engine, &repository, b"known graph fault");
    let epoch = engine.metadata_integrity.capture();
    let hash = page(&engine, "operation_principals");
    fs::remove_file(object_path(&engine.members()[0], "state-page", &hash)).unwrap();
    let mut observed = false;
    let outcome = engine.prepare_integrity_scrub().unwrap().run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |report| {
            if report.metadata_failures > 0 {
                observed = true;
                assert!(!epoch.matches(&engine.metadata_integrity));
            }
        },
    );
    assert!(observed);
    assert!(!engine.finish_integrity_scrub(outcome).is_healthy());
}

#[test]
fn payload_and_local_version_proof_failures_do_not_revoke_shared_graph_provenance() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"local availability only");
    let epoch = engine.metadata_integrity.capture();
    fs::remove_file(payload_path(&engine, &engine.members()[0], &id)).unwrap();
    let version = &engine.state.versions[&id];
    fs::remove_file(
        version_dir(&engine.members()[1], &version.history_tree_id, &id)
            .join("publication-proof.json"),
    )
    .unwrap();
    let report = scrub(&mut engine);
    assert!(!report.is_healthy());
    assert!(
        epoch.matches(&engine.metadata_integrity),
        "local version availability is not a shared metadata graph fault"
    );
}

#[test]
fn old_read_plan_member_identity_failure_revokes_the_current_writer_after_fallback() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"old plan healthy fallback");
    let plan = engine.prepare_content(&id).unwrap();
    engine
        .persist_auth_state(
            json_codec::json!({"auth":"new publication"}),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let epoch = engine.metadata_integrity.capture();
    fs::remove_file(engine.members()[0].root.join("disk.json")).unwrap();
    let opened = plan.open().unwrap();
    assert!(
        !opened.fallback_files.is_empty(),
        "another independently verified copy remains usable"
    );
    assert!(
        !epoch.matches(&engine.metadata_integrity),
        "an old plan must revoke the current writer, not just its old baseline"
    );
}

#[test]
fn old_read_plan_shared_membership_failure_revokes_current_provenance_on_healthy_fallback() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"shared membership boundary");
    let plan = engine.prepare_content(&id).unwrap();
    engine
        .persist_auth_state(
            json_codec::json!({"auth":"newer"}),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let epoch = engine.metadata_integrity.capture();
    fs::remove_file(
        engine.members()[0]
            .root
            .join("metadata/pool-membership/config.json"),
    )
    .unwrap();
    assert!(
        plan.open().is_ok(),
        "other members still supply independently verified copies"
    );
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn saved_membership_is_compared_to_the_live_binding_without_reclassifying_local_proof_errors() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"binding context");
    let path = engine.members()[0]
        .root
        .join("metadata/pool-membership/config.json");
    let mut saved: Config = read_json(&path).unwrap();
    saved.disks[0].binding_generation += 1;
    write_json(&path, &saved, true).unwrap();
    let epoch = engine.metadata_integrity.capture();
    assert!(engine.prepare_content(&id).unwrap().open().is_ok());
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn completed_graph_coverage_is_separate_from_unavailable_local_payload_health() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"local payload later missing");
    fs::remove_file(payload_path(&engine, &engine.members()[0], &id)).unwrap();
    let owner = engine
        .begin_integrity_scrub_schedule(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(120),
        )
        .unwrap();
    let outcome = engine
        .prepare_scheduled_integrity_scrub(&owner)
        .unwrap()
        .run(
            &IntegrityScrubCancellation::new(),
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(!report.is_healthy());
    assert_eq!(report.payload_copies_unavailable, 1);
    assert!(
        engine.has_current_integrity_scrub_coverage(),
        "the full shared graph was checked even though one version copy is unavailable"
    );
}

#[test]
fn changed_shared_seal_bytes_after_initial_identity_admission_revoke_provenance() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"held identity boundary");
    let member = &engine.members()[0];
    let identity = validate_seal(&engine.config, member).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &identity);
    let epoch = engine.metadata_integrity.capture();
    let mut changed = identity.clone();
    changed.binding_generation += 1;
    write_json(&member.root.join("disk.json"), &changed, true).unwrap();
    let version = &engine.state.versions[&id];
    let directory = version_dir(member, &version.history_tree_id, &id);
    let error =
        crate::journal::verify_local_publication_in_context(&directory, version, Some(&context))
            .unwrap_err();
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn version_diagnostics_notify_shared_faults_but_not_payload_availability() {
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"version diagnostic scopes");
    let epoch = engine.metadata_integrity.capture();
    fs::remove_file(payload_path(&engine, &engine.members()[0], &id)).unwrap();
    assert!(!engine.check().unwrap().issues.is_empty());
    assert!(epoch.matches(&engine.metadata_integrity));
    fs::remove_file(
        engine.members()[0]
            .root
            .join("metadata/pool-membership/config.json"),
    )
    .unwrap();
    assert!(!engine.check().unwrap().issues.is_empty());
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn scheduled_resource_failure_revokes_before_terminal_progress_and_pacer_wait() {
    use std::cell::Cell;
    let (_temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository, b"namespace body");
    move_fixture_into_namespace(&mut engine, &repository);
    let owner = engine
        .begin_integrity_scrub_schedule(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs(7200),
        )
        .unwrap();
    let outcome = engine
        .prepare_scheduled_integrity_scrub(&owner)
        .unwrap()
        .run(
            &IntegrityScrubCancellation::new(),
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    assert!(engine.finish_integrity_scrub(outcome).is_healthy());
    let epoch = engine.metadata_integrity.capture();
    let version = &engine.state.versions[&id];
    let marker = engine.config.disks[0]
        .root
        .join("histories")
        .join(&version.history_tree_id)
        .join("file-containers")
        .join(&version.file_id)
        .join("namespace/.zxs-namespace.json");
    fs::write(marker, vec![b' '; 70 * 1024]).unwrap();
    let awaiting_charge = Cell::new(false);
    let terminal = Cell::new(false);
    let plan = engine.prepare_scheduled_integrity_scrub(&owner).unwrap();
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| {
            let batch = engine.admit_integrity_scrub_batch(plan, id)?;
            awaiting_charge.set(true);
            Ok(batch)
        },
        |_| {
            if awaiting_charge.replace(false) {
                assert!(terminal.get());
                assert!(!engine.has_current_integrity_scrub_coverage());
            }
        },
        |report| {
            if let IntegrityScrubCompletion::Failed(error) = &report.completion {
                assert_eq!(error.code(), ErrorCode::ResourceLimit);
                terminal.set(true);
                assert!(
                    !engine.has_current_integrity_scrub_coverage(),
                    "revocation precedes the terminal callback"
                );
                assert!(epoch.matches(&engine.metadata_integrity));
            }
        },
    );
    assert!(terminal.get());
    assert!(
        matches!(engine.finish_integrity_scrub(outcome).completion,IntegrityScrubCompletion::Failed(ref error) if error.code()==ErrorCode::ResourceLimit)
    );
}

#[test]
fn scheduled_real_purge_supersession_revokes_before_finish_without_a_metadata_fault() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"old body");
    save(&mut engine, &repository, b"current body");
    let purge = engine
        .create_purge_plan(&repository.history_tree_id, vec![old], "admin")
        .unwrap();
    let owner = engine
        .begin_integrity_scrub_schedule(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs(7200),
        )
        .unwrap();
    let outcome = engine
        .prepare_scheduled_integrity_scrub(&owner)
        .unwrap()
        .run(
            &IntegrityScrubCancellation::new(),
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    assert!(engine.finish_integrity_scrub(outcome).is_healthy());
    let epoch = engine.metadata_integrity.capture();
    let plan = engine.prepare_scheduled_integrity_scrub(&owner).unwrap();
    engine
        .commit_plan(&purge.plan_id, "admin", OperationId::generate())
        .unwrap();
    assert!(engine.has_current_integrity_scrub_coverage());
    let mut superseded = false;
    let outcome = plan.run(
        &IntegrityScrubCancellation::new(),
        |plan, id| engine.admit_integrity_scrub_batch(plan, id),
        |_| {},
        |report| {
            if matches!(report.completion, IntegrityScrubCompletion::Superseded) {
                superseded = true;
                assert!(!engine.has_current_integrity_scrub_coverage());
                assert!(epoch.matches(&engine.metadata_integrity));
            }
        },
    );
    assert!(superseded);
    assert!(matches!(
        engine.finish_integrity_scrub(outcome).completion,
        IntegrityScrubCompletion::Superseded
    ));
}
