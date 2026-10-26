use super::*;
use crate::{Backend, engine_backend::EngineBackend};
use json_codec::json;
use storage_contract::{Config, new_id};

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
        .create_repository("Application", "P01", 3, "owner", &new_id("op"))
        .unwrap();
    let identity = crate::engine_backend::install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}

#[test]
fn protection_commands_validate_wire_types_and_unknown_fields() {
    let policy = json!({"policy_id":new_id("policy"),"repository_id":new_id("repo"),
            "path_prefix":"","target_copies":3,"expected_generation":"0","operation_id":new_id("op")});
    let rule = json!({"rule_id":new_id("rule"),"repository_id":new_id("repo"),
            "path_prefix":"","primary_slot_id":"P01","expected_generation":"0","operation_id":new_id("op")});
    for (is_policy, valid) in [(true, policy), (false, rule)] {
        let decode = |value: json_codec::Value| {
            if is_policy {
                json_codec::from_value::<UpdateProtectionPolicy>(value).map(|_| ())
            } else {
                json_codec::from_value::<UpdatePlacementRule>(value).map(|_| ())
            }
        };
        assert!(decode(valid.clone()).is_ok());
        for generation in [
            json!(0),
            json!(null),
            json!(""),
            json!("00"),
            json!("01"),
            json!("+1"),
            json!(" 1"),
            json!("-1"),
            json!("18446744073709551616"),
        ] {
            let mut invalid = valid.clone();
            invalid["expected_generation"] = generation;
            assert!(decode(invalid).is_err());
        }
        let mut max = valid.clone();
        max["expected_generation"] = json!(u64::MAX.to_string());
        assert!(decode(max).is_ok());
        for key in ["repository_id", "operation_id"] {
            let mut invalid = valid.clone();
            invalid[key] = json!("");
            assert!(decode(invalid).is_err());
        }
        let mut extra = valid.clone();
        extra["principal"] = json!("admin");
        assert!(decode(extra).is_err());
        let mut invalid = valid.clone();
        invalid["path_prefix"] = json!(null);
        assert!(decode(invalid).is_err());
        if is_policy {
            for copies in [json!("3"), json!(3.5), json!(-1), json!(4294967296_u64)] {
                let mut invalid = valid.clone();
                invalid["target_copies"] = copies;
                assert!(decode(invalid).is_err());
            }
        }
    }
}

