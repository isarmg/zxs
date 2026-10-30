use super::*;
use crate::{Backend, engine_backend::EngineBackend};
use json_codec::json;
use storage_contract::{Config, NamespaceOperation, new_id};

fn fixture() -> (
    tempfile::TempDir,
    Engine,
    storage_engine::Repository,
    Identity,
) {
    let root = crate::tests::private_tempdir();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Application").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let identity = crate::engine_backend::install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}

#[test]
fn protection_commands_preserve_core_replay_and_transport_contract() {
    for is_policy in [true, false] {
        let (_root, mut engine, repo, owner) = fixture();
        let operation = storage_contract::OperationId::generate();
        let id = new_id(if is_policy { "policy" } else { "rule" });
        let original = if is_policy {
            engine
                .update_protection_policy(storage_engine::ProtectionPolicyUpdate {
                    policy_id: &id,
                    repository_id: &repo.repository_id,
                    path_prefix: "",
                    target_copies: 3,
                    expected_generation: 0,
                    principal: &owner.principal,
                    operation_id: operation,
                })
                .unwrap()
        } else {
            engine
                .update_placement_rule(storage_engine::PlacementRuleUpdate {
                    rule_id: &id,
                    repository_id: &repo.repository_id,
                    path_prefix: "",
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: &owner.principal,
                    operation_id: operation,
                })
                .unwrap()
        };
        let backend = EngineBackend::new(engine);
        let execute = |identity: &Identity, path: &str, generation: u64, operation_id| {
            if is_policy {
                backend.update_protection_policy(
                    identity,
                    crate::UpdateProtectionPolicy {
                        policy_id: id.clone(),
                        repository_id: repo.repository_id.clone(),
                        path_prefix: path.into(),
                        target_copies: 3,
                        expected_generation: generation,
                        operation_id,
                    },
                )
            } else {
                backend.update_placement_rule(
                    identity,
                    crate::UpdatePlacementRule {
                        rule_id: id.clone(),
                        repository_id: repo.repository_id.clone(),
                        path_prefix: path.into(),
                        primary_slot_id: "P01".into(),
                        expected_generation: generation,
                        operation_id,
                    },
                )
            }
        };
        let replay = execute(&owner, "", 0, operation).unwrap();
        assert_eq!(replay, original);
        let wire = crate::operation_wire::operation_wire(replay.into()).unwrap();
        assert_eq!(wire["request_digest"], original.request_digest);
        assert_eq!(wire["result"], original.result);
        assert_eq!(wire["publication_sequence"], original.sequence.to_string());
        assert_eq!(wire["operation_kind"], original.kind);
        assert_eq!(
            execute(&owner, "Other", 0, operation).unwrap_err().code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        // Keep the public Engine ID/path boundary ahead of receipt lookup.
        assert_eq!(
            execute(&owner, "../invalid", 0, operation)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::NameInvalidCharacter
        );
        let stranger = Identity {
            admission_session: None,
            smb_binding: None,
            principal: "stranger".into(),
        };
        assert_eq!(
            execute(&stranger, "", 0, operation).unwrap_err().status,
            401
        );
        let unadmitted_owner = Identity {
            admission_session: Some("unknown-session".into()),
            ..owner.clone()
        };
        assert_eq!(
            execute(
                &unadmitted_owner,
                "",
                1,
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .status,
            401
        );
        assert_eq!(
            backend.engine.lock().unwrap().state().operations[&operation].request_digest,
            original.request_digest
        );
        let applied = execute(&owner, "", 1, storage_contract::OperationId::generate()).unwrap();
        let record = if is_policy { "policy" } else { "rule" };
        assert_eq!(applied.result[record]["generation"], "2");
        let snapshot = if is_policy {
            json_codec::to_value(crate::backend_protection::PolicyResponse::from(
                backend.protection_policy(&owner, &id).unwrap(),
            ))
            .unwrap()
        } else {
            json_codec::to_value(crate::backend_protection::PlacementRuleResponse::from(
                backend.placement_rule(&owner, &id).unwrap(),
            ))
            .unwrap()
        };
        assert_eq!(snapshot, applied.result[record]);
        let stale = execute(&owner, "", 1, storage_contract::OperationId::generate()).unwrap_err();
        assert_eq!(stale.code, storage_contract::ErrorCode::ConfigConflict);
        assert_eq!(stale.status, 409);
    }
}

#[test]
fn public_commit_roundtrip_preserves_canonical_envelope_and_executes() {
    let (root, mut engine, repo, identity) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .unwrap();
    Application::new(&mut engine, &identity)
        .stage_namespace(StageNamespace {
            workspace_id: workspace.workspace_id.clone(),
            change: NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Mkdir,
                "Folder",
                None,
            )
            .unwrap(),
        })
        .unwrap();
    let mut body = engine
        .seal_workspace(&workspace.workspace_id, &identity.principal)
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    body["message"] = "目录 😀".into();
    body["request_digest"] = storage_engine::commit_request_digest(&workspace.workspace_id, &body)
        .unwrap()
        .into();
    let command =
        crate::http_workspace::public_commit(&workspace.workspace_id, body.clone()).unwrap();
    assert_eq!(
        command.claimed_digest(),
        body["request_digest"].as_str().unwrap()
    );
    let result = Application::new(&mut engine, &identity)
        .commit_workspace_public(command.clone())
        .unwrap();
    let replay = Application::new(&mut engine, &identity)
        .commit_workspace_public(command)
        .unwrap();
    assert_eq!(replay.commit_id, result.commit_id);
    let expected = storage_contract::canonical_json(&body).unwrap();
    for parent in [
        root.path().join("system/workspaces"),
        root.path().join("d1/staging/workspaces"),
    ] {
        assert_eq!(
            std::fs::read(
                parent
                    .join(&workspace.workspace_id)
                    .join("wire-request.json")
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        result.request_digest,
        body["request_digest"].as_str().unwrap()
    );

    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|entry| entry.path == "Folder")
    );
}

#[test]
fn workspace_ownership_and_session_admission_apply_to_typed_and_wire_paths() {
    let (_root, mut engine, repo, owner) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &owner.principal)
        .unwrap();
    let intruder = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "intruder".into(),
    };
    let result = Application::new(&mut engine, &intruder).stage_namespace(StageNamespace {
        workspace_id: workspace.workspace_id.clone(),
        change: NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Mkdir,
            "TypedDenied",
            None,
        )
        .unwrap(),
    });
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::Forbidden
    );
    let unadmitted_owner = Identity {
        admission_session: Some("unknown-session".into()),
        ..owner.clone()
    };
    let result = Application::new(&mut engine, &intruder).commit_workspace(CommitWorkspace {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: String::new(),
        request_digest: None,
    });
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::Forbidden
    );
    let backend = EngineBackend::new(engine);
    for identity in [&intruder, &unadmitted_owner] {
        let result = stage_namespace_wire(
            &backend,
            identity,
            json!({"workspace_id":workspace.workspace_id,
                "kind":"mkdir", "path":"WireDenied"}),
        );
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    let engine = backend.engine.lock().unwrap();
    assert!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
}

