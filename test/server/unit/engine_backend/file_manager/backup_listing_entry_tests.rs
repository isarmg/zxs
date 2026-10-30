//! Backup listing behavior through the authenticated Backend consumer.
use crate::{
    ApiError, Backend,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use json_codec::Value;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use storage_contract::{
    Config, Entry, ErrorCode, NamespaceOperation, NamespaceOperationKind, OperationId, Repository,
    RepositoryName,
};
use storage_engine::{BackupConfigurationRequest, CommitRequest, Engine};

fn initialize(maximum: usize) -> (tempfile::TempDir, Engine) {
    let root = crate::tests::private_tempdir();
    let mut config = Config::development([root.path().join("primary"), root.path().join("backup")]);
    config.max_entries = maximum;
    let engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    (root, engine)
}

fn repository(engine: &mut Engine, name: &str) -> Repository {
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

fn save(engine: &mut Engine, repo: &Repository, path: &str, body: &[u8]) -> Entry {
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
            message: "backup entry fixture".into(),
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

fn configure(engine: &mut Engine, repo: &Repository, path: &str, prefix: &str) {
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

fn authenticated(engine: Engine, root: &Path) -> (Arc<EngineBackend>, AuthStore, Identity) {
    let backend = Arc::new(EngineBackend::new(engine));
    let mut auth = backend
        .protect_auth(AuthStore::initialize(root, "owner", "integration-test-password").unwrap())
        .unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    (backend, auth, identity)
}

fn listing(
    backend: &EngineBackend,
    identity: &Identity,
    path: &str,
    options: &[(&str, &str)],
) -> Result<Value, ApiError> {
    let mut query = BTreeMap::from([("path".into(), path.into())]);
    query.extend(
        options
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into())),
    );
    let request = crate::ReadListing::parse("B01".into(), query)?;
    let snapshot = backend.listing(identity, request)?;
    json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot))
        .map_err(ApiError::invalid_source)
}

#[test]
fn backend_backup_listing_rejects_global_conflicts_outside_the_requested_page() {
    let (root, mut engine) = initialize(100);
    let visible = repository(&mut engine, "Visible");
    let first = repository(&mut engine, "First");
    let second = repository(&mut engine, "Second");
    for repo in [&visible, &first, &second] {
        save(&mut engine, repo, "report.txt", b"separate container");
    }
    configure(&mut engine, &visible, "report.txt", "A-visible");
    configure(&mut engine, &first, "report.txt", "Z-hidden-conflict");
    configure(&mut engine, &second, "report.txt", "z-HIDDEN-conflict");
    let (backend, _auth, identity) = authenticated(engine, root.path());
    for (path, query) in [
        ("", ""),
        ("", "visible"),
        ("A-visible", "report"),
        ("missing-directory", ""),
    ] {
        let failure = listing(
            &backend,
            &identity,
            path,
            &[("limit", "1"), ("query", query)],
        )
        .unwrap_err();
        assert_eq!(failure.status, http::StatusCode::CONFLICT);
        assert_eq!(failure.code, ErrorCode::BackupPathConflict);
    }
}

#[test]
fn backend_backup_listing_keeps_the_configured_global_synthetic_entry_limit() {
    for maximum in [4, 5] {
        let (root, mut engine) = initialize(maximum);
        for name in ["Alpha", "Beta"] {
            let repo = repository(&mut engine, name);
            save(&mut engine, &repo, "report.txt", b"body");
        }
        let (backend, _auth, identity) = authenticated(engine, root.path());
        // One file per primary namespace expands to five global backup nodes:
        // P01, both repository directories, and their two files.
        let result = listing(
            &backend,
            &identity,
            "P01/Alpha",
            &[("limit", "1"), ("query", "report")],
        );
        if maximum == 4 {
            let failure = result.unwrap_err();
            assert_eq!(failure.status, http::StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(failure.code, ErrorCode::ResourceLimitExceeded);
        } else {
            let snapshot = result.unwrap();
            assert_eq!(snapshot["total_entries"], "1");
            assert_eq!(snapshot["entries"][0]["name"], "report.txt");
            let root = listing(&backend, &identity, "", &[]).unwrap();
            assert_eq!(root["total_entries"], "1");
            assert_eq!(root["directory_file_count"], "2");
            assert_eq!(root["entries"][0]["path"], "P01");
        }
    }
}

#[test]
fn backend_backup_listing_preserves_fixed_prefix_history_and_filtered_selection() {
    let (root, mut engine) = initialize(100);
    let repo = repository(&mut engine, "Files");
    let first = save(&mut engine, &repo, "docs/report.txt", b"first");
    let newest = save(&mut engine, &repo, "docs/report.txt", b"newer body");
    let current = save(&mut engine, &repo, "docs/zeta.txt", b"current body");
    configure(&mut engine, &repo, "docs", "Archives/Café");
    for (kind, path, destination) in [
        (NamespaceOperationKind::Rename, "docs", Some("moved")),
        (NamespaceOperationKind::Delete, "moved/report.txt", None),
    ] {
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(kind, path, destination).unwrap(),
                "owner",
                OperationId::generate(),
            )
            .unwrap();
    }
    let (backend, _auth, identity) = authenticated(engine, root.path());
    let snapshot = listing(
        &backend,
        &identity,
        "archives/CAFE\u{301}",
        &[
            ("limit", "1"),
            ("query", "zeta"),
            ("selected_path", "ARCHIVES/CAFE\u{301}/REPORT.TXT"),
            ("selected_entry_id", &first.file_id),
        ],
    )
    .unwrap();
    assert_eq!(snapshot["backup"], true);
    assert_eq!(snapshot["writable"], false);
    assert!(snapshot["context"].is_null());
    assert_eq!(snapshot["directory_branches"], json_codec::json!([]));
    assert_eq!(snapshot["directory_branches_supported"], false);
    assert_eq!(snapshot["default_branch_scope"], false);
    assert_eq!(snapshot["directory_file_count"], "2");
    assert_eq!(snapshot["total_entries"], "1");
    assert!(snapshot["next_cursor"].is_null());
    let row = &snapshot["entries"][0];
    assert_eq!(row["file_id"], current.file_id);
    assert_eq!(row["path"], "Archives/Café/zeta.txt");
    assert_eq!(row["repository_path"], "moved/zeta.txt");
    assert_eq!(row["historical_only"], false);
    assert_eq!(row["read_only"], true);
    let selected = &snapshot["selected_entry"];
    assert_eq!(selected["entry_id"], first.file_id);
    assert_eq!(selected["file_id"], first.file_id);
    assert_eq!(selected["path"], "Archives/Café/report.txt");
    assert_eq!(selected["version_id"], newest.version_id);
    assert_eq!(selected["node_id"], newest.version_id);
    assert_eq!(selected["size"], "10");
    assert_eq!(selected["repository_id"], repo.repository_id);
    assert!(selected["repository_path"].is_null());
    assert!(selected["selected_branch_id"].is_null());
    assert_eq!(selected["historical_only"], true);
    assert_eq!(selected["read_only"], true);
    assert_eq!(selected["backup"], true);
}