#[test]
fn protection_commands_preserve_core_replay_and_transport_contract() {
    for is_policy in [true, false] {
        let (_root, mut engine, repo, owner) = fixture();
        let operation = new_id("op");
        let id = new_id(if is_policy { "policy" } else { "rule" });
        let (action, input, original) = if is_policy {
            let original = engine
                .update_protection_policy(storage_engine::ProtectionPolicyUpdate {
                    policy_id: &id,
                    repository_id: &repo.repository_id,
                    path_prefix: "",
                    target_copies: 3,
                    expected_generation: 0,
                    principal: &owner.principal,
                    operation_id: &operation,
                })
                .unwrap();
            (
                crate::BackendAction::UpdatePolicy,
                json!({"policy_id":id,"repository_id":repo.repository_id,"path_prefix":"",
                        "target_copies":3,"expected_generation":"0","operation_id":operation}),
                original,
            )
        } else {
            let original = engine
                .update_placement_rule(storage_engine::PlacementRuleUpdate {
                    rule_id: &id,
                    repository_id: &repo.repository_id,
                    path_prefix: "",
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: &owner.principal,
                    operation_id: &operation,
                })
                .unwrap();
            (
                crate::BackendAction::UpdatePlacementRule,
                json!({"rule_id":id,"repository_id":repo.repository_id,"path_prefix":"",
                        "primary_slot_id":"P01","expected_generation":"0","operation_id":operation}),
                original,
            )
        };
        let backend = EngineBackend::new(engine);
        let replay = backend.call(action, &owner, input.clone()).unwrap();
        assert_eq!(replay["request_digest"], original.request_digest);
        assert_eq!(replay["result"], original.result);
        assert_eq!(
            replay["publication_sequence"],
            original.sequence.to_string()
        );
        assert_eq!(replay["operation_kind"], original.kind);
        let mut changed = input.clone();
        changed["path_prefix"] = json!("Other");
        assert_eq!(
            backend.call(action, &owner, changed).unwrap_err().code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        let mut extra = input.clone();
        extra["unexpected"] = json!(true);
        assert_eq!(backend.call(action, &owner, extra).unwrap_err().status, 400);
        let stranger = Identity {
            admission_session: None,
            smb_binding: None,
            principal: "stranger".into(),
        };
        assert_eq!(
            backend
                .call(action, &stranger, input.clone())
                .unwrap_err()
                .status,
            401
        );
        let mut malformed = input.clone();
        malformed["expected_generation"] = json!(1);
        assert_eq!(
            backend
                .call(action, &stranger, malformed.clone())
                .unwrap_err()
                .status,
            401
        );
        assert_eq!(
            backend.call(action, &owner, malformed).unwrap_err().status,
            400
        );
        assert_eq!(
            backend.engine.lock().unwrap().state().operations[&operation].request_digest,
            original.request_digest
        );
        let mut update = if is_policy {
            json!({"policy_id":id,"repository_id":repo.repository_id,"path_prefix":"",
                    "target_copies":3,"expected_generation":"1","operation_id":new_id("op")})
        } else {
            json!({"rule_id":id,"repository_id":repo.repository_id,"path_prefix":"",
                    "primary_slot_id":"P01","expected_generation":"1","operation_id":new_id("op")})
        };
        let unadmitted_owner = Identity {
            admission_session: Some("unknown-session".into()),
            ..owner.clone()
        };
        assert_eq!(
            backend
                .call(action, &unadmitted_owner, update.clone())
                .unwrap_err()
                .status,
            401
        );
        let applied = backend.call(action, &owner, update.clone()).unwrap();
        let record = if is_policy { "policy" } else { "rule" };
        assert_eq!(applied["result"][record]["generation"], "2");
        update["operation_id"] = json!(new_id("op"));
        let stale = backend.call(action, &owner, update).unwrap_err();
        assert_eq!(stale.code, storage_contract::ErrorCode::ConfigConflict);
        assert_eq!(stale.status, 409);
    }
}

#[test]
fn public_commit_roundtrip_preserves_canonical_envelope_and_executes() {
    let (_root, mut engine, repo, identity) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, &identity.principal)
        .unwrap();
    Application::new(&mut engine, &identity)
        .stage_namespace(StageNamespace {
            workspace_id: workspace.workspace_id.clone(),
            change: NamespaceOperation {
                kind: storage_contract::NamespaceOperationKind::Mkdir,
                path: "Folder".into(),
                destination: None,
            },
        })
        .unwrap();
    let mut body = engine
        .seal_workspace(&workspace.workspace_id, &identity.principal)
        .unwrap();
    body["request_digest"] = storage_engine::commit_request_digest(&workspace.workspace_id, &body)
        .unwrap()
        .into();
    let command: PublicCommitWorkspace = json_codec::from_value(body.clone()).unwrap();
    assert_eq!(json_codec::to_value(&command).unwrap(), body);
    let result = Application::new(&mut engine, &identity)
        .commit_workspace_public(command.clone())
        .unwrap();
    let replay = Application::new(&mut engine, &identity)
        .commit_workspace_public(command)
        .unwrap();
    assert_eq!(replay.commit_id, result.commit_id);
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
        change: NamespaceOperation {
            kind: storage_contract::NamespaceOperationKind::Mkdir,
            path: "TypedDenied".into(),
            destination: None,
        },
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
        operation_id: new_id("op"),
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
        let result = backend.call(
            crate::BackendAction::Namespace,
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
    let body = json!({"workspace_id":"ws", "operation_id":"op", "repository_id":"repo", "branch_id":"branch",
            "expected_head":"head", "fencing_token":"1", "message":"", "requested_protection":"strict",
            "file_placement":{}, "expected_topology_generation":"1", "sealed_manifest_hash":"hash", "request_digest":"digest"});
    assert!(json_codec::from_value::<PublicCommitWorkspace>(body.clone()).is_ok());
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
            json_codec::from_value::<PublicCommitWorkspace>(missing).is_err(),
            "{field}"
        );
    }
    for field in ["fencing_token", "expected_topology_generation"] {
        let mut numeric = body.clone();
        numeric[field] = json!(1);
        assert!(
            json_codec::from_value::<PublicCommitWorkspace>(numeric).is_err(),
            "{field}"
        );
    }
    let mut extra = body;
    extra["principal"] = "someone-else".into();
    assert!(json_codec::from_value::<PublicCommitWorkspace>(extra).is_err());
}

