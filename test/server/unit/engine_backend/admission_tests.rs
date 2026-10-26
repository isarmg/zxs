use super::*;
use json_codec::{Value, json};
use std::sync::atomic::Ordering;
use storage_engine::{Config, new_id};

fn fixture() -> (tempfile::TempDir, AdmissionEngine) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("disk1"),
        temp.path().join("disk2"),
        temp.path().join("disk3"),
    ]);
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    (temp, AdmissionEngine::new(engine))
}

fn caller() -> Identity {
    Identity {
        admission_session: None,
        smb_binding: None,
        principal: "owner".into(),
    }
}

fn snapshot(session_count: usize) -> Value {
    static SINGLETON: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let mut value = SINGLETON
        .get_or_init(|| {
            let root = crate::tests::private_tempdir();
            crate::auth::AuthStore::initialize(root.path(), "owner", "integration-test-password")
                .unwrap()
                .snapshot()
                .unwrap()
        })
        .clone();
    for n in 0..session_count {
        value["sessions"][format!("{n:064x}")] =
            json!({"generation":"1","expires_at":"18446744073709551615"});
    }
    value
}

fn publish(engine: &mut AdmissionEngine, value: Value) {
    engine
        .persist_auth_state(value, "system_auth", &new_id("op"))
        .unwrap();
}

#[test]
fn repeated_reads_validate_once_for_empty_and_full_session_indexes() {
    let (_root, mut engine) = fixture();
    for size in [0, 32, 64] {
        publish(&mut engine, snapshot(size));
        let before = engine.validations.load(Ordering::Relaxed);
        for _ in 0..100 {
            let identity = engine.identity(&caller()).unwrap();
            assert_eq!(identity.principal, "owner");
        }
        assert_eq!(engine.validations.load(Ordering::Relaxed) - before, 1);
        // A real immutable engine operation must not accidentally clear it.
        let _ = engine.state().sequence;
        assert!(engine.admission.get().is_some());
    }
}

#[test]
fn all_mutable_access_invalidates_before_a_borrow_can_change_or_replace_engine() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    engine.identity(&caller()).unwrap();
    assert!(engine.admission.get().is_some());
    let _mutable_engine: &mut Engine = &mut engine;
    assert!(engine.admission.get().is_none());
    engine.identity(&caller()).unwrap();
    let mut revoked = snapshot(0);
    revoked["administrator"]["username"] = json!("replacement");
    publish(&mut engine, revoked);
    assert!(engine.admission.get().is_none());
    assert_eq!(
        engine.identity(&caller()).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn absent_and_replaced_principals_fail_without_trusting_the_caller() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    let mut absent = caller();
    absent.principal = "not-in-store".into();
    assert_eq!(engine.identity(&absent).unwrap_err().status, 401);
    let mut value = snapshot(0);
    value["administrator"]["username"] = json!("replacement");
    publish(&mut engine, value);
    assert_eq!(engine.identity(&caller()).unwrap_err().status, 401);
}

#[test]
fn malformed_singleton_sessions_and_top_level_fields_fail_closed() {
    let (_root, mut engine) = fixture();
    for location in [0, 1, 2, 3] {
        publish(&mut engine, snapshot(1));
        engine.identity(&caller()).unwrap();
        let mut invalid = snapshot(1);
        match location {
            0 => invalid["administrator"]["generation"] = json!("0"),
            1 => invalid["sessions"][format!("{:064x}", 0)]["unknown"] = json!(true),
            2 => {
                invalid["sessions"][format!("{:064x}", 0)]["expires_at"] =
                    json!("18446744073709551616")
            }
            _ => invalid["unknown"] = json!(true),
        }
        publish(&mut engine, invalid);
        let before = engine.validations.load(Ordering::Relaxed);
        for _ in 0..2 {
            assert_eq!(
                engine.identity(&caller()).unwrap_err().code,
                storage_contract::ErrorCode::InvalidRequest
            );
        }
        assert_eq!(engine.validations.load(Ordering::Relaxed) - before, 1);
    }
    publish(&mut engine, snapshot(0));
    assert!(engine.identity(&caller()).is_ok());
}

#[test]
fn rejected_publication_drops_projection_and_revalidates_unchanged_authority() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    engine.identity(&caller()).unwrap();
    let before = engine.validations.load(Ordering::Relaxed);
    assert!(
        engine
            .persist_auth_state(json!(false), "system_auth", &new_id("op"))
            .is_err()
    );
    assert!(engine.admission.get().is_none());
    assert_eq!(engine.identity(&caller()).unwrap().principal, "owner");
    assert_eq!(engine.validations.load(Ordering::Relaxed), before + 1);
}

