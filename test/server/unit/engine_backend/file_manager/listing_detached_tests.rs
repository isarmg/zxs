//! Primary listing interleavings at the real capture/scan boundary.
use super::*;
use crate::tests::directory_git_fixture::checkpoint;
use crate::{
    Backend,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use std::{
    cell::RefCell,
    sync::{Mutex, mpsc},
    time::Duration,
};
use storage_contract::{CommitRequest, Config, OperationId};

type ScanHook = Box<dyn FnOnce(&ListingMetadata)>;
thread_local! {
    static BEFORE_SCAN: RefCell<Option<ScanHook>> = const { RefCell::new(None) };
}

pub(super) fn before_scan(view: &ListingMetadata) {
    let hook = BEFORE_SCAN.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook(view);
    }
}

fn pause(started: mpsc::SyncSender<u64>, release: mpsc::Receiver<()>) {
    BEFORE_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |view| {
            started.send(view.sequence).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
    });
}

type Fixture = (
    tempfile::TempDir,
    Arc<EngineBackend>,
    AuthStore,
    String,
    Identity,
    Repository,
);

fn fixture() -> Fixture {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Files").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for path in [
        "alpha.txt",
        "beta.txt",
        "zeta.txt",
        "Café/child.txt",
        "Café/z.txt",
    ] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"original")
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "listing fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let mut auth = backend
        .protect_auth(
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap(),
        )
        .unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    (root, backend, auth, token, identity, repo)
}

fn request(path: &str, values: &[(&str, &str)]) -> crate::ReadListing {
    let mut query = values
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect::<BTreeMap<_, _>>();
    query.insert("path".into(), path.into());
    crate::ReadListing::parse("P01".into(), query).unwrap()
}

fn read(
    backend: &dyn Backend,
    identity: &Identity,
    input: crate::ReadListing,
) -> Result<Value, ApiError> {
    backend.listing(identity, input).map(|snapshot| {
        json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot)).unwrap()
    })
}

#[test]
fn writers_finish_during_primary_scan_and_changed_pages_never_project_old_indices() {
    for change_namespace in [true, false] {
        let (_root, backend, _auth, _token, identity, repo) = fixture();
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                LISTED_ENTRY_PROJECTIONS.set(0);
                let result = read(
                    backend.as_ref(),
                    &identity,
                    request("Files", &[("limit", "1")]),
                );
                (result, LISTED_ENTRY_PROJECTIONS.get())
            });
            let captured = observed.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut writer = backend
                .engine
                .try_lock()
                .expect("primary scan retained Engine");
            if change_namespace {
                writer
                    .file_namespace(
                        &repo.repository_id,
                        NamespaceOperation::parse(
                            storage_contract::NamespaceOperationKind::Delete,
                            "alpha.txt",
                            None,
                        )
                        .unwrap(),
                        "owner",
                        OperationId::generate().unwrap(),
                    )
                    .unwrap();
            } else {
                let file = writer
                    .namespace_tree(&repo.main_branch_id)
                    .unwrap()
                    .get("alpha.txt")
                    .unwrap()
                    .file_id
                    .clone();
                writer
                    .set_file_note(
                        &file,
                        None,
                        &storage_contract::Note::parse("changed".into()).unwrap(),
                        "owner",
                        OperationId::generate().unwrap(),
                    )
                    .unwrap();
            }
            assert!(writer.state().sequence > captured);
            drop(writer);
            release.send(()).unwrap();
            let (result, projected) = reader.join().unwrap();
            assert_eq!(
                result.unwrap_err().code,
                storage_contract::ErrorCode::ListingChanged
            );
            assert_eq!(
                projected, 0,
                "final checks precede every bounded row projection"
            );
        });
    }
}

#[test]
fn revoked_identity_wins_over_a_changed_primary_listing() {
    let (_root, backend, mut auth, token, identity, repo) = fixture();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(backend.as_ref(), &identity, request("Files", &[]));
            (result, LISTED_ENTRY_PROJECTIONS.get())
        });
        observed.recv_timeout(Duration::from_secs(10)).unwrap();
        backend
            .engine
            .try_lock()
            .expect("primary scan retained Engine")
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Delete,
                    "alpha.txt",
                    None,
                )
                .unwrap(),
                "owner",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        auth.logout(&token).unwrap();
        release.send(()).unwrap();
        let (result, projected) = reader.join().unwrap();
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
        assert_eq!(projected, 0);
    });
}

