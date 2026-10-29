use super::*;

fn complete(name: &str) -> UploadRecord {
    let mut record = record(name, "alice");
    record.logical_size = 3;
    record.content_hash = Some(record.persisted_hash.parse().unwrap());
    record
}

fn authority(record: &UploadRecord) -> storage_contract::Workspace {
    storage_contract::Workspace {
        workspace_id: record.workspace_id.clone(),
        purpose: storage_contract::WorkspacePurpose::Normal,
        branch_id: "branch".into(),
        principal: record.principal.clone(),
        base_head: "head".into(),
        fencing_token: 2,
        files: BTreeMap::from([(
            record.item_id.clone(),
            storage_contract::StagedFile {
                item_id: record.item_id.clone(),
                path: name_policy::PortablePath::parse(&record.path).unwrap(),
                source_path: "/core/independent/full-copy".into(),
                size: record.logical_size,
                content_hash: record.content_hash.unwrap(),
                dirty_generation: record.generation,
                base_version_id: None,
            },
        )]),
        selected_file_branches: BTreeMap::new(),
        selected_file_contexts: BTreeMap::new(),
        selected_directory_branches: BTreeMap::new(),
        directory_selection_generations: BTreeMap::new(),
        file_selection_generations: BTreeMap::new(),
        namespace_operations: Vec::new(),
        created_at: 1,
        state: storage_contract::WorkspaceState::Committed,
    }
}

fn publish(budget: &Arc<SpoolBudget>, record: &UploadRecord) -> Arc<StagedFile> {
    let reservation = budget
        .reserve_with_available("alice", &record.data_file, 3, u64::MAX)
        .unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    staged.write(b"abc").unwrap();
    staged.sync().unwrap();
    uploads::persist_staged(record, &staged).unwrap();
    staged
}

fn retire(budget: &SpoolBudget, record: &UploadRecord) {
    budget
        .retire_workspace(&authority(record), &Arc::new(Mutex::new(BTreeSet::new())))
        .unwrap();
}

#[test]
fn committed_current_and_identical_retained_copy_release_quota_for_continuous_saves() {
    let root = private_tempdir();
    let quota = METADATA_ALLOWANCE + 3;
    let budget = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    for number in 0..4 {
        let record = complete(&format!("copy-{number}.data"));
        let staged = publish(&budget, &record);
        staged.retain_generation(&record).unwrap();
        drop(staged);
        assert_eq!(total(&budget), quota);
        retire(&budget, &record);
        assert_eq!(total(&budget), 0);
        assert!(!root.path().join(&record.data_file).exists());
        assert!(!root.path().join(uploads::retained_name(&record)).exists());
        assert!(
            !root
                .path()
                .join(format!("{}.json", uploads::key("ws", "item")))
                .exists()
        );
        assert!(budget.ledger.lock().unwrap().metadata.is_empty());
    }
    drop(budget);
    assert_eq!(
        total(&SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap()),
        0
    );
}

#[test]
fn active_owner_and_claim_defer_retirement_without_losing_capacity_or_files() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let record = complete("copy.data");
    let staged = publish(&budget, &record);
    retire(&budget, &record);
    assert!(root.path().join(&record.data_file).exists());
    drop(staged);
    let claims = Arc::new(Mutex::new(BTreeSet::from([uploads::key("ws", "item")])));
    budget
        .retire_workspace(&authority(&record), &claims)
        .unwrap();
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 3);
    claims.lock().unwrap().clear();
    budget
        .retire_workspace(&authority(&record), &claims)
        .unwrap();
    assert_eq!(total(&budget), 0);
}