#[test]
fn interrupted_revocation_is_revalidated_from_recovered_authority() {
    let (root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    engine.identity(&caller()).unwrap();
    let mut revoked = snapshot(0);
    revoked["administrator"]["username"] = json!("replacement");
    engine.inject_fault_once("after_publication_1");
    // Warm again after the test-only mutable fault injection.
    engine.identity(&caller()).unwrap();
    assert!(
        engine
            .persist_auth_state(revoked, "system_auth", &new_id("op"))
            .is_err()
    );
    assert!(engine.admission.get().is_none());
    drop(engine);
    let engine = AdmissionEngine::new(Engine::open(&root.path().join("system")).unwrap());
    assert!(engine.admission.get().is_none());
    assert_eq!(engine.identity(&caller()).unwrap_err().status, 401);
}

#[test]
fn embedded_identity_is_rejected_until_an_authoritative_snapshot_is_published() {
    let (_root, mut engine) = fixture();
    assert_eq!(
        engine.identity(&caller()).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    publish(&mut engine, snapshot(0));
    assert_eq!(engine.identity(&caller()).unwrap().principal, "owner");
}

#[test]
fn failed_degraded_recovery_does_not_keep_or_forge_an_admission_projection() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    engine.identity(&caller()).unwrap();
    let before = engine.validations.load(Ordering::Relaxed);
    assert_eq!(
        engine
            .commit_degraded_recovery(&new_id("recovery"), "missing-digest", "owner", false)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryPlanNotFound
    );
    assert!(engine.admission.get().is_none());
    assert_eq!(engine.identity(&caller()).unwrap().principal, "owner");
    assert_eq!(engine.validations.load(Ordering::Relaxed), before + 1);
}

#[test]
fn replacing_engine_through_mutable_reference_cannot_reuse_an_old_projection() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    engine.identity(&caller()).unwrap();
    let (recovered_root, mut recovered) = fixture();
    let mut revoked = snapshot(0);
    revoked["administrator"]["username"] = json!("replacement");
    publish(&mut recovered, revoked);
    drop(recovered);
    let replacement = Engine::open(&recovered_root.path().join("system")).unwrap();
    drop(std::mem::replace(&mut *engine, replacement));
    assert!(engine.admission.get().is_none());
    assert_eq!(engine.identity(&caller()).unwrap_err().status, 401);
}

// Exercise the real dispatch boundary: helper-only tests cannot detect accidental
// mutable coercions in handlers that are logically read-only.
fn backend_fixture() -> (
    tempfile::TempDir,
    crate::engine_backend::EngineBackend,
    storage_contract::Repository,
    storage_contract::Entry,
) {
    let (root, mut holder) = fixture();
    let repo = holder
        .create_repository("ReadFixture", "P01", 3, "owner", &new_id("op"))
        .unwrap();
    let workspace = holder
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    holder
        .stage_bytes(&workspace.workspace_id, "file.txt", b"content")
        .unwrap();
    holder
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "read admission fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = holder.list_tree(&repo.main_branch_id).unwrap().remove(0);
    publish(&mut holder, snapshot(64));
    // Tests live in the holder's private child module; no production raw-engine
    // accessor is added just to construct a backend fixture.
    (
        root,
        crate::engine_backend::EngineBackend::new(holder.engine),
        repo,
        entry,
    )
}

fn read_requests(
    repo: &storage_contract::Repository,
    entry: &storage_contract::Entry,
) -> Vec<(crate::BackendAction, Value)> {
    use crate::BackendAction::*;
    vec![
        (Repositories, json!({})),
        (Tree, json!({"branch_id":repo.main_branch_id})),
        (
            FileManagerEntries,
            json!({"slot_id":"P01","path":repo.name}),
        ),
        (History, json!({"file_id":entry.file_id})),
        (FileManagerHistory, json!({})),
        (FileManagerFileHistory, json!({"file_id":entry.file_id})),
        (
            FileManagerBackupConfiguration,
            json!({"repository_id":repo.repository_id}),
        ),
        (
            FileManagerDownloadContext,
            json!({"repository_id":repo.repository_id,"path":entry.path,"version_id":entry.version_id}),
        ),
    ]
}

#[test]
fn backend_read_requests_reuse_validation_and_completed_revocation_denies_each_route() {
    use crate::Backend;
    let (_root, backend, repo, entry) = backend_fixture();
    for _ in 0..3 {
        for (action, input) in read_requests(&repo, &entry) {
            backend.call(action, &caller(), input).unwrap();
        }
    }
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        1
    );
    let mut revoked = snapshot(64);
    revoked["administrator"]["username"] = json!("replacement");
    publish(&mut backend.engine.lock().unwrap(), revoked);
    for (action, input) in read_requests(&repo, &entry) {
        assert_eq!(
            backend.call(action, &caller(), input).unwrap_err().status,
            401
        );
    }
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        2
    );
}

