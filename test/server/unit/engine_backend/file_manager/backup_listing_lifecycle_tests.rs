//! Backup listing interleavings at the real Backend capture/scan boundary.
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
use storage_contract::{Config, OperationId};
use storage_engine::{BackupConfigurationRequest, CommitRequest};

type ScanHook = Box<dyn FnOnce(&BackupListingMetadata)>;
thread_local! {
    static BEFORE_SCAN: RefCell<Option<ScanHook>> = const { RefCell::new(None) };
}

pub(super) fn before_scan(view: &BackupListingMetadata) {
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

fn save(engine: &mut Engine, repo: &Repository, files: &[(&str, &[u8])]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for (path, body) in files {
        engine
            .stage_bytes(&workspace.workspace_id, path, body)
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "backup lifecycle fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

fn fixture() -> Fixture {
    let root = crate::tests::private_tempdir();
    let config = Config::development([root.path().join("primary"), root.path().join("backup")]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Lifecycle").unwrap(),
            "P01",
            2,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    save(
        &mut engine,
        &repo,
        &[
            ("alpha.txt", b"original alpha"),
            ("beta.txt", b"original beta"),
            ("zeta.txt", b"original zeta"),
        ],
    );
    // Keep a nonempty historical Commit outside every captured current head.
    save(&mut engine, &repo, &[("zeta.txt", b"later zeta")]);
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
    crate::ReadListing::parse("B01".into(), query).unwrap()
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

#[derive(Clone, Copy)]
enum BusinessChange {
    DeleteFirstEntry,
    FileNote,
}

#[test]
fn writers_finish_during_backup_scan_and_changed_pages_never_project_old_indices() {
    for change in [BusinessChange::DeleteFirstEntry, BusinessChange::FileNote] {
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
                    request("P01/Lifecycle", &[("limit", "1"), ("query", "beta")]),
                );
                (result, LISTED_ENTRY_PROJECTIONS.get())
            });
            let captured = observed.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut writer = backend
                .engine
                .try_lock()
                .expect("backup scan retained Engine");
            let head = writer.state().branches[&repo.main_branch_id]
                .head_commit_id
                .clone();
            let tree = writer.namespace_tree(&repo.main_branch_id).unwrap();
            let file = tree.get("beta.txt").unwrap().file_id.clone();
            let index = tree
                .entries()
                .iter()
                .position(|entry| entry.file_id == file)
                .unwrap();
            assert_eq!(index, 1);
            match change {
                BusinessChange::DeleteFirstEntry => {
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
                            OperationId::generate(),
                        )
                        .unwrap();
                    assert_eq!(
                        writer.list_tree_ref(&repo.main_branch_id).unwrap()[0].file_id,
                        file
                    );
                }
                BusinessChange::FileNote => {
                    writer
                        .set_file_note(
                            &file,
                            None,
                            &storage_contract::Note::parse("changed".into()).unwrap(),
                            "owner",
                            OperationId::generate(),
                        )
                        .unwrap();
                    assert_eq!(
                        writer.state().branches[&repo.main_branch_id].head_commit_id,
                        head
                    );
                }
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
                "final checks precede every old-index projection"
            );
        });
    }
}

#[test]
fn revoked_identity_wins_over_a_changed_backup_listing_without_projecting_rows() {
    let (_root, backend, mut auth, token, identity, repo) = fixture();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(
                backend.as_ref(),
                &identity,
                request("P01/Lifecycle", &[("limit", "1")]),
            );
            (result, LISTED_ENTRY_PROJECTIONS.get())
        });
        observed.recv_timeout(Duration::from_secs(10)).unwrap();
        backend
            .engine
            .try_lock()
            .expect("backup scan retained Engine")
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Delete,
                    "alpha.txt",
                    None,
                )
                .unwrap(),
                "owner",
                OperationId::generate(),
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

