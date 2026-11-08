use super::*;

fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development([temp.path().join("one"), temp.path().join("two")]).unwrap();
    config.disks[1].slot_id = "P02".into();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    (temp, engine)
}
fn repository(engine: &mut Engine, name: &str, slot: &str) -> Repository {
    engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            slot,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
}
fn save(engine: &mut Engine, repo: &Repository, path: &str) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, b"disk catalog")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "disk catalog".into(),
            request_digest: None,
        })
        .unwrap();
}

#[test]
fn same_name_on_distinct_primary_disks_has_distinct_scoped_catalogs() {
    let (_temp, mut engine) = fixture();
    let left = repository(&mut engine, "left", "P01");
    let right = repository(&mut engine, "right", "P02");
    save(&mut engine, &left, "same.txt");
    save(&mut engine, &right, "same.txt");
    engine.rebuild_disk_catalog("P01").unwrap();
    engine.rebuild_disk_catalog("P02").unwrap();
    let left_rows = engine.disk_catalog_snapshot("P01").unwrap();
    let right_rows = engine.disk_catalog_snapshot("P02").unwrap();
    assert_eq!(left_rows.versions.len(), 1);
    assert_eq!(right_rows.versions.len(), 1);
    assert_eq!(
        left_rows.versions[0].version.history_tree_id,
        left.history_tree_id
    );
    assert_eq!(
        right_rows.versions[0].version.history_tree_id,
        right.history_tree_id
    );
    assert_ne!(
        left_rows.versions[0].version.file_id,
        right_rows.versions[0].version.file_id
    );
    assert_eq!(
        left_rows.versions[0].owner_path.as_deref(),
        Some("same.txt")
    );
    assert_eq!(
        right_rows.versions[0].owner_path.as_deref(),
        Some("same.txt")
    );
    let left_file = engine.disk("P01").unwrap().root.join(DISK_DATABASE_NAME);
    let right_file = engine.disk("P02").unwrap().root.join(DISK_DATABASE_NAME);
    let bytes = std::fs::read(left_file).unwrap();
    std::fs::write(&right_file, &bytes).unwrap();
    assert!(engine.disk_catalog_snapshot("P02").is_err());
    assert!(engine.rebuild_disk_catalog("P02").is_err());
    assert_eq!(std::fs::read(right_file).unwrap(), bytes);
}

#[test]
fn explicit_rebuild_survives_cold_open_and_requires_refresh_after_path_change() {
    let (temp, mut engine) = fixture();
    let repo = repository(&mut engine, "files", "P01");
    save(&mut engine, &repo, "before.txt");
    let first = engine.rebuild_disk_catalog("P01").unwrap();
    let again = engine.rebuild_disk_catalog("P01").unwrap();
    assert_eq!(first.records_hash, again.records_hash);
    let expected = engine.disk_catalog_snapshot("P01").unwrap();
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.disk_catalog_snapshot("P01").unwrap(), expected);
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "before.txt",
                Some("after.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert!(engine.disk_catalog_snapshot("P01").is_err());
    engine.rebuild_disk_catalog("P01").unwrap();
    assert_eq!(
        engine.disk_catalog_snapshot("P01").unwrap().versions[0]
            .owner_path
            .as_deref(),
        Some("after.txt")
    );
}

#[test]
fn wrong_generation_is_not_assigned_to_the_current_disk_and_unknown_database_is_preserved() {
    let (_temp, mut engine) = fixture();
    let repo = repository(&mut engine, "files", "P01");
    save(&mut engine, &repo, "file");
    let (_disk, identity, _handle) = engine.catalog_disk("P01").unwrap();
    let mut old_binding = (*engine.state).clone();
    for placement in old_binding.placements.values_mut() {
        placement
            .binding_generations
            .insert("P01".into(), identity.binding_generation + 1);
    }
    assert!(
        snapshot(&old_binding, identity)
            .unwrap()
            .versions
            .is_empty()
    );
    let path = engine.disk("P01").unwrap().root.join(DISK_DATABASE_NAME);
    std::fs::write(&path, b"unknown data, not a disposable database").unwrap();
    assert!(engine.rebuild_disk_catalog("P01").is_err());
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"unknown data, not a disposable database"
    );
    assert!(engine.rebuild_disk_catalog("not-registered").is_err());
}

#[test]
fn unrelated_disk_publication_keeps_materialization_sequence_but_verifies_new_state() {
    let (_temp, mut engine) = fixture();
    let left = repository(&mut engine, "left", "P01");
    let right = repository(&mut engine, "right", "P02");
    save(&mut engine, &left, "left.txt");
    let saved = engine.rebuild_disk_catalog("P01").unwrap();
    let path = engine.disk("P01").unwrap().root.join(DISK_DATABASE_NAME);
    let original = std::fs::read(&path).unwrap();
    save(&mut engine, &right, "right.txt");
    let observed = engine.disk_catalog_observation("P01").unwrap();
    assert_eq!(observed.snapshot.source_sequence, saved.source_sequence);
    assert_eq!(observed.verified_sequence, engine.state.sequence);
    assert!(observed.verified_sequence > observed.snapshot.source_sequence);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn explicit_refresh_preserves_completed_disk_and_reports_unknown_target_bytes() {
    let (_temp, mut engine) = fixture();
    let left = repository(&mut engine, "left", "P01");
    let right = repository(&mut engine, "right", "P02");
    save(&mut engine, &left, "left.txt");
    save(&mut engine, &right, "right.txt");
    let bad = engine.disk("P02").unwrap().root.join(DISK_DATABASE_NAME);
    std::fs::write(&bad, b"unrelated object").unwrap();
    let report = engine
        .refresh_disk_catalogs(&["P01".into(), "P02".into()])
        .unwrap();
    assert_eq!(report.source_sequence, engine.state.sequence);
    assert!(matches!(
        report.results[0].outcome,
        DiskCatalogRefreshOutcome::Updated(_)
    ));
    assert!(matches!(
        report.results[1].outcome,
        DiskCatalogRefreshOutcome::Failed(_)
    ));
    assert_eq!(std::fs::read(bad).unwrap(), b"unrelated object");
    assert_eq!(
        engine.disk_catalog_snapshot("P01").unwrap().versions.len(),
        1
    );
}

#[test]
fn invalid_bulk_target_list_has_no_partial_database_creation() {
    let (_temp, mut engine) = fixture();
    let path = engine.disk("P01").unwrap().root.join(DISK_DATABASE_NAME);
    assert!(engine.refresh_disk_catalogs(&[]).is_err());
    assert!(
        engine
            .refresh_disk_catalogs(&["P01".into(), "P01".into()])
            .is_err()
    );
    assert!(
        engine
            .refresh_disk_catalogs(&["P01".into(), "unknown".into()])
            .is_err()
    );
    assert!(!path.exists());
}