#[test]
fn backend_missing_and_denied_reads_retain_projection_without_lazy_creation() {
    use crate::{Backend, BackendAction::*};
    let (_root, backend, repo, entry) = backend_fixture();
    let before = json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap();
    for _ in 0..3 {
        for (action, input) in [
            (
                FileManagerEntries,
                json!({"slot_id":"P01","path":"missing"}),
            ),
            (FileManagerEntries, json!({"slot_id":"missing","path":""})),
            (Tree, json!({"branch_id":"missing"})),
            (
                FileManagerBackupConfiguration,
                json!({"repository_id":"missing"}),
            ),
        ] {
            assert!(backend.call(action, &caller(), input).is_err());
        }
    }
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap(),
        before
    );
    // A new sole administrator invalidates the old principal across all reads.
    let mut replaced = snapshot(0);
    replaced["administrator"]["username"] = json!("replacement");
    publish(&mut backend.engine.lock().unwrap(), replaced);
    for _ in 0..3 {
        for (action, input) in read_requests(&repo, &entry) {
            assert_eq!(
                backend.call(action, &caller(), input).unwrap_err().code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
    }
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        2
    );
}

#[test]
fn backend_mutations_and_lazy_root_creation_invalidate_before_following_reads() {
    use crate::{Backend, BackendAction::*};
    let (_root, backend, repo, entry) = backend_fixture();
    backend.call(Repositories, &caller(), json!({})).unwrap();
    backend
        .call(
            FileManagerFileMetadata,
            &caller(),
            json!({
                "file_id":entry.file_id,"note":"updated","operation_id":new_id("op")
            }),
        )
        .unwrap();
    assert!(backend.engine.lock().unwrap().admission.get().is_none());
    backend
        .call(Tree, &caller(), json!({"branch_id":repo.main_branch_id}))
        .unwrap();
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        2
    );
    assert!(
        !backend
            .engine
            .lock()
            .unwrap()
            .state()
            .repositories
            .values()
            .any(|r| r.file_manager_root)
    );
    backend
        .call(
            FileManagerNamespace,
            &caller(),
            json!({
                "slot_id":"P01","path":"new-folder","kind":"mkdir","operation_id":new_id("op")
            }),
        )
        .unwrap();
    assert!(backend.engine.lock().unwrap().admission.get().is_none());
    assert!(
        backend
            .engine
            .lock()
            .unwrap()
            .state()
            .repositories
            .values()
            .any(|r| r.file_manager_root)
    );
    backend
        .call(
            FileManagerEntries,
            &caller(),
            json!({"slot_id":"P01","path":"new-folder"}),
        )
        .unwrap();
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .validations
            .load(Ordering::Relaxed),
        3
    );
}

#[test]
fn protected_snapshot_refresh_preserves_the_internal_smb_binding_constraint() {
    let (_root, mut engine) = fixture();
    publish(&mut engine, snapshot(0));
    let mut caller = caller();
    caller.smb_binding = Some(crate::auth::SmbBinding {
        repository_id: "repository-one".into(),
        branch_id: "branch-one".into(),
        generation: 7,
    });
    let refreshed = engine.identity(&caller).unwrap();
    assert_eq!(refreshed.principal, "owner");
    assert_eq!(refreshed.smb_binding, caller.smb_binding);
    let serialized = json_codec::to_value(&refreshed).unwrap();
    assert!(serialized.get("smb_binding").is_none());
}

#[test]
fn file_manager_borrowed_tree_reads_preserve_listing_and_path_context() {
    use crate::{Backend, BackendAction};
    let (_root, backend, repo, entry) = backend_fixture();
    let listing = backend
        .call(
            BackendAction::FileManagerEntries,
            &caller(),
            json!({"slot_id":"P01","path":repo.name}),
        )
        .unwrap();
    assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
    assert_eq!(listing["entries"][0]["entry_id"], entry.entry_id);
    assert_eq!(listing["entries"][0]["repository_path"], entry.path());
    let history = backend
        .call(
            BackendAction::FileManagerFileHistory,
            &caller(),
            json!({"file_id":entry.file_id}),
        )
        .unwrap();
    let contextual = backend
        .call(
            BackendAction::FileManagerFileHistory,
            &caller(),
            json!({"file_id":entry.file_id,"query":{"repository_id":repo.repository_id,"path":entry.path}}),
        )
        .unwrap();
    assert_eq!(contextual, history);
    assert_eq!(history["path"], entry.path());
    assert_eq!(history["versions"].as_array().unwrap().len(), 1);
    let missing = backend
        .call(
            BackendAction::FileManagerFileHistory,
            &caller(),
            json!({"file_id":entry.file_id,"query":{"repository_id":repo.repository_id,"path":"missing.txt"}}),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::FileNotFound);
    let missing_directory = backend
        .call(
            BackendAction::FileManagerEntries,
            &caller(),
            json!({"slot_id":"P01","path":format!("{}/missing",repo.name)}),
        )
        .unwrap_err();
    assert_eq!(
        missing_directory.code,
        storage_contract::ErrorCode::DirectoryNotFound
    );
}
