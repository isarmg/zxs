//! Real published metadata exercises global backup validation before page selection.
use super::*;
use storage_contract::{
    BranchName, Config, Entry, ErrorCode, NamespaceOperationKind, OperationId, RepositoryName,
};
use storage_engine::{BackupConfigurationRequest, CommitRequest, ProtectionPolicyUpdate};

fn initialize(maximum: usize) -> (tempfile::TempDir, AdmissionEngine) {
    let root = crate::tests::private_tempdir();
    let mut config = Config::development([root.path().join("primary"), root.path().join("backup")]);
    config.max_entries = maximum;
    let engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    (root, AdmissionEngine::new(engine))
}

fn repository(engine: &mut AdmissionEngine, name: &str) -> Repository {
    engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            "P01",
            2,
            "owner",
            OperationId::generate(),
        )
        .unwrap()
}

fn save(engine: &mut AdmissionEngine, repo: &Repository, path: &str, body: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "backup listing fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn configure(engine: &mut AdmissionEngine, repo: &Repository, path: &str, prefix: &str) {
    let previous = engine
        .backup_configuration(&repo.repository_id, path)
        .unwrap();
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path,
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), prefix.into())]),
            expected_generation: previous.generation,
            principal: "owner",
            operation_id: OperationId::generate(),
        })
        .unwrap();
}

fn namespace(
    engine: &mut AdmissionEngine,
    repo: &Repository,
    kind: NamespaceOperationKind,
    path: &str,
    destination: Option<&str>,
) {
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(kind, path, destination).unwrap(),
            "owner",
            OperationId::generate(),
        )
        .unwrap();
}

fn listing(
    engine: &AdmissionEngine,
    path: &str,
    options: &[(&str, &str)],
) -> Result<Value, ApiError> {
    let mut query = BTreeMap::from([("path".into(), path.into())]);
    query.extend(
        options
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into())),
    );
    let request = PageRequest::parse(engine, crate::ReadListing::parse("B01".into(), query)?)?;
    let selected = BackupListingRead::capture(engine, "B01", path, request)?.select()?;
    let projected = selected.project(engine)?;
    json_codec::to_value(crate::backend_listing::ListingResponse::from(
        selected.finish(projected),
    ))
    .map_err(ApiError::invalid_source)
}

fn read_only(snapshot: &Value) {
    assert_eq!(snapshot["backup"], true);
    assert_eq!(snapshot["writable"], false);
    assert!(snapshot["context"].is_null());
    assert_eq!(snapshot["directory_branches"], json!([]));
    assert_eq!(snapshot["directory_branches_supported"], false);
    assert_eq!(snapshot["default_branch_scope"], false);
    for row in snapshot["entries"].as_array().unwrap() {
        assert_eq!(row["backup"], true);
        assert_eq!(row["read_only"], true);
    }
}

#[test]
fn global_conflicts_survive_paging_filtering_and_unrelated_directory_requests() {
    let (_root, mut engine) = initialize(100);
    let visible = repository(&mut engine, "Visible");
    let first = repository(&mut engine, "First");
    let second = repository(&mut engine, "Second");
    for repo in [&visible, &first, &second] {
        save(&mut engine, repo, "report.txt", b"independent body");
    }
    configure(&mut engine, &visible, "report.txt", "A-visible");
    configure(&mut engine, &first, "report.txt", "Z-conflict");
    // Exercise both equal file paths and a file used as another file's parent.
    for prefix in ["z-CONFLICT", "Z-conflict/report.txt"] {
        configure(&mut engine, &second, "report.txt", prefix);
        for (path, query) in [
            ("", ""),
            ("", "visible"),
            ("A-visible", "report"),
            ("missing-directory", ""),
        ] {
            let failure = listing(&engine, path, &[("limit", "1"), ("query", query)]).unwrap_err();
            assert_eq!(failure.status, http::StatusCode::CONFLICT);
            assert_eq!(failure.code, ErrorCode::BackupPathConflict);
        }
    }
}