#[test]
fn typed_branch_rejects_cross_repository_source() {
    let (_root, mut engine, repo, owner) = fixture();
    let other = engine
        .create_repository("Other", "P01", 3, &owner.principal, &new_id("op"))
        .unwrap();
    let source = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    let result = Application::new(&mut engine, &owner).create_branch(CreateBranch {
        repository_id: repo.repository_id,
        source_commit_id: source,
        name: "Cross scope".into(),
        operation_id: new_id("op"),
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
    let repository_request = json!({
        "name":"Creation response", "primary_slot_id":"P01",
        "target_copies":3, "operation_id":new_id("op")
    });
    let repository = backend
        .call(
            crate::BackendAction::CreateRepository,
            &owner,
            repository_request.clone(),
        )
        .unwrap();
    let created: storage_engine::Repository = json_codec::from_value(repository.clone()).unwrap();
    let source = backend.engine.lock().unwrap().state().histories[&created.history_tree_id]
        .root_commit_id
        .clone();
    let branch_request = json!({
        "repository_id":created.repository_id, "source_commit_id":source,
        "name":"side", "operation_id":new_id("op")
    });
    let branch = backend
        .call(
            crate::BackendAction::CreateBranch,
            &owner,
            branch_request.clone(),
        )
        .unwrap();
    assert_eq!(branch["state"], "active");
    assert_eq!(branch["generation"], "1");
    assert!(branch.get("created_branch").is_none());
    backend
        .call(
            crate::BackendAction::Archive,
            &owner,
            json!({
                "branch_id":branch["branch_id"], "operation_id":new_id("op")
            }),
        )
        .unwrap();
    backend
        .call(
            crate::BackendAction::UpdateSmbBinding,
            &owner,
            json!({
                "repository_id":created.repository_id, "branch_id":created.main_branch_id,
                "enabled":false, "expected_history_generation":"1", "operation_id":new_id("op")
            }),
        )
        .unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(backend);
            backend = EngineBackend::new(Engine::open(&root.path().join("system")).unwrap());
        }
        let before =
            storage_engine::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
        for (action, request, response) in [
            (
                crate::BackendAction::CreateRepository,
                &repository_request,
                &repository,
            ),
            (crate::BackendAction::CreateBranch, &branch_request, &branch),
        ] {
            assert_eq!(
                backend.call(action, &owner, request.clone()).unwrap(),
                *response
            );
            let mut changed = request.clone();
            changed["name"] = "Different request".into();
            assert_eq!(
                backend.call(action, &owner, changed).unwrap_err().code,
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        }
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
        .create_branch(&root_commit, "side", &owner.principal, &new_id("op"))
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
            operation_id: new_id("op"),
            principal: owner.principal.clone(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "cut".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let command = MergeLink {
        history_tree_id: repo.history_tree_id.clone(),
        source_branch_id: side.branch_id,
        source_commit_id: side.head_commit_id,
        target_branch_id: repo.main_branch_id,
        target_commit_id: cut.clone(),
        operation_id: new_id("op"),
    };
    let original = Application::new(&mut engine, &owner)
        .merge_link(command.clone())
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, &owner.principal)
        .unwrap();
    let split = engine
        .commit_plan(&plan.plan_id, &owner.principal, &new_id("op"))
        .unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root.path().join("system")).unwrap();
        }
        let before = storage_engine::canonical_json(engine.state()).unwrap();
        let replay = Application::new(&mut engine, &owner)
            .merge_link(command.clone())
            .unwrap();
        assert_eq!(
            storage_engine::canonical_json(&replay).unwrap(),
            storage_engine::canonical_json(&original).unwrap()
        );
        let mut changed = command.clone();
        changed.target_commit_id = root_commit.clone();
        assert_eq!(
            Application::new(&mut engine, &owner)
                .merge_link(changed)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        let mut wrong_history = command.clone();
        wrong_history.history_tree_id = split.history_tree_id.clone().unwrap();
        assert!(
            Application::new(&mut engine, &owner)
                .merge_link(wrong_history)
                .is_err()
        );
        let mut new_request = command.clone();
        new_request.operation_id = new_id("op");
        assert_eq!(
            Application::new(&mut engine, &owner)
                .merge_link(new_request)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::TreeMoved
        );
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
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
            let operation = new_id("op");
            let input = if public {
                let mut input = engine
                    .seal_workspace(&workspace.workspace_id, &owner.principal)
                    .unwrap();
                input["operation_id"] = operation.clone().into();
                input["request_digest"] =
                    storage_engine::commit_request_digest(&workspace.workspace_id, &input)
                        .unwrap()
                        .into();
                input
            } else {
                json!({"workspace_id":workspace.workspace_id,"operation_id":operation,
                        "expected_head":workspace.base_head,"fencing_token":workspace.fencing_token.to_string(),"message":"commit"})
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
                let result = backend.call(
                    if public {
                        crate::BackendAction::CommitWorkspace
                    } else {
                        crate::BackendAction::CommitWorkspaceInternal
                    },
                    &owner,
                    input,
                );
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
                writer_backend.call(
                    if public {
                        crate::BackendAction::CommitWorkspace
                    } else {
                        crate::BackendAction::CommitWorkspaceInternal
                    },
                    &writer_owner,
                    input,
                )
            });
            started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let reader_backend = backend.clone();
            let reader_owner = owner.clone();
            let (query_tx, query_rx) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                query_tx
                    .send(reader_backend.call(
                        crate::BackendAction::Repositories,
                        &reader_owner,
                        json!({}),
                    ))
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