#[test]
fn restart_after_phase_one_deletion_converges_without_new_receipt() {
    let root = private_tempdir();
    let quota = METADATA_ALLOWANCE + 3;
    let budget = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    let record = complete("copy.data");
    let staged = publish(&budget, &record);
    staged.retain_generation(&record).unwrap();
    drop(staged);
    budget
        .directory
        .remove_files(&[
            EntryName::new(&record.data_file).unwrap(),
            EntryName::new(format!("{}.owner.json", record.data_file)).unwrap(),
            EntryName::new(uploads::retained_name(&record)).unwrap(),
        ])
        .unwrap();
    // Crash boundary: no live refund happened, and current remains the locator.
    assert_eq!(total(&budget), quota);
    drop(budget);
    let recovered =
        SpoolBudget::recover(root.path(), limits(METADATA_ALLOWANCE, METADATA_ALLOWANCE)).unwrap();
    assert_eq!(total(&recovered), METADATA_ALLOWANCE);
    retire(&recovered, &record);
    assert_eq!(total(&recovered), 0);
}

#[test]
fn other_valid_reference_and_target_corruption_preserve_copy_but_unrelated_unknown_does_not() {
    enum Extra {
        DifferentReference,
        TargetCorrupt,
        UnrelatedCorrupt,
    }
    for extra in [
        Extra::DifferentReference,
        Extra::TargetCorrupt,
        Extra::UnrelatedCorrupt,
    ] {
        let root = private_tempdir();
        let budget = SpoolBudget::recover(
            root.path(),
            limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
        )
        .unwrap();
        let record = complete("copy.data");
        drop(publish(&budget, &record));
        match extra {
            Extra::DifferentReference => {
                let mut other = record.clone();
                other.item_id = "other".into();
                uploads::persist(root.path(), &other).unwrap();
            }
            Extra::TargetCorrupt => {
                AtomicFile::create(
                    &budget.directory,
                    &EntryName::new(uploads::retained_name(&record)).unwrap(),
                    b"broken",
                )
                .unwrap();
            }
            Extra::UnrelatedCorrupt => {
                AtomicFile::create(
                    &budget.directory,
                    &EntryName::new("opaque.json").unwrap(),
                    b"broken",
                )
                .unwrap();
            }
        }
        drop(budget);
        let budget = SpoolBudget::recover(
            root.path(),
            limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
        )
        .unwrap();
        retire(&budget, &record);
        match extra {
            Extra::DifferentReference | Extra::TargetCorrupt => {
                assert!(root.path().join(&record.data_file).exists())
            }
            Extra::UnrelatedCorrupt => {
                assert!(!root.path().join(&record.data_file).exists());
                assert_eq!(
                    fs::read(root.path().join("opaque.json")).unwrap(),
                    b"broken"
                );
                assert_eq!(total(&budget), METADATA_ALLOWANCE + 6);
            }
        }
    }
}

#[test]
fn retirement_identity_failure_retains_charge_and_releases_claim() {
    let root = private_tempdir();
    let path = root.path().join("spool");
    PrivateDirectory::create(&path).unwrap();
    let budget = SpoolBudget::recover(
        &path,
        limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let record = complete("copy.data");
    drop(publish(&budget, &record));
    fs::rename(&path, root.path().join("original")).unwrap();
    PrivateDirectory::create(&path).unwrap();
    let claims = Arc::new(Mutex::new(BTreeSet::new()));
    let error = budget
        .retire_workspace(&authority(&record), &claims)
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert!(budget.is_uncertain());
    assert!(claims.lock().unwrap().is_empty());
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 3);
    assert_eq!(
        fs::read(root.path().join("original/copy.data")).unwrap(),
        b"abc"
    );
    assert!(fs::read_dir(&path).unwrap().next().is_none());
    assert!(
        budget
            .reserve_with_available("alice", "next.data", 0, u64::MAX)
            .is_err()
    );
}

#[test]
fn recovered_payload_that_is_also_other_upload_metadata_is_not_retired() {
    let root = private_tempdir();
    let mut other = complete("other.data");
    other.item_id = "other".into();
    uploads::persist(root.path(), &other).unwrap();
    let name = format!("{}.json", uploads::key("ws", "other"));
    let bytes = fs::read(root.path().join(&name)).unwrap();
    let mut record = complete(&name);
    record.logical_size = bytes.len() as u64;
    record.persisted_bytes = record.logical_size;
    record.persisted_hash = blake3::hash(&bytes).to_hex().to_string();
    record.content_hash = Some(record.persisted_hash.parse().unwrap());
    uploads::persist(root.path(), &record).unwrap();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let before = total(&budget);
    retire(&budget, &record);
    assert_eq!(total(&budget), before);
    assert_eq!(fs::read(root.path().join(&name)).unwrap(), bytes);
}

