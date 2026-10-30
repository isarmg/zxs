//! Deterministic interleavings at the real metadata capture/projection boundary.
use crate::{
    Backend,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use json_codec::Value;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use storage_contract::{Config, Entry, OperationId, Repository};
use storage_engine::{CommitRequest, Engine, MetadataView};

type CaptureHook = Box<dyn FnOnce(&MetadataView)>;
thread_local! {
    static AFTER_CAPTURE: RefCell<Option<CaptureHook>> = const { RefCell::new(None) };
}

pub(crate) fn after_capture(view: &MetadataView) {
    let hook = AFTER_CAPTURE.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook(view);
    }
}

fn pause(started: mpsc::SyncSender<u64>, release: mpsc::Receiver<()>) {
    AFTER_CAPTURE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |view| {
            started.send(view.state().sequence).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
    });
}

fn fixture() -> (tempfile::TempDir, Engine, Repository, Entry) {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("History").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "before.txt", b"original")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "original".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    (root, engine, repo, entry)
}

fn authenticated(
    engine: Engine,
    root: &std::path::Path,
) -> (Arc<EngineBackend>, AuthStore, String, Identity) {
    let backend = Arc::new(EngineBackend::new(engine));
    let mut auth = backend
        .protect_auth(AuthStore::initialize(root, "owner", "integration-test-password").unwrap())
        .unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    (backend, auth, token, identity)
}

#[derive(Clone, Copy)]
enum Query {
    File,
    Graph,
}

impl Query {
    fn read(
        self,
        backend: &dyn Backend,
        identity: &Identity,
        repo: &Repository,
        entry: &Entry,
    ) -> Result<Value, crate::ApiError> {
        match self {
            Self::File => backend
                .file_history_snapshot(
                    identity,
                    crate::ReadFileHistory::parse(entry.file_id.clone(), None, None).unwrap(),
                )
                .map(|snapshot| {
                    json_codec::to_value(crate::backend_file_history::FileHistoryResponse::from(
                        snapshot,
                    ))
                    .unwrap()
                }),
            Self::Graph => backend
                .history_graph(identity, &repo.history_tree_id)
                .map(|snapshot| {
                    json_codec::to_value(crate::backend_graph::HistoryGraphResponse::from(snapshot))
                        .unwrap()
                }),
        }
    }
}

#[test]
fn writers_progress_while_both_history_projections_keep_one_generation() {
    for query in [Query::File, Query::Graph] {
        let (root, engine, repo, entry) = fixture();
        let (backend, _auth, _token, identity) = authenticated(engine, root.path());
        let expected = query
            .read(backend.as_ref(), &identity, &repo, &entry)
            .unwrap();
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                query.read(backend.as_ref(), &identity, &repo, &entry)
            });
            let captured = observed.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut writer = backend
                .engine
                .try_lock()
                .expect("projection retained Engine");
            writer
                .file_namespace(
                    &repo.repository_id,
                    storage_contract::NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Rename,
                        "before.txt",
                        Some("after.txt"),
                    )
                    .unwrap(),
                    "owner",
                    OperationId::generate(),
                )
                .unwrap();
            writer
                .set_file_note(
                    &entry.file_id,
                    None,
                    &storage_contract::Note::parse("later note".into()).unwrap(),
                    "owner",
                    OperationId::generate(),
                )
                .unwrap();
            assert!(writer.state().sequence > captured);
            drop(writer);
            release.send(()).unwrap();
            assert_eq!(reader.join().unwrap().unwrap(), expected);
        });
        assert_ne!(
            query
                .read(backend.as_ref(), &identity, &repo, &entry)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn retained_file_history_preserves_aliases_merges_pending_sources_paths_and_tags() {
    let (root, mut engine, repo, entry) = fixture();
    let branches = ["left", "right", "pending"].map(|name| {
        engine
            .create_file_branch(
                &entry.file_id,
                &entry.version_id,
                &storage_contract::BranchName::parse(name).unwrap(),
                "owner",
                OperationId::generate(),
            )
            .unwrap()
            .result["branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    engine
        .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
            file_id: &entry.file_id,
            targets: &[storage_contract::FileTagTarget {
                branch_id: branches[0].clone(),
                source_node_id: entry.version_id.clone(),
                expected_node_id: entry.version_id.clone(),
            }],
            add: &["review".into()],
            remove: &[],
            location: None,
            principal: "owner",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    let inputs = branches[..2]
        .iter()
        .map(|branch| storage_contract::FileMergeInput {
            branch_id: branch.clone(),
            expected_node_id: engine.file_branch_head(branch).unwrap(),
        })
        .collect::<Vec<_>>();
    engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: &inputs[0].expected_node_id,
            surviving_branch_id: &branches[0],
            location: None,
            principal: "owner",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    engine
        .select_file_branch(
            &entry.file_id,
            &branches[2],
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "before.txt",
                Some("after.txt"),
            )
            .unwrap(),
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let request = || crate::ReadFileHistory::parse(entry.file_id.clone(), None, None).unwrap();
    let captured =
        crate::engine_backend::file_manager::FileHistoryRead::capture(&engine, request()).unwrap();
    let expected = json_codec::to_value(crate::backend_file_history::FileHistoryResponse::from(
        captured.project().unwrap(),
    ))
    .unwrap();
    assert_eq!(expected["path"], "after.txt");
    assert_eq!(expected["source_version_id"], entry.version_id);
    assert_eq!(expected["content_state"], "PENDING");
    assert_eq!(expected["merges"].as_array().unwrap().len(), 1);
    let alias = expected["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == branches[1])
        .unwrap();
    assert_eq!(alias["alias_of"], branches[0]);
    assert!(expected["nodes"].as_array().unwrap().iter().any(|node| {
        node["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == "review")
    }));
    let (backend, _auth, _token, identity) = authenticated(engine, root.path());
    assert_eq!(
        Query::File
            .read(backend.as_ref(), &identity, &repo, &entry)
            .unwrap(),
        expected
    );
    backend.test_with_engine_locked(|engine| {
        engine
            .set_file_note(
                &entry.file_id,
                None,
                &storage_contract::Note::parse("later note".into()).unwrap(),
                "owner",
                OperationId::generate(),
            )
            .unwrap();
    });
    assert_eq!(
        json_codec::to_value(crate::backend_file_history::FileHistoryResponse::from(
            captured.project().unwrap(),
        ))
        .unwrap(),
        expected
    );
}

#[test]
fn both_history_reads_recheck_revocation_after_capture() {
    for query in [Query::File, Query::Graph] {
        let (root, engine, repo, entry) = fixture();
        let (backend, mut auth, token, identity) = authenticated(engine, root.path());
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                query.read(backend.as_ref(), &identity, &repo, &entry)
            });
            observed.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(
                backend
                    .engine
                    .try_lock()
                    .expect("projection retained Engine"),
            );
            auth.logout(&token).unwrap();
            release.send(()).unwrap();
            assert_eq!(
                reader.join().unwrap().unwrap_err().code,
                storage_contract::ErrorCode::Unauthorized
            );
        });
    }
}

#[test]
fn scoped_file_history_rechecks_deleted_path_but_historical_file_remains_readable() {
    let (root, engine, repo, entry) = fixture();
    let (backend, _auth, _token, identity) = authenticated(engine, root.path());
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pause(started, resume);
            backend.file_history_snapshot(
                &identity,
                crate::ReadFileHistory::parse(
                    entry.file_id.clone(),
                    Some(repo.repository_id.clone()),
                    Some("before.txt".into()),
                )
                .unwrap(),
            )
        });
        observed.recv_timeout(Duration::from_secs(10)).unwrap();
        backend
            .engine
            .try_lock()
            .expect("projection retained Engine")
            .file_namespace(
                &repo.repository_id,
                storage_contract::NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Delete,
                    "before.txt",
                    None,
                )
                .unwrap(),
                "owner",
                OperationId::generate(),
            )
            .unwrap();
        release.send(()).unwrap();
        assert_eq!(
            reader.join().unwrap().unwrap_err().code,
            storage_contract::ErrorCode::FileNotFound
        );
    });
    let historical = Query::File
        .read(backend.as_ref(), &identity, &repo, &entry)
        .unwrap();
    assert!(historical["path"].is_null());
    assert!(!historical["versions"].as_array().unwrap().is_empty());
}