#[test]
fn public_commit_decode_rejects_unknown_missing_and_numeric_decimal_fields() {
    let body = json!({"workspace_id":"ws", "operation_id":"op_00000000000000000000000000000000", "repository_id":"repo", "branch_id":"branch",
            "expected_head":"head", "fencing_token":"1", "message":"", "requested_protection":"strict",
            "file_placement":{}, "expected_topology_generation":"1", "sealed_manifest_hash":"hash", "request_digest":"digest"});
    assert!(crate::http_workspace::public_commit("ws", body.clone()).is_ok());
    for field in [
        "workspace_id",
        "operation_id",
        "expected_head",
        "message",
        "request_digest",
    ] {
        let mut missing = body.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            crate::http_workspace::public_commit("ws", missing).is_err(),
            "{field}"
        );
    }
    for field in ["fencing_token", "expected_topology_generation"] {
        let mut numeric = body.clone();
        numeric[field] = json!(1);
        assert!(
            crate::http_workspace::public_commit("ws", numeric).is_err(),
            "{field}"
        );
    }
    let mut extra = body;
    extra["principal"] = "someone-else".into();
    assert!(crate::http_workspace::public_commit("ws", extra).is_err());
}

#[test]
fn typed_branch_rejects_cross_repository_source() {
    let (_root, mut engine, repo, owner) = fixture();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            &owner.principal,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let source = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    let result = Application::new(&mut engine, &owner).create_branch(CreateBranch {
        repository_id: repo.repository_id,
        source_commit_id: source,
        name: storage_contract::BranchName::parse("Cross scope").unwrap(),
        operation_id: storage_contract::OperationId::generate(),
    });
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::HistoryNodeNotFound
    );
}