#[test]
fn durable_replacement_of_opaque_retained_record_refunds_only_deleted_metadata_charge() {
    let root = private_tempdir();
    let record = complete("copy.data");
    let directory = PrivateDirectory::open_trusted(root.path()).unwrap();
    AtomicFile::create(
        &directory,
        &EntryName::new(uploads::retained_name(&record)).unwrap(),
        b"broken",
    )
    .unwrap();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let staged = publish(&budget, &record);
    staged.retain_generation(&record).unwrap();
    drop(staged);
    assert!(total(&budget) > METADATA_ALLOWANCE + 3);
    retire(&budget, &record);
    assert_eq!(total(&budget), 0);
    assert!(budget.ledger.lock().unwrap().metadata.is_empty());
}

#[test]
fn maintenance_failure_is_nonblocking_and_only_its_successful_retry_clears_it() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let first = complete("first.data");
    let staged = publish(&budget, &first);
    staged.retain_generation(&first).unwrap();
    drop(staged);
    let retained = RelativePath::new(uploads::retained_name(&first)).unwrap();
    let original = fs::read(root.path().join(uploads::retained_name(&first))).unwrap();
    AtomicFile::replace(&budget.directory, &retained, b"damaged").unwrap();
    retire(&budget, &first);
    assert_eq!(budget.maintenance_issue_count(), 1);
    assert!(!budget.is_uncertain());
    retire(&budget, &first);
    assert_eq!(budget.maintenance_issue_count(), 1);
    let mut other = complete("other.data");
    other.item_id = "other".into();
    drop(publish(&budget, &other));
    retire(&budget, &other);
    assert_eq!(budget.maintenance_issue_count(), 1);
    AtomicFile::replace(&budget.directory, &retained, &original).unwrap();
    retire(&budget, &first);
    assert_eq!(budget.maintenance_issue_count(), 0);
    assert_eq!(total(&budget), 0);
}

enum ReboundMetadata {
    Current,
    UnindexedRetained,
}

fn observed_rebinding_preserves_both_copies(target: ReboundMetadata) {
    let root = private_tempdir();
    let quota = 8 * METADATA_ALLOWANCE;
    let budget = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    let mut first = complete("first.data");
    first.item_id = "a".into();
    let mut second = complete("second.data");
    second.item_id = "b".into();
    drop(publish(&budget, &first));
    drop(publish(&budget, &second));
    let mut workspace = authority(&first);
    workspace.files.extend(authority(&second).files);
    let mut rebound = first.clone();
    rebound.data_file = second.data_file.clone();
    match target {
        ReboundMetadata::Current => uploads::persist(root.path(), &rebound).unwrap(),
        ReboundMetadata::UnindexedRetained => {
            AtomicFile::create(
                &budget.directory,
                &EntryName::new(uploads::retained_name(&first)).unwrap(),
                &json_codec::to_vec(&rebound).unwrap(),
            )
            .unwrap();
        }
    }
    let before = total(&budget);
    let claims = Arc::new(Mutex::new(BTreeSet::new()));
    assert!(budget.retire_workspace(&workspace, &claims).is_err());
    assert!(budget.is_uncertain());
    assert_eq!(total(&budget), before);
    assert!(claims.lock().unwrap().is_empty());
    assert_eq!(
        fs::read(root.path().join(&first.data_file)).unwrap(),
        b"abc"
    );
    assert_eq!(
        fs::read(root.path().join(&second.data_file)).unwrap(),
        b"abc"
    );
    budget
        .retire_workspace(&authority(&second), &claims)
        .unwrap();
    assert_eq!(total(&budget), before);
    assert!(
        budget
            .reserve_with_available("alice", "next.data", 0, u64::MAX)
            .is_err()
    );
    drop(budget);
    let recovered = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    recovered.retire_workspace(&workspace, &claims).unwrap();
    assert_eq!(
        fs::read(root.path().join(&second.data_file)).unwrap(),
        b"abc"
    );
}