#[test]
fn revoked_identity_wins_over_global_backup_conflict_discovered_during_scan() {
    let (_root, backend, mut auth, token, identity, repo) = fixture();
    backend.test_with_engine_locked(|engine| {
        let other = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Conflicting").unwrap(),
                "P01",
                2,
                "owner",
                OperationId::generate(),
            )
            .unwrap();
        save(engine, &other, &[("beta.txt", b"independent body")]);
        configure(engine, &repo, "beta.txt", "Collision");
        configure(engine, &other, "beta.txt", "COLLISION");
    });
    assert_eq!(
        read(
            backend.as_ref(),
            &identity,
            request("P01/Lifecycle", &[("limit", "1"), ("query", "alpha")]),
        )
        .unwrap_err()
        .code,
        storage_contract::ErrorCode::BackupPathConflict
    );
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(
                backend.as_ref(),
                &identity,
                request("P01/Lifecycle", &[("limit", "1"), ("query", "alpha")]),
            );
            (result, LISTED_ENTRY_PROJECTIONS.get())
        });
        observed.recv_timeout(Duration::from_secs(10)).unwrap();
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
fn auth_only_publication_keeps_backup_snapshot_selection_and_bounded_projection() {
    let (_root, backend, mut auth, _token, identity, repo) = fixture();
    let all = read(backend.as_ref(), &identity, request("P01/Lifecycle", &[])).unwrap();
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
    let expected = read(
        backend.as_ref(),
        &identity,
        request("P01/Lifecycle", &values),
    )
    .unwrap();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            LISTED_ENTRY_PROJECTIONS.set(0);
            let result = read(
                backend.as_ref(),
                &identity,
                request("P01/Lifecycle", &values),
            );
            (result, LISTED_ENTRY_PROJECTIONS.get())
        });
        let captured = observed.recv_timeout(Duration::from_secs(10)).unwrap();
        let (snapshot, original) = {
            let engine = backend
                .engine
                .try_lock()
                .expect("backup scan retained Engine");
            let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
            (
                engine.listing_snapshot().to_owned(),
                engine.state().commits.get_shared(head).unwrap(),
            )
        };
        auth.login("owner", "integration-test-password").unwrap();
        {
            let engine = backend.engine.lock().unwrap();
            let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
            let current = engine.state().commits.get_shared(head).unwrap();
            assert!(engine.state().sequence > captured);
            assert_eq!(engine.listing_snapshot(), snapshot);
            assert!(Arc::ptr_eq(&original, &current));
        }
        release.send(()).unwrap();
        let (result, projected) = reader.join().unwrap();
        assert_eq!(result.unwrap(), expected);
        assert_eq!(
            projected, 2,
            "one page row and one independent selected row"
        );
    });
}

fn reopen(backend: &EngineBackend, root: &std::path::Path) {
    let spare_root = crate::tests::private_tempdir();
    let spare = Engine::initialize(
        &spare_root.path().join("system"),
        Config::development([spare_root.path().join("primary")]),
    )
    .unwrap();
    backend.test_with_engine_locked(|engine| {
        drop(std::mem::replace(engine, spare));
        *engine = Engine::open(&root.join("system")).unwrap();
    });
}

#[test]
fn same_token_engine_reopen_rejects_backup_file_and_directory_pages_before_projection() {
    for path in ["", "P01/Lifecycle"] {
        let (root, backend, _auth, _token, identity, repo) = fixture();
        let (snapshot, head, original) = {
            let engine = backend.engine.lock().unwrap();
            let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
            (
                engine.listing_snapshot().to_owned(),
                head.clone(),
                engine.state().commits.get_shared(head).unwrap(),
            )
        };
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
            // A retained backup projection holds no Engine or disk lock, so this
            // exact committed generation can reopen with new Commit objects.
            reopen(&backend, root.path());
            {
                let engine = backend.engine.lock().unwrap();
                let current = engine.state().commits.get_shared(&head).unwrap();
                assert_eq!(engine.listing_snapshot(), snapshot);
                assert_eq!(*original, *current);
                assert!(!Arc::ptr_eq(&original, &current));
            }
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

#[tokio::test]
async fn canceled_backup_listing_retains_only_required_records_auth_and_slot_until_worker_exits() {
    let (root, backend, auth, _token, identity, repo) = fixture();
    let (old_record, required_version) = {
        let engine = backend.engine.lock().unwrap();
        let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
        let historical = engine
            .state()
            .commits
            .values()
            .find(|commit| {
                commit.history_tree_id == repo.history_tree_id
                    && commit.commit_id != *head
                    && !commit.entries.is_empty()
            })
            .unwrap();
        // The current namespace Arc does not own this historical record. Its
        // lifetime after replacement detects accidental capture of all commits.
        let version = engine.state().versions.values().next().unwrap();
        (
            Arc::downgrade(
                &engine
                    .state()
                    .commits
                    .get_shared(&historical.commit_id)
                    .unwrap(),
            ),
            Arc::downgrade(
                &engine
                    .state()
                    .versions
                    .get_shared(&version.version_id)
                    .unwrap(),
            ),
        )
    };
    let spare = Engine::initialize(
        &root.path().join("replacement-system"),
        Config::development([root.path().join("replacement-primary")]),
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
        read(
            backend,
            &identity,
            request("P01/Lifecycle", &[("limit", "1")]),
        )
    });
    let waiter = tokio::spawn(wait);
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    backend.test_with_engine_locked(|engine| *engine = spare);
    assert!(
        old_record.upgrade().is_none(),
        "backup worker must not retain unrelated historical commits"
    );
    assert!(
        required_version.upgrade().is_some(),
        "paused worker still owns required version records for global validation"
    );
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
    assert!(
        old_record.upgrade().is_none(),
        "completed jobs retain no view cache"
    );
    assert!(required_version.upgrade().is_none());
    assert_eq!(slots.available_permits(), 1);
}