#[test]
fn global_limit_counts_shared_synthetic_parents_across_repositories() {
    for maximum in [4, 5] {
        let (_root, mut engine) = initialize(maximum);
        for name in ["Alpha", "Beta"] {
            let repo = repository(&mut engine, name);
            save(&mut engine, &repo, "report.txt", b"body");
        }
        // Each admitted namespace contains only one entry. The global backup
        // graph contains P01 plus two repository directories and two files.
        let result = listing(&engine, "P01/Alpha", &[("limit", "1"), ("query", "report")]);
        if maximum == 4 {
            let failure = result.unwrap_err();
            assert_eq!(failure.status, http::StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(failure.code, ErrorCode::ResourceLimitExceeded);
        } else {
            assert_eq!(result.unwrap()["entries"].as_array().unwrap().len(), 1);
            let root = listing(&engine, "", &[]).unwrap();
            assert_eq!(root["total_entries"], "1");
            assert_eq!(root["directory_file_count"], "2");
            assert_eq!(root["entries"][0]["name"], "P01");
        }
    }
}

#[test]
fn canonical_owner_wins_over_frozen_aliases_in_either_namespace_order() {
    for (source, owner) in [("Alpha", "Zulu"), ("Zulu", "Alpha")] {
        let (_root, mut engine) = initialize(100);
        let repo = repository(&mut engine, "Files");
        let source_path = format!("{source}/report.txt");
        let owner_path = format!("{owner}/report.txt");
        let original = save(&mut engine, &repo, &source_path, b"shared original");
        save(&mut engine, &repo, &format!("{owner}/keep.txt"), b"keep");
        let scope = name_policy::LookupPath::parse(source).unwrap();
        let frozen = BranchName::parse("frozen alias").unwrap();
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&scope),
                &frozen,
                "owner",
                OperationId::generate(),
            )
            .unwrap();
        namespace(
            &mut engine,
            &repo,
            NamespaceOperationKind::Rename,
            &source_path,
            Some(&owner_path),
        );
        engine
            .select_directory_branches(
                &repo.repository_id,
                Some(&scope),
                &frozen,
                "owner",
                OperationId::generate(),
            )
            .unwrap();
        assert_eq!(engine.state().file_locations[&original.file_id], owner_path);
        assert_eq!(
            engine
                .list_tree_ref(&repo.main_branch_id)
                .unwrap()
                .iter()
                .filter(|entry| entry.file_id == original.file_id)
                .count(),
            2,
            "fixture must retain both logical aliases"
        );
        let root = listing(&engine, "P01/Files", &[]).unwrap();
        assert_eq!(root["total_entries"], "1");
        assert_eq!(root["directory_file_count"], "2");
        assert_eq!(root["entries"][0]["name"], owner);
        let children = listing(&engine, &format!("P01/Files/{owner}"), &[]).unwrap();
        let row = children["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["file_id"] == original.file_id)
            .unwrap();
        assert_eq!(row["repository_path"], owner_path);
        assert_eq!(row["path"], format!("P01/Files/{owner_path}"));
        assert_eq!(row["entry_id"], original.entry_id);
        assert_eq!(row["version_id"], original.version_id);
        assert_eq!(row["historical_only"], false);
        read_only(&children);
    }
}

