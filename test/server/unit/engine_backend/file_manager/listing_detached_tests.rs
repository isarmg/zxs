//! Primary listing interleavings at the real capture/scan boundary.
use super::*;
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

type ScanHook = Box<dyn FnOnce(u64)>;
thread_local! {
    static BEFORE_SCAN: RefCell<Option<ScanHook>> = const { RefCell::new(None) };
}

pub(super) fn before_scan(sequence: u64) {
    let hook = BEFORE_SCAN.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook(sequence);
    }
}

fn pause(started: mpsc::SyncSender<u64>, release: mpsc::Receiver<()>) {
    BEFORE_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |sequence| {
            started.send(sequence).unwrap();
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
        Config::development(crate::test_disks([root.path().join("disk")])).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "owner", OperationId::generate().unwrap())
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
                    request("", &[("limit", "1")]),
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
            let result = read(backend.as_ref(), &identity, request("", &[]));
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
    let all = read(backend.as_ref(), &identity, request("", &[])).unwrap();
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
    let expected = read(backend.as_ref(), &identity, request("", &values)).unwrap();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(backend.as_ref(), &identity, request("", &values));
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
        Config::development(crate::test_disks([spare_root.path().join("disk")])).unwrap(),
    )
    .unwrap();
    backend.test_with_engine_locked(|engine| {
        drop(std::mem::replace(engine, spare));
        *engine = Engine::open(&root.join("system")).unwrap();
    });
}

#[test]
fn same_snapshot_after_engine_reopen_rejects_original_namespace_indices() {
    for path in ["Café", ""] {
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
        request("Café", &[("limit", "1")]),
    )
    .unwrap();
    let cursor = page["next_cursor"].as_str().unwrap();
    let mismatched = crate::ReadListing::parse(
        "missing".into(),
        [
            ("path".into(), "Café".into()),
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
        request("Café", &[("limit", "1"), ("cursor", cursor)]),
    )
    .unwrap_err();
    BEFORE_SCAN.with(|hook| hook.borrow_mut().take());
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
    assert_eq!(
        read(backend.as_ref(), &identity, request("Café", &[]))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::DirectoryNotFound
    );
}

#[test]
fn primary_scan_preserves_counts_unicode_and_the_exact_disk_relative_context() {
    let (_root, backend, _auth, _token, identity, repo) = fixture();
    let top = read(backend.as_ref(), &identity, request("", &[])).unwrap();
    assert_eq!(top["context"]["repository_id"], repo.repository_id);
    assert_eq!(top["context"]["repository_path"], "");
    assert_eq!(top["total_entries"], "4");
    assert_eq!(top["directory_file_count"], "5");
    for old in ["directory_branches", "directory_branches_supported", "default_branch_scope", "preview"] {
        assert!(top.get(old).is_none());
    }
    let directory = read(backend.as_ref(), &identity, request("CAFE\u{301}", &[])).unwrap();
    assert_eq!(directory["context"]["repository_path"], "CAFE\u{301}");
    assert_eq!(directory["total_entries"], "2");
    assert_eq!(directory["directory_file_count"], "2");
    assert!(directory["entries"].as_array().unwrap().iter().all(|entry| {
        entry["path"] == entry["repository_path"]
            && entry["path"].as_str().unwrap().starts_with("Café/")
    }));
}

#[tokio::test]
async fn canceled_primary_listing_retains_existing_slot_auth_and_view_until_worker_exits() {
    let (root, backend, auth, _token, identity, repo) = fixture();
    // The scan owns its exact namespace, not an unrelated historical commit table
    // or version records. Cancellation still retains the worker and its permit.
    let (old_record, old_version, required_head) = backend.test_with_engine_locked(|engine| {
        let current = &engine.state().commits[&engine.state().branches[&repo.main_branch_id].head_commit_id];
        let head = current.parent_commit_ids.first().expect("actual main predecessor");
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
        Config::development(crate::test_disks([root.path().join("replacement-disk")])).unwrap(),
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
        read(backend, &identity, request("", &[("limit", "1")]))
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
fn primary_capture_rejects_an_owner_replacement_after_the_unlocked_scan() {
    let (root, backend, _auth, _token, identity, _repo) = fixture();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            (read(backend.as_ref(), &identity, request("", &[("limit", "1")])),
             LISTED_ENTRY_PROJECTIONS.get())
        });
        let sequence = observed.recv_timeout(Duration::from_secs(10)).unwrap();
        let common = root.path().join("disk/common-data");
        std::fs::rename(&common, root.path().join("original")).unwrap();
        std::fs::create_dir(&common).unwrap();
        assert_eq!(backend.engine.lock().unwrap().state().sequence, sequence);
        release.send(()).unwrap();
        let (result, projected) = reader.join().unwrap();
        assert!(result.is_err());
        assert_eq!(projected, 0);
        assert!(std::fs::read_dir(&common).unwrap().next().is_none());
    });
}
