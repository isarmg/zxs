//! Deterministic interleavings of the backup capture/selection interface.
//! Backend::listing entry-point interleavings require integration tests too.
use super::*;
use crate::{
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use storage_contract::{Config, Entry, OperationId, Repository};
use storage_engine::CommitRequest;

fn fixture() -> (tempfile::TempDir, Engine, Repository, Entry) {
    let root = crate::tests::private_tempdir();
    let config = Config::development([
        root.path().join("primary"),
        root.path().join("backup"),
        root.path().join("cold"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Lifecycle").unwrap(),
            "P01",
            3,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "before.txt", b"retained body")
        .unwrap();
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
    let entry = engine.list_tree_ref(&repo.main_branch_id).unwrap()[0].clone();
    (root, engine, repo, entry)
}

fn capture(engine: &AdmissionEngine) -> BackupListingRead {
    let path = "P01/Lifecycle";
    let input = crate::ReadListing::parse(
        "B01".into(),
        [("path".into(), path.into()), ("limit".into(), "1".into())].into(),
    )
    .unwrap();
    let request = PageRequest::parse(engine, input).unwrap();
    BackupListingRead::capture(engine, "B01", path, request).unwrap()
}

fn authenticated(
    engine: Engine,
    root: &std::path::Path,
) -> (Arc<EngineBackend>, AuthStore, Identity) {
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

fn replacement(root: &std::path::Path, auth: &AuthStore) -> Engine {
    let mut engine = Engine::initialize(
        &root.join("replacement-system"),
        Config::development([root.join("replacement-primary")]),
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
fn writers_progress_between_backup_capture_and_selection_and_stale_candidates_are_rejected() {
    let (_root, engine, repo, _entry) = fixture();
    let shared = Arc::new(Mutex::new(AdmissionEngine::new(engine)));
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let reader_engine = Arc::clone(&shared);
        let reader = scope.spawn(move || {
            let (read, generation) = {
                let engine = reader_engine.lock().unwrap();
                (capture(&engine), engine.state().business_generation)
            };
            started.send(generation).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
            let selected = read.select().unwrap();
            let engine = reader_engine.lock().unwrap();
            selected.project(&engine).err().unwrap().code
        });
        let captured_generation = observed.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut writer = shared
            .try_lock()
            .expect("backup scan preparation retained the Engine mutex");
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
        assert!(writer.state().business_generation > captured_generation);
        drop(writer);
        release.send(()).unwrap();
        assert_eq!(
            reader.join().unwrap(),
            storage_contract::ErrorCode::ListingChanged
        );
    });
}

#[test]
fn backup_projection_rejects_business_changes_without_namespace_changes() {
    let (_root, engine, repo, entry) = fixture();
    let mut engine = AdmissionEngine::new(engine);
    let selected = capture(&engine).select().unwrap();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let before = engine.listing_snapshot().to_owned();
    engine
        .set_file_note(
            &entry.file_id,
            None,
            &storage_contract::Note::parse("changed after capture".into()).unwrap(),
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.state().branches[&repo.main_branch_id].head_commit_id,
        head
    );
    assert_ne!(engine.listing_snapshot(), before);
    assert_eq!(
        selected.project(&engine).err().unwrap().code,
        storage_contract::ErrorCode::ListingChanged
    );
}

#[test]
fn auth_only_publication_preserves_backup_projection_of_the_same_commit_objects() {
    let (root, engine, repo, entry) = fixture();
    let mut engine = AdmissionEngine::new(engine);
    let selected = capture(&engine).select().unwrap();
    let before = engine.listing_snapshot().to_owned();
    let sequence = engine.state().sequence;
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let original = engine.state().commits.get_shared(head).unwrap();
    let auth = AuthStore::initialize(
        &root.path().join("auth"),
        "owner",
        "integration-test-password",
    )
    .unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            OperationId::generate(),
        )
        .unwrap();
    assert!(engine.state().sequence > sequence);
    assert_eq!(engine.listing_snapshot(), before);
    let current_head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let current = engine.state().commits.get_shared(current_head).unwrap();
    assert!(Arc::ptr_eq(&original, &current));
    let projected = selected.project(&engine).unwrap();
    let snapshot = selected.finish(projected);
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].file_id, entry.file_id);
    assert_eq!(snapshot.entries[0].path, "P01/Lifecycle/before.txt");
}

#[test]
fn same_token_reopened_engine_rejects_backup_candidates_from_distinct_commit_objects() {
    let (root, engine, repo, _entry) = fixture();
    let engine = AdmissionEngine::new(engine);
    let snapshot = engine.listing_snapshot().to_owned();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let original = engine.state().commits.get_shared(&head).unwrap();
    let selected = capture(&engine).select().unwrap();
    // A retained MetadataView owns no Engine or disk locks, so the same
    // committed metadata can be reopened while the old candidates survive.
    drop(engine);
    let current = AdmissionEngine::new(Engine::open(&root.path().join("system")).unwrap());
    let reopened = current.state().commits.get_shared(&head).unwrap();
    assert_eq!(current.listing_snapshot(), snapshot);
    assert_eq!(*original, *reopened);
    assert!(!Arc::ptr_eq(&original, &reopened));
    assert_eq!(
        selected.project(&current).err().unwrap().code,
        storage_contract::ErrorCode::ListingChanged
    );
}

#[tokio::test]
async fn canceled_backup_waiter_retains_view_auth_and_existing_backend_slot_until_worker_ends() {
    let (root, engine, repo, _entry) = fixture();
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let record = engine.state().commits.get_shared(head).unwrap();
    let old_record = Arc::downgrade(&record);
    drop(record);
    let (backend, auth, identity) = authenticated(engine, root.path());
    let new_engine = replacement(root.path(), &auth);
    let auth = Arc::new(Mutex::new(auth));
    let auth_owner = Arc::downgrade(&auth);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let (started, observed) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let worker_backend = Arc::clone(&backend);
    let wait = crate::http_job::HttpBackendJob::new(
        backend.clone(),
        auth,
        slots.clone().try_acquire_owned().unwrap(),
    )
    .run(move |_backend| {
        let read = {
            let engine = worker_backend.engine.lock().unwrap();
            crate::engine_backend::current_protected_identity(&engine, &identity)?;
            capture(&engine)
        };
        started.send(()).unwrap();
        resume.recv_timeout(Duration::from_secs(10)).unwrap();
        let selection = read.select();
        let (selected, projected) = {
            let engine = worker_backend.engine.lock().unwrap();
            crate::engine_backend::current_protected_identity(&engine, &identity)?;
            let selected = selection?;
            let projected = selected.project(&engine)?;
            (selected, projected)
        };
        let _snapshot = selected.finish(projected);
        Ok(())
    });
    let waiter = tokio::spawn(wait);
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    backend.test_with_engine_locked(|engine| *engine = new_engine);
    assert!(
        old_record.upgrade().is_some(),
        "worker must retain the old view"
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
        "finished jobs retain no view cache"
    );
    assert_eq!(slots.available_permits(), 1);
}