#[test]
fn auth_only_publication_keeps_primary_snapshot_selection_and_bounded_projection() {
    let (_root, backend, mut auth, _token, identity, _repo) = fixture();
    let all = read(backend.as_ref(), &identity, request("Files", &[])).unwrap();
    let selected = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "zeta.txt")
        .unwrap();
    let values = [
        ("limit", "1"),
        ("query", "alpha"),
        ("selected_path", selected["path"].as_str().unwrap()),
        ("selected_entry_id", selected["entry_id"].as_str().unwrap()),
    ];
    let expected = read(backend.as_ref(), &identity, request("Files", &values)).unwrap();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(backend.as_ref(), &identity, request("Files", &values));
            (result, LISTED_ENTRY_PROJECTIONS.get())
        });
        let captured = observed.recv_timeout(Duration::from_secs(10)).unwrap();
        let snapshot = backend
            .engine
            .try_lock()
            .expect("primary scan retained Engine")
            .listing_snapshot()
            .to_owned();
        auth.login("owner", "integration-test-password").unwrap();
        {
            let engine = backend.engine.lock().unwrap();
            assert!(engine.state().sequence > captured);
            assert_eq!(engine.listing_snapshot(), snapshot);
        }
        release.send(()).unwrap();
        let (result, projected) = reader.join().unwrap();
        assert_eq!(result.unwrap(), expected);
        assert_eq!(
            projected, 2,
            "one page row and one independently selected row"
        );
    });
}

fn reopen(backend: &EngineBackend, root: &std::path::Path) {
    let spare_root = crate::tests::private_tempdir();
    let spare = Engine::initialize(
        &spare_root.path().join("system"),
        Config::development([spare_root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    backend.test_with_engine_locked(|engine| {
        drop(std::mem::replace(engine, spare));
        *engine = Engine::open(&root.join("system")).unwrap();
    });
}

#[test]
fn same_snapshot_after_engine_reopen_rejects_namespace_and_virtual_repository_candidates() {
    for path in ["Files", ""] {
        let (root, backend, _auth, _token, identity, _repo) = fixture();
        let snapshot = backend.engine.lock().unwrap().listing_snapshot().to_owned();
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                LISTED_ENTRY_PROJECTIONS.set(0);
                let result = read(
                    backend.as_ref(),
                    &identity,
                    request(path, &[("limit", "1")]),
                );
                (result, LISTED_ENTRY_PROJECTIONS.get())
            });
            observed.recv_timeout(Duration::from_secs(10)).unwrap();
            reopen(&backend, root.path());
            assert_eq!(backend.engine.lock().unwrap().listing_snapshot(), snapshot);
            release.send(()).unwrap();
            let (result, projected) = reader.join().unwrap();
            assert_eq!(
                result.unwrap_err().code,
                storage_contract::ErrorCode::ListingChanged
            );
            assert_eq!(projected, 0);
        });
    }
}

#[test]
fn primary_capture_keeps_cursor_error_precedence_before_disk_and_directory_errors() {
    let (_root, backend, _auth, _token, identity, repo) = fixture();
    let page = read(
        backend.as_ref(),
        &identity,
        request("Files/Café", &[("limit", "1")]),
    )
    .unwrap();
    let cursor = page["next_cursor"].as_str().unwrap();
    let mismatched = crate::ReadListing::parse(
        "missing".into(),
        [
            ("path".into(), "Files/Café".into()),
            ("limit".into(), "1".into()),
            ("cursor".into(), cursor.into()),
        ]
        .into(),
    )
    .unwrap();
    assert_eq!(
        read(backend.as_ref(), &identity, mismatched)
            .unwrap_err()
            .message,
        "cursor does not match the listing query"
    );
    backend.test_with_engine_locked(|engine| {
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::DeleteTree,
                    "Café",
                    None,
                )
                .unwrap(),
                "owner",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    });
    BEFORE_SCAN
        .with(|hook| *hook.borrow_mut() = Some(Box::new(|_| panic!("stale cursor entered scan"))));
    let stale = read(
        backend.as_ref(),
        &identity,
        request("Files/Café", &[("limit", "1"), ("cursor", cursor)]),
    )
    .unwrap_err();
    BEFORE_SCAN.with(|hook| hook.borrow_mut().take());
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
    assert_eq!(
        read(backend.as_ref(), &identity, request("Files/Café", &[]))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::DirectoryNotFound
    );
}