#[test]
fn both_history_reads_recheck_actual_split_scope() {
    for query in [Query::File, Query::Graph] {
        let (root, engine, repo, entry) = fixture();
        let cut = engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let (backend, _auth, _token, identity) = authenticated(engine, root.path());
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                query.read(backend.as_ref(), &identity, &repo, &entry)
            });
            observed.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut writer = backend
                .engine
                .try_lock()
                .expect("projection retained Engine");
            let plan = writer
                .create_split_plan(&repo.history_tree_id, &cut, "owner")
                .unwrap();
            writer
                .commit_plan(&plan.plan_id, "owner", OperationId::generate())
                .unwrap();
            drop(writer);
            release.send(()).unwrap();
            assert_eq!(
                reader.join().unwrap().unwrap_err().code,
                storage_contract::ErrorCode::TreeMoved
            );
        });
    }
}

fn replacement(root: &std::path::Path, auth: &AuthStore) -> Engine {
    let mut engine = Engine::initialize(
        &root.join("replacement-system"),
        Config::development([root.join("replacement-disk")]),
    )
    .unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            OperationId::generate(),
        )
        .unwrap();
    engine
}

#[test]
fn both_reads_reject_a_target_removed_by_engine_replacement() {
    for query in [Query::File, Query::Graph] {
        let (root, engine, repo, entry) = fixture();
        let (backend, auth, _token, identity) = authenticated(engine, root.path());
        let new_engine = replacement(root.path(), &auth);
        let (started, observed) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                pause(started, resume);
                query.read(backend.as_ref(), &identity, &repo, &entry)
            });
            observed.recv_timeout(Duration::from_secs(10)).unwrap();
            backend.test_with_engine_locked(|engine| *engine = new_engine);
            release.send(()).unwrap();
            assert_eq!(
                reader.join().unwrap().unwrap_err().code,
                storage_contract::ErrorCode::NotFound
            );
        });
    }
}

#[tokio::test]
async fn canceled_waiters_retain_history_views_auth_and_the_existing_backend_slot() {
    for query in [Query::File, Query::Graph] {
        let (root, engine, repo, entry) = fixture();
        let old_head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
        let record = engine.state().commits.get_shared(old_head).unwrap();
        let old_record = Arc::downgrade(&record);
        drop(record);
        let (backend, auth, _token, identity) = authenticated(engine, root.path());
        let new_engine = replacement(root.path(), &auth);
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
            query.read(backend, &identity, &repo, &entry)
        });
        let waiter = tokio::spawn(wait);
        observed.recv_timeout(Duration::from_secs(10)).unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        backend.test_with_engine_locked(|engine| *engine = new_engine);
        assert!(
            old_record.upgrade().is_some(),
            "worker must retain the captured State"
        );
        assert!(auth_owner.upgrade().is_some());
        assert_eq!(slots.available_permits(), 0);
        assert!(
            slots.clone().try_acquire_owned().is_err(),
            "old views stay admission-bounded"
        );
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
            "no cross-request view cache"
        );
        assert_eq!(slots.available_permits(), 1);
    }
}