#[test]
fn current_slot_version_precedes_newer_history_and_retains_pending_context() {
    let (_root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Files");
    let first = save(&mut engine, &repo, "report.txt", b"first");
    let pending = engine
        .create_file_branch(
            &first.file_id,
            &first.version_id,
            &BranchName::parse("older pending").unwrap(),
            "owner",
            OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let newest = save(&mut engine, &repo, "report.txt", b"newer body");
    assert_ne!(first.version_id, newest.version_id);
    engine
        .select_file_branch(&first.file_id, &pending, "owner", OperationId::generate())
        .unwrap();
    let current = listing(&engine, "P01/Files", &[]).unwrap();
    let row = &current["entries"][0];
    assert_eq!(row["version_id"], first.version_id);
    assert_eq!(row["source_version_id"], first.version_id);
    assert_eq!(row["content_state"], "PENDING");
    assert_eq!(row["selected_branch_id"], pending);
    assert_eq!(row["size"], "5");
    assert_eq!(row["historical_only"], false);
    namespace(
        &mut engine,
        &repo,
        NamespaceOperationKind::Delete,
        "report.txt",
        None,
    );
    let historical = listing(&engine, "P01/Files", &[]).unwrap();
    let row = &historical["entries"][0];
    assert_eq!(row["version_id"], newest.version_id);
    assert_eq!(row["node_id"], newest.version_id);
    assert_eq!(row["entry_id"], first.file_id);
    assert_eq!(row["historical_only"], true);
    assert!(row["repository_path"].is_null());
    assert!(row["selected_branch_id"].is_null());
    read_only(&historical);
}

#[test]
fn newest_available_backup_version_overlays_a_primary_only_current_entry() {
    let (_root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Files");
    save(&mut engine, &repo, "report.txt", b"first");
    let available = save(&mut engine, &repo, "report.txt", b"backup body");
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &storage_engine::new_id("policy"),
            repository_id: &repo.repository_id,
            path_prefix: "",
            target_copies: 1,
            expected_generation: 0,
            principal: "owner",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    let primary = save(&mut engine, &repo, "report.txt", b"primary only body");
    assert_eq!(
        engine.state().placements[&primary.version_id].route_slot_ids,
        vec!["P01".to_owned()]
    );
    let snapshot = listing(&engine, "P01/Files", &[]).unwrap();
    let row = &snapshot["entries"][0];
    assert_eq!(row["entry_id"], primary.entry_id);
    assert_eq!(row["file_id"], primary.file_id);
    assert_eq!(row["repository_path"], "report.txt");
    assert_eq!(row["version_id"], available.version_id);
    assert_eq!(row["node_id"], available.version_id);
    assert_eq!(row["size"], "11");
    assert_eq!(row["historical_only"], false);
    read_only(&snapshot);
}

#[test]
fn fixed_prefixes_keep_display_paths_after_primary_rename_and_delete() {
    let (_root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Files");
    let first = save(&mut engine, &repo, "docs/report.txt", b"first");
    configure(&mut engine, &repo, "docs", "Archives/Café");
    let newest = save(&mut engine, &repo, "docs/report.txt", b"newer body");
    namespace(
        &mut engine,
        &repo,
        NamespaceOperationKind::Rename,
        "docs",
        Some("moved"),
    );
    namespace(
        &mut engine,
        &repo,
        NamespaceOperationKind::Rename,
        "moved/report.txt",
        Some("moved/renamed.txt"),
    );
    let snapshot = listing(&engine, "archives/CAFE\u{301}", &[]).unwrap();
    let row = &snapshot["entries"][0];
    assert_eq!(row["path"], "Archives/Café/report.txt");
    assert_eq!(row["name"], "report.txt");
    assert_eq!(row["file_id"], first.file_id);
    assert_eq!(row["version_id"], newest.version_id);
    assert_eq!(row["repository_id"], repo.repository_id);
    assert_eq!(row["repository_path"], "moved/renamed.txt");
    assert_eq!(row["historical_only"], false);
    assert_eq!(snapshot["directory_file_count"], "1");
    read_only(&snapshot);
    namespace(
        &mut engine,
        &repo,
        NamespaceOperationKind::Delete,
        "moved/renamed.txt",
        None,
    );
    let historical = listing(&engine, "Archives/Café", &[]).unwrap();
    let row = &historical["entries"][0];
    assert_eq!(row["path"], "Archives/Café/report.txt");
    assert_eq!(row["version_id"], newest.version_id);
    assert_eq!(row["node_id"], newest.version_id);
    assert_eq!(row["historical_only"], true);
    assert!(row["repository_path"].is_null());
    assert!(row["selected_branch_id"].is_null());
    assert_eq!(
        engine.state().file_backup_paths[&first.file_id]["B01"],
        "Archives/Café/report.txt"
    );
}

#[test]
fn backup_pages_preserve_directory_order_cursors_and_filtered_selection() {
    let (_root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Files");
    for path in [
        "alpha.txt",
        "Beta.txt",
        "Café.txt",
        "zeta.txt",
        "nested/child.txt",
    ] {
        save(&mut engine, &repo, path, b"body");
    }
    let all = listing(&engine, "P01/Files", &[]).unwrap();
    let selected = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "zeta.txt")
        .unwrap();
    let id = selected["entry_id"].as_str().unwrap();
    LISTED_ENTRY_PROJECTIONS.set(0);
    let first = listing(
        &engine,
        "p01/FILES",
        &[
            ("limit", "2"),
            ("selected_path", "P01/files/ZETA.TXT"),
            ("selected_entry_id", id),
        ],
    )
    .unwrap();
    assert_eq!(first["total_entries"], "5");
    assert_eq!(first["directory_file_count"], "5");
    assert_eq!(first["entries"][0]["kind"], "directory");
    assert_eq!(first["entries"][0]["name"], "nested");
    assert_eq!(first["entries"][1]["name"], "alpha.txt");
    assert_eq!(first["selected_entry"], *selected);
    assert!(LISTED_ENTRY_PROJECTIONS.get() <= 3);
    let next = listing(
        &engine,
        "P01/Files",
        &[
            ("limit", "2"),
            ("cursor", first["next_cursor"].as_str().unwrap()),
        ],
    )
    .unwrap();
    let previous = listing(
        &engine,
        "P01/Files",
        &[
            ("limit", "2"),
            ("cursor", next["previous_cursor"].as_str().unwrap()),
        ],
    )
    .unwrap();
    assert_eq!(previous["entries"], first["entries"]);
    assert_eq!(previous["snapshot"], first["snapshot"]);
    let filtered = listing(
        &engine,
        "P01/Files",
        &[
            ("limit", "1"),
            ("query", "ALPHA"),
            ("selected_path", "P01/Files/zeta.txt"),
            ("selected_entry_id", id),
        ],
    )
    .unwrap();
    assert_eq!(filtered["total_entries"], "1");
    assert_eq!(filtered["entries"][0]["name"], "alpha.txt");
    assert_eq!(filtered["selected_entry"], *selected);
    assert!(filtered["next_cursor"].is_null());
    assert_eq!(filtered["directory_file_count"], "5");
}

#[test]
fn selected_backup_page_releases_scan_tables_and_head_records_before_recheck() {
    let (root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Retention");
    let old = save(&mut engine, &repo, "alpha.txt", b"old alpha");
    let other = save(&mut engine, &repo, "beta.txt", b"beta");
    let newest = save(&mut engine, &repo, "alpha.txt", b"new alpha");
    let weak_version = |id: &str| Arc::downgrade(&engine.state().versions.get_shared(id).unwrap());
    let old_version = weak_version(&old.version_id);
    let other_version = weak_version(&other.version_id);
    let selected_version = weak_version(&newest.version_id);
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let old_head = Arc::downgrade(&engine.state().commits.get_shared(head).unwrap());
    let input = crate::ReadListing::parse(
        "B01".into(),
        BTreeMap::from([
            ("path".into(), "P01/Retention".into()),
            ("limit".into(), "1".into()),
        ]),
    )
    .unwrap();
    let request = PageRequest::parse(&engine, input).unwrap();
    let selected = BackupListingRead::capture(&engine, "B01", "P01/Retention", request)
        .unwrap()
        .select()
        .unwrap();
    assert_eq!(selected.scope.versions.len(), 1);
    assert!(selected.scope.versions.contains_key(&newest.version_id));
    let projected = selected.project(&engine).unwrap();
    assert_eq!(projected.entries.len(), 1);
    assert_eq!(projected.entries[0].name, "alpha.txt");
    drop(projected);
    drop(engine);
    assert!(old_version.upgrade().is_none());
    assert!(other_version.upgrade().is_none());
    assert!(
        old_head.upgrade().is_none(),
        "only a weak head identity remains"
    );
    assert!(selected_version.upgrade().is_some());

    // The same persisted token and IDs do not admit indices against replacement
    // allocations, even after the former head's strong references are gone.
    let reopened = AdmissionEngine::new(Engine::open(&root.path().join("system")).unwrap());
    let error = selected.project(&reopened).err().unwrap();
    assert_eq!(error.code, ErrorCode::ListingChanged);
    drop(selected);
    assert!(selected_version.upgrade().is_none());
}