#[test]
fn primary_scan_preserves_virtual_root_recursive_counts_unicode_and_directory_choices() {
    let (_root, backend, _auth, _token, identity, repo) = fixture();
    backend.test_with_engine_locked(|engine| {
        checkpoint(engine, &repo, "", "owner");
        checkpoint(engine, &repo, "Café", "owner");
    });
    let root = read(backend.as_ref(), &identity, request("", &[])).unwrap();
    assert_eq!(root["entries"][0]["entry_id"], repo.repository_id);
    assert_eq!(root["entries"][0]["name"], "Files");
    assert!(root["context"].is_null());
    assert_eq!(root["directory_branches_supported"], false);
    let top = read(backend.as_ref(), &identity, request("FILES", &[])).unwrap();
    assert_eq!(top["total_entries"], "4");
    assert_eq!(top["directory_file_count"], "5");
    assert_eq!(top["default_branch_scope"], true);
    assert_eq!(top["directory_branches_supported"], true);
    assert!(!top["directory_branches"].as_array().unwrap().is_empty());
    let directory = read(
        backend.as_ref(),
        &identity,
        request("FILES/CAFE\u{301}", &[]),
    )
    .unwrap();
    assert_eq!(directory["total_entries"], "2");
    assert_eq!(directory["directory_file_count"], "2");
    assert_eq!(directory["default_branch_scope"], false);
    assert!(
        directory["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| { entry["path"].as_str().unwrap().starts_with("Files/Café/") })
    );
    assert!(
        directory["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["current"] == true)
    );
}

#[tokio::test]
async fn canceled_primary_listing_retains_existing_slot_auth_and_view_until_worker_exits() {
    let (root, backend, auth, _token, identity, repo) = fixture();
    // The scan owns its exact namespace, not an unrelated historical commit table
    // or version records. Cancellation still retains the worker and its permit.
    let (old_record, old_version, required_head) = backend.test_with_engine_locked(|engine| {
        let other = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Other").unwrap(),
                "P01",
                1,
                "owner",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let head = &engine.state().branches[&other.main_branch_id].head_commit_id;
        (
            Arc::downgrade(&engine.state().commits.get_shared(head).unwrap()),
            Arc::downgrade(engine.state().versions.shared_values().next().unwrap()),
            Arc::downgrade(
                &engine
                    .state()
                    .commits
                    .get_shared(&engine.state().branches[&repo.main_branch_id].head_commit_id)
                    .unwrap(),
            ),
        )
    });
    let spare = Engine::initialize(
        &root.path().join("replacement-system"),
        Config::development([root.path().join("replacement-disk")]).unwrap(),
    )
    .unwrap();
    let auth = Arc::new(Mutex::new(auth));
    let auth_owner = Arc::downgrade(&auth);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let wait = crate::http_job::HttpBackendJob::new(
        backend.clone(),
        auth,
        slots.clone().try_acquire_owned().unwrap(),
    )
    .run(move |backend| {
        pause(started, resume);
        read(backend, &identity, request("Files", &[("limit", "1")]))
    });
    let waiter = tokio::spawn(wait);
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    backend.test_with_engine_locked(|engine| *engine = spare);
    assert!(old_record.upgrade().is_none());
    assert!(old_version.upgrade().is_none());
    assert!(required_head.upgrade().is_some());
    assert!(auth_owner.upgrade().is_some());
    assert_eq!(slots.available_permits(), 0);
    assert!(slots.clone().try_acquire_owned().is_err());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while auth_owner.upgrade().is_some() || slots.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(old_record.upgrade().is_none());
    assert!(required_head.upgrade().is_none());
    assert_eq!(slots.available_permits(), 1);
}

#[test]
fn listing_projection_shares_only_required_tables_and_exact_namespace_owners() {
    let (_root, backend, auth, _token, _identity, repo) = fixture();
    let (mut state, view) = {
        let engine = backend.engine.lock().unwrap();
        (
            engine.state().clone(),
            ListingMetadata::capture(&engine, "P01", true).unwrap(),
        )
    };
    drop(auth);
    drop(backend);
    let versions = std::ptr::from_ref(&*state.versions);
    let placements = std::ptr::from_ref(&*state.placements);
    let content_states = std::ptr::from_ref(&*state.content_states);
    let branches = std::ptr::from_ref(&*state.branches);
    let commits = std::ptr::from_ref(&*state.commits);
    assert!(std::ptr::eq(&*view.repositories, &*state.repositories));
    assert_eq!(view.namespaces.len(), 1);
    assert_eq!(
        view.namespaces[&repo.repository_id].branch_id(),
        repo.main_branch_id
    );
    assert!(std::ptr::eq(
        &*view.directory_branches,
        &*state.directory_branches
    ));
    assert!(std::ptr::eq(
        &*view.directory_selections,
        &*state.directory_selections
    ));
    state.versions.extend_shared(&Default::default());
    state.placements.clear();
    state.content_states.clear();
    state.branches.clear();
    state.commits.extend_shared(&Default::default());
    assert_eq!(versions, std::ptr::from_ref(&*state.versions));
    assert_eq!(placements, std::ptr::from_ref(&*state.placements));
    assert_eq!(content_states, std::ptr::from_ref(&*state.content_states));
    assert_eq!(branches, std::ptr::from_ref(&*state.branches));
    assert_eq!(commits, std::ptr::from_ref(&*state.commits));
    state
        .directory_selections
        .insert("new-test-directory".into(), "new-selection".into());
    assert!(!view.directory_selections.contains_key("new-test-directory"));
    assert!(!std::ptr::eq(
        &*view.directory_selections,
        &*state.directory_selections
    ));
}

fn preview_fixture() -> (Fixture, String, String, Value) {
    let fixture = fixture();
    let (_, backend, _, _, _, repo) = &fixture;
    let mut engine = backend.engine.lock().unwrap();
    checkpoint(&mut engine, repo, "Café", "owner");
    let branch = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("Café").unwrap()),
            &storage_contract::BranchName::parse("frozen").unwrap(),
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let tree = engine.state().directory_branches[&branch]
        .head_tree_id
        .clone();
    let frozen = json_codec::to_value(&engine.state().directory_branches[&branch]).unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "Café/child.txt",
                None,
            )
            .unwrap(),
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    (fixture, branch, tree, frozen)
}