#[test]
fn creation_replays_keep_public_response_shapes_after_archive_binding_change_and_reopen() {
    let (root, engine, _, owner) = fixture();
    let mut backend = EngineBackend::new(engine);
    let repository_operation = storage_contract::OperationId::generate();
    let repository_request = |name: &str| crate::CreateRepository {
        name: storage_contract::RepositoryName::parse(name).unwrap(),
        primary_slot_id: "P01".into(),
        target_copies: 3,
        operation_id: repository_operation,
    };
    let created = backend
        .create_repository(&owner, repository_request("Creation response"))
        .unwrap();
    let repository = json_codec::to_value(crate::backend_read::RepositoryResponse::from(
        created.clone(),
    ))
    .unwrap();
    let source = backend.engine.lock().unwrap().state().histories[&created.history_tree_id]
        .root_commit_id
        .clone();
    let operation_id = storage_contract::OperationId::generate();
    let branch_request = |name: &str| crate::CreateBranch {
        repository_id: created.repository_id.clone(),
        source_commit_id: source.clone(),
        name: storage_contract::BranchName::parse(name).unwrap(),
        operation_id,
    };
    let branch = json_codec::to_value(crate::backend_branch::BranchResponse::from(
        backend
            .create_branch(&owner, branch_request("side"))
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(branch["state"], "active");
    assert_eq!(branch["generation"], "1");
    assert!(branch.get("created_branch").is_none());
    backend
        .archive_branch(
            &owner,
            crate::ArchiveBranch {
                branch_id: branch["branch_id"].as_str().unwrap().into(),
                operation_id: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
    backend
        .update_smb_binding(
            &owner,
            crate::UpdateSmbBinding {
                repository: &created.repository_id,
                branch: &created.main_branch_id,
                enabled: false,
                expected_history_generation: 1,
                operation: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(backend);
            backend = EngineBackend::new(Engine::open(&root.path().join("system")).unwrap());
        }
        let before =
            storage_engine::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
        let replay = backend
            .create_repository(&owner, repository_request("Creation response"))
            .unwrap();
        assert_eq!(
            json_codec::to_value(crate::backend_read::RepositoryResponse::from(replay)).unwrap(),
            repository
        );
        assert_eq!(
            backend
                .create_repository(&owner, repository_request("Different request"))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        let replay = backend
            .create_branch(&owner, branch_request("side"))
            .unwrap();
        assert_eq!(
            json_codec::to_value(crate::backend_branch::BranchResponse::from(replay)).unwrap(),
            branch
        );
        assert_eq!(
            backend
                .create_branch(&owner, branch_request("Different request"))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(
            storage_engine::canonical_json(backend.engine.lock().unwrap().state()).unwrap(),
            before
        );
    }
}

#[test]
fn merge_link_retry_replays_original_scope_after_split_and_restart() {
    let (root, mut engine, repo, owner) = fixture();
    let root_commit = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let side = engine
        .create_branch(
            &root_commit,
            &storage_contract::BranchName::parse("side").unwrap(),
            &owner.principal,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &owner.principal)
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"body")
        .unwrap();
    let cut = engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: owner.principal.clone(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "cut".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let operation_id = storage_contract::OperationId::generate();
    let command = || crate::CreateMergeLink {
        history_tree_id: repo.history_tree_id.clone(),
        source_branch_id: side.branch_id.clone(),
        source_commit_id: side.head_commit_id.clone(),
        target_branch_id: repo.main_branch_id.clone(),
        target_commit_id: cut.clone(),
        operation_id,
    };
    let mut backend = EngineBackend::new(engine);
    let original = backend.create_merge_link(&owner, command()).unwrap();
    let mut engine = backend.engine.lock().unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, &owner.principal)
        .unwrap();
    let split = engine
        .commit_plan(
            &plan.plan_id,
            &owner.principal,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    drop(engine);
    for reopened in [false, true] {
        if reopened {
            drop(backend);
            backend = EngineBackend::new(Engine::open(&root.path().join("system")).unwrap());
        }
        let before =
            storage_engine::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
        let replay = backend.create_merge_link(&owner, command()).unwrap();
        assert_eq!(
            storage_engine::canonical_json(&replay).unwrap(),
            storage_engine::canonical_json(&original).unwrap()
        );
        let mut changed = command();
        changed.target_commit_id = root_commit.clone();
        assert_eq!(
            backend.create_merge_link(&owner, changed).unwrap_err().code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        let mut wrong_history = command();
        wrong_history.history_tree_id = split.history_tree_id.clone().unwrap();
        assert!(backend.create_merge_link(&owner, wrong_history).is_err());
        let mut new_request = command();
        new_request.operation_id = storage_contract::OperationId::generate();
        assert_eq!(
            backend
                .create_merge_link(&owner, new_request)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::TreeMoved
        );
        assert_eq!(
            storage_engine::canonical_json(backend.engine.lock().unwrap().state()).unwrap(),
            before
        );
    }
}

#[test]
fn upload_finalization_rechecks_completed_session_revocation() {
    for password_change in [false, true] {
        let (root, mut engine, repo, owner) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, &owner.principal)
            .unwrap();
        let source = root.path().join("upload-source");
        std::fs::write(&source, b"prepared bytes").unwrap();
        let item = new_id("entry");
        let backend = std::sync::Arc::new(EngineBackend::new(engine));
        let auth =
            crate::auth::AuthStore::initialize(root.path(), "owner", "integration-test-password")
                .unwrap();
        let mut auth = backend.protect_auth(auth).unwrap();
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let owner = auth.authenticate(&token).unwrap();
        let plan = {
            let engine = backend.engine.lock().unwrap();
            engine
                .prepare_upload(&workspace.workspace_id, &item, "file.txt", &source, 1)
                .unwrap()
        };
        // This update completes while byte preparation holds no engine lock.
        if password_change {
            auth.change_password(
                &token,
                "integration-test-password",
                "new-integration-test-password",
            )
            .unwrap();
        } else {
            auth.logout(&token).unwrap();
        }
        let prepared = plan.execute().unwrap();
        let mut engine = backend.engine.lock().unwrap();
        let result = crate::engine_backend::finish_authorized_upload(
            &mut engine,
            &owner,
            &workspace.workspace_id,
            prepared,
        );
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
        assert!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .files
                .is_empty()
        );
    }
}
#[test]
fn commit_preparation_releases_engine_and_rechecks_completed_revocation() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    for public in [false, true] {
        for revoke in [None, Some(false), Some(true)] {
            let (root, mut engine, repo, owner) = fixture();
            let workspace = engine
                .create_workspace(&repo.main_branch_id, &owner.principal)
                .unwrap();
            engine
                .stage_bytes(&workspace.workspace_id, "file.txt", b"commit bytes")
                .unwrap();
            let operation = storage_contract::OperationId::generate();
            let command = CommitWorkspace {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: operation,
                expected_head: workspace.base_head.clone(),
                fencing_token: workspace.fencing_token,
                message: "commit".into(),
                request_digest: None,
            };
            let input = if public {
                let mut input = engine
                    .seal_workspace(&workspace.workspace_id, &owner.principal)
                    .map(|seal| json_codec::to_value(seal).unwrap())
                    .unwrap();
                input["operation_id"] = operation.to_string().into();
                input["request_digest"] =
                    storage_engine::commit_request_digest(&workspace.workspace_id, &input)
                        .unwrap()
                        .into();
                input
            } else {
                json_codec::Value::Null
            };
            let backend = Arc::new(EngineBackend::new(engine));
            let auth = crate::auth::AuthStore::initialize(
                root.path(),
                "owner",
                "integration-test-password",
            )
            .unwrap();
            let mut auth = backend.protect_auth(auth).unwrap();
            let token = auth
                .login("owner", "integration-test-password")
                .unwrap()
                .token;
            let owner = auth.authenticate(&token).unwrap();
            // A request can retain an Identity captured before revocation.
            // Reject it before preparing bytes, even though concurrent auth
            // writes would also be fenced once preparation has started.
            if let Some(password_change) = revoke {
                if password_change {
                    auth.change_password(
                        &token,
                        "integration-test-password",
                        "new-integration-test-password",
                    )
                    .unwrap();
                } else {
                    auth.logout(&token).unwrap();
                }
                let result = if public {
                    backend
                        .commit_workspace_public(
                            &owner,
                            crate::http_workspace::public_commit(&workspace.workspace_id, input)
                                .unwrap(),
                        )
                        .map(|_| ())
                } else {
                    backend.commit_workspace(&owner, command).map(|_| ())
                };
                assert_eq!(
                    result.unwrap_err().code,
                    storage_contract::ErrorCode::Unauthorized
                );
                let engine = backend.engine.lock().unwrap();
                assert!(!engine.state().operations.contains_key(&operation));
                assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
                continue;
            }
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            *backend.commit_preparation_hook.lock().unwrap() = Some(Arc::new(move || {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }));
            let writer_backend = backend.clone();
            let writer_owner = owner.clone();
            let writer = std::thread::spawn(move || {
                if public {
                    writer_backend
                        .commit_workspace_public(
                            &writer_owner,
                            crate::http_workspace::public_commit(&workspace.workspace_id, input)
                                .unwrap(),
                        )
                        .map(|_| ())
                } else {
                    writer_backend
                        .commit_workspace(&writer_owner, command)
                        .map(|_| ())
                }
            });
            started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let reader_backend = backend.clone();
            let reader_owner = owner.clone();
            let (query_tx, query_rx) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                query_tx
                    .send(reader_backend.repositories_snapshot(&reader_owner))
                    .unwrap();
            });
            let queried = query_rx.recv_timeout(Duration::from_secs(2));
            if queried.is_err() {
                release_tx.send(()).unwrap();
                let _ = writer.join();
                let _ = reader.join();
                panic!("repository query blocked behind detached commit preparation");
            }
            assert!(queried.unwrap().is_ok());
            reader.join().unwrap();
            let before_busy = auth.snapshot().unwrap();
            let busy = auth
                .change_password(
                    &token,
                    "integration-test-password",
                    "new-integration-test-password",
                )
                .unwrap_err();
            assert_eq!(
                busy.code,
                storage_contract::ErrorCode::CommitPreparationBusy
            );
            assert!(busy.retryable);
            assert_eq!(auth.snapshot().unwrap(), before_busy);
            assert!(auth.authenticate(&token).is_ok());
            release_tx.send(()).unwrap();
            let result = writer.join().unwrap();
            let engine = backend.engine.lock().unwrap();
            assert!(result.is_ok(), "{result:?}");
            assert!(engine.state().operations.contains_key(&operation));
        }
    }
}

#[test]
fn namespace_wire_preserves_naming_error_codes_before_staging() {
    use storage_contract::ErrorCode;
    let (_root, mut engine, repo, identity) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .unwrap();
    let backend = EngineBackend::new(engine);
    for (kind, path, destination, code) in [
        ("mkdir", "CON", None, ErrorCode::NameReserved),
        ("mkdir", "e\u{301}.txt", None, ErrorCode::NameNotNormalized),
        ("mkdir", "/outside", None, ErrorCode::PathInvalid),
        ("rename", "source", Some("CON"), ErrorCode::NameReserved),
        (
            "rename",
            "source",
            Some("e\u{301}.txt"),
            ErrorCode::NameNotNormalized,
        ),
    ] {
        let failure = stage_namespace_wire(
            &backend,
            &identity,
            json!({"workspace_id":workspace.workspace_id,
                    "kind":kind,"path":path,"destination":destination}),
        )
        .unwrap_err();
        assert_eq!(failure.code, code, "{kind} {path} {destination:?}");
        assert!(failure.source.is_some());
    }
    assert!(
        backend
            .engine
            .lock()
            .unwrap()
            .workspace(&workspace.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
}

#[test]
fn namespace_wire_carries_lookup_spelling_and_creation_spelling_into_commit() {
    let (_root, mut engine, repo, identity) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .unwrap();
    let backend = EngineBackend::new(engine);
    for (kind, path, destination) in [
        ("mkdir", "Café", None),
        ("rename", "CAFE\u{301}", Some("Renamed")),
    ] {
        stage_namespace_wire(
            &backend,
            &identity,
            json!({"workspace_id":workspace.workspace_id,
                "kind":kind,"path":path,"destination":destination}),
        )
        .unwrap();
    }
    let mut engine = backend.engine.lock().unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: identity.principal,
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path(), "Renamed");
    assert_eq!(entries[0].name(), "Renamed");
    assert_eq!(entries[0].lookup_key(), "renamed");
}

#[test]
fn namespace_wire_rejects_nonstring_destinations_without_staging() {
    let (_root, mut engine, repo, identity) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .unwrap();
    let backend = EngineBackend::new(engine);
    for kind in ["mkdir", "delete", "delete_tree", "rename"] {
        for destination in [json!(123), json!(false), json!({}), json!([])] {
            let failure = stage_namespace_wire(
                &backend,
                &identity,
                json!({"workspace_id":workspace.workspace_id,
                    "kind":kind,"path":"source","destination":destination}),
            )
            .unwrap_err();
            assert_eq!(failure.code, storage_contract::ErrorCode::InvalidRequest);
            assert_eq!(failure.message, "destination must be a string");
        }
    }
    assert!(
        backend
            .engine
            .lock()
            .unwrap()
            .workspace(&workspace.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
}

// External HTTP wire parsing is tested explicitly; no JSON backend action remains.
fn stage_namespace_wire(
    backend: &dyn Backend,
    identity: &Identity,
    body: json_codec::Value,
) -> Result<storage_contract::Workspace, ApiError> {
    backend.stage_namespace(identity, crate::http_workspace::namespace_command(body)?)
}

#[test]
fn typed_plan_commit_retains_published_failure_evidence_and_resumes_cleanup_on_reopen() {
    let (root, mut engine, repo, owner) = fixture();
    let save = |engine: &mut Engine, bytes: &[u8]| {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, &owner.principal)
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: owner.principal.clone(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "cleanup recovery".into(),
                request_digest: None,
            })
            .unwrap();
    };
    save(&mut engine, b"first");
    let first = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, b"latest");
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![first.clone()], &owner.principal)
        .unwrap();
    let blocker = engine.config().disks[0]
        .root
        .join("histories")
        .join(&repo.history_tree_id)
        .join("versions")
        .join(first)
        .join("purged.json");
    std::fs::create_dir(&blocker).unwrap();
    let operation = storage_contract::OperationId::generate();
    let request = || {
        Ok(crate::CommitPlan {
            plan_id: plan.plan_id.clone(),
            plan_digest: plan.digest.clone(),
            operation_id: Ok(operation),
        })
    };
    let backend = EngineBackend::new(engine);
    assert_eq!(
        backend.commit_plan(&owner, request()).unwrap_err().code,
        storage_contract::ErrorCode::UnsafePath
    );
    let (receipt, published) = {
        let engine = backend.engine.lock().unwrap();
        (
            engine.operation(operation).unwrap(),
            storage_contract::canonical_json(engine.state()).unwrap(),
        )
    };
    assert_eq!(
        receipt.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        backend.commit_plan(&owner, request()).unwrap_err().code,
        storage_contract::ErrorCode::UnsafePath
    );
    drop(backend);
    let system = root.path().join("system");
    assert_eq!(
        Engine::open(&system).err().unwrap().code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert!(blocker.is_dir());
    std::fs::remove_dir(&blocker).unwrap();
    let backend = EngineBackend::new(Engine::open(&system).unwrap());
    assert!(blocker.is_file());
    let replay = backend.commit_plan(&owner, request()).unwrap();
    assert_eq!(replay.result, receipt.result);
    assert_eq!(replay.request_digest, receipt.request_digest);
    assert_eq!(
        storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap(),
        published
    );
}