#[test]
fn observed_current_rebinding_stops_retirement_before_the_new_target() {
    observed_rebinding_preserves_both_copies(ReboundMetadata::Current);
}

#[test]
fn observed_unindexed_retained_rebinding_invalidates_the_reference_inventory() {
    observed_rebinding_preserves_both_copies(ReboundMetadata::UnindexedRetained);
}

#[cfg(feature = "core")]
#[test]
#[ignore = "requires the isolated budget-io-eio.sh libc fault child"]
fn real_retirement_directory_sync_failure_preserves_published_authority_and_charge() {
    use crate::Backend;
    let root = std::path::PathBuf::from(std::env::var_os("ZXS_SMB_BUDGET_FAULT_ROOT").unwrap());
    let second = std::env::var("ZXS_HTTP_RETIREMENT_BARRIER").unwrap() == "second";
    let core = private_tempdir();
    let mut engine = storage_engine::Engine::initialize(
        &core.path().join("system"),
        storage_contract::Config::development([
            core.path().join("d1"),
            core.path().join("d2"),
            core.path().join("d3"),
        ]),
    )
    .unwrap();
    let identity = crate::engine_backend::install_test_administrator(&mut engine, "alice");
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Retirement").unwrap(),
            "P01",
            3,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"abc")
        .unwrap();
    let backend = crate::engine_backend::EngineBackend::new(engine);
    let operation = storage_contract::OperationId::generate();
    let published = backend
        .commit_workspace(
            &identity,
            crate::CommitWorkspace {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: operation,
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "published before local cleanup".into(),
                request_digest: None,
            },
        )
        .unwrap();
    let committed = backend
        .http_spool_workspace(&identity, &workspace.workspace_id)
        .unwrap()
        .unwrap();
    let file = committed.files.values().next().unwrap();
    let mut record = complete("copy.data");
    record.workspace_id = committed.workspace_id.clone();
    record.item_id = file.item_id.clone();
    record.generation = file.dirty_generation;
    let mut config = crate::ServerConfig::loopback(core.path());
    config.spool_dir = root.clone();
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let spool = crate::http_spool::HttpSpool::open_blocking(&config, &backend);
    let budget = spool.budget().unwrap();
    let staged = publish(&budget, &record);
    staged.retain_generation(&record).unwrap();
    drop(staged);
    let before = total(&budget);
    fs::write(root.join(".inject"), b"enable only retirement barriers").unwrap();
    let claims = Arc::new(Mutex::new(BTreeSet::new()));
    let error = budget.retire_workspace(&committed, &claims).unwrap_err();
    let mut cause: &dyn std::error::Error = &error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    assert_eq!(
        cause
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(5)
    );
    assert_eq!(total(&budget), before);
    assert!(budget.is_uncertain());
    assert_eq!(spool.status(), "recovery_required");
    assert!(claims.lock().unwrap().is_empty());
    assert!(
        budget
            .reserve_with_available("alice", "next.data", 0, u64::MAX)
            .is_err()
    );
    assert!(!root.join(&record.data_file).exists());
    let current = root.join(format!(
        "{}.json",
        uploads::key(&record.workspace_id, &record.item_id)
    ));
    assert_eq!(current.exists(), !second);
    assert_eq!(
        backend.operation(&identity, operation).unwrap().outcome,
        published.into()
    );
    fs::remove_file(root.join(".inject")).unwrap();
    drop(budget);
    drop(spool);
    let budget = SpoolBudget::recover(
        &root,
        limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    assert_eq!(total(&budget), if second { 0 } else { METADATA_ALLOWANCE });
    budget.retire_workspace(&committed, &claims).unwrap();
    assert_eq!(total(&budget), 0);
    assert!(
        backend
            .http_spool_workspace(&identity, &record.workspace_id)
            .unwrap()
            .is_some()
    );
    println!(
        "HTTP retirement barrier={} completed",
        if second { "second" } else { "first" }
    );
}