#[test]
fn branch_preview_lists_frozen_files_without_changing_namespace_or_receipts() {
    let ((_root, backend, _auth, _token, identity, repo), branch, tree, frozen) = preview_fixture();
    let before = storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
    let page = read(
        backend.as_ref(),
        &identity,
        request(
            "Files/CAFÉ",
            &[
                ("directory_branch_id", &branch),
                ("expected_tree_id", &tree),
                ("limit", "1"),
            ],
        ),
    )
    .unwrap();
    assert_eq!(page["preview"], true);
    assert_eq!(page["writable"], false);
    assert_eq!(page["entries"][0]["name"], "child.txt");
    assert_eq!(page["entries"][0]["read_only"], true);
    assert_eq!(
        page["entries"][0]["version_id"],
        frozen["entries"][0]["version_id"]
    );
    assert_eq!(page["context"]["repository_id"], repo.repository_id);
    let cursor = page["next_cursor"].as_str().unwrap();
    let second = read(
        backend.as_ref(),
        &identity,
        request(
            "Files/CAFÉ",
            &[
                ("directory_branch_id", &branch),
                ("expected_tree_id", &tree),
                ("limit", "1"),
                ("cursor", cursor),
            ],
        ),
    )
    .unwrap();
    assert_eq!(second["entries"][0]["name"], "z.txt");
    assert!(
        read(
            backend.as_ref(),
            &identity,
            request("Files/CAFÉ", &[("limit", "1"), ("cursor", cursor)])
        )
        .is_err()
    );
    assert!(
        read(
            backend.as_ref(),
            &identity,
            request(
                "Files",
                &[
                    ("directory_branch_id", &branch),
                    ("expected_tree_id", &tree)
                ]
            )
        )
        .is_err()
    );
    assert_eq!(
        before,
        storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap()
    );
    let default = read(backend.as_ref(), &identity, request("Files/Café", &[])).unwrap();
    assert_eq!(default["entries"].as_array().unwrap().len(), 1);
    assert_eq!(default["entries"][0]["name"], "z.txt");
}

#[test]
fn preview_rechecks_auth_and_exact_tree_after_detached_scan() {
    let ((_root, backend, mut auth, token, identity, repo), branch, tree, _) = preview_fixture();
    let parsed = request(
        "Files/Café",
        &[
            ("directory_branch_id", &branch),
            ("expected_tree_id", &tree),
        ],
    );
    let captured = capture(&backend.engine.lock().unwrap(), parsed).unwrap();
    let ListingRead::Primary(captured) = captured else {
        panic!("expected primary preview")
    };
    let selected = captured.select().unwrap();
    backend
        .engine
        .lock()
        .unwrap()
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "Café/z.txt",
                None,
            )
            .unwrap(),
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        selected
            .project(&backend.engine.lock().unwrap())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ListingChanged
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        read(
            backend.as_ref(),
            &identity,
            request(
                "Files/Café",
                &[
                    ("directory_branch_id", &branch),
                    ("expected_tree_id", &tree)
                ]
            )
        )
        .unwrap_err()
        .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
