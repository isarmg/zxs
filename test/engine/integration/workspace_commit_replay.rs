use std::fs;
use storage_engine::{CommitRequest, Config, Engine, new_id};

fn saved() -> (tempfile::TempDir, Engine, String, CommitRequest) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("files").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"retained history")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "original save".into(),
        request_digest: None,
    };
    engine.commit_workspace(request.clone()).unwrap();
    (temp, engine, repo.repository_id, request)
}

#[test]
fn native_commit_replay_rejects_a_different_operation_actor() {
    let (_temp, mut engine, _repository, mut request) = saved();
    request.principal = "different-actor".into();
    assert_eq!(
        engine.commit_workspace(request).unwrap_err().code(),
        storage_contract::ErrorCode::PermissionDenied
    );
}

#[test]
fn native_commit_replay_survives_system_disk_restore() {
    let (temp, mut engine, _repository, request) = saved();
    let original = engine.commit_workspace(request.clone()).unwrap();
    let disks = engine
        .config()
        .disks
        .iter()
        .map(|d| d.root.clone())
        .collect::<Vec<_>>();
    drop(engine);
    fs::remove_dir_all(temp.path().join("system")).unwrap();
    let mut engine = Engine::restore_from_disks(&temp.path().join("replacement"), &disks).unwrap();
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), original);
    let prepared = engine
        .prepare_workspace_commit(request, None)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(engine.finish_workspace_commit(prepared).unwrap(), original);
}

fn request_paths(engine: &Engine, request: &CommitRequest) -> [std::path::PathBuf; 2] {
    [
        engine
            .root()
            .join("workspaces")
            .join(&request.workspace_id)
            .join("commit-request.json"),
        engine.config().disks[0]
            .root
            .join("staging/workspaces")
            .join(&request.workspace_id)
            .join("commit-request.json"),
    ]
}

#[test]
fn native_commit_replay_uses_only_exact_bound_request_copies() {
    for corrupt in [false, true] {
        let (_temp, mut engine, _repository, request) = saved();
        let paths = request_paths(&engine, &request);
        let original = engine.commit_workspace(request.clone()).unwrap();
        let bad = if corrupt {
            b"corrupt".to_vec()
        } else {
            let mut other = request.clone();
            other.message = "different request".into();
            json_codec::to_vec(&other).unwrap()
        };
        fs::write(&paths[0], &bad).unwrap();
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), original);
        fs::write(&paths[1], &bad).unwrap();
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap_err().code(),
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(&paths[0]).unwrap(), bad);
        assert_eq!(fs::read(&paths[1]).unwrap(), bad);
    }
}

#[test]
fn native_commit_replay_does_not_trust_matching_but_unbound_saved_json() {
    let (_temp, mut engine, _repository, original) = saved();
    let paths = request_paths(&engine, &original);
    let requests = paths.clone().map(|path| fs::read(path).unwrap());
    let manifests = paths
        .clone()
        .map(|path| fs::read(path.with_file_name("workspace.json")).unwrap());
    let workspace = engine.workspace(&original.workspace_id).unwrap();
    let source = &workspace.files.values().next().unwrap().source_path;
    let payload = fs::read(source).unwrap();
    let receipt = engine.published_receipt(original.operation_id).unwrap();
    let state = json_codec::to_vec(engine.state()).unwrap();
    let mut request = original.clone();
    request.message = "not the published request".into();
    let forged = json_codec::to_vec(&request).unwrap();

    for damaged_copies in 0..=2 {
        if damaged_copies > 0 {
            fs::write(&paths[damaged_copies - 1], &forged).unwrap();
        }
        let error = engine.commit_workspace(request.clone()).unwrap_err();
        if damaged_copies < 2 {
            // An authenticated surviving request still admits the workspace;
            // the changed caller input must then fail the permanent digest.
            assert_eq!(
                error.code(),
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
            assert_eq!(
                engine.commit_workspace(original.clone()).unwrap(),
                receipt.clone().unwrap()
            );
        } else {
            // Both forged copies destroy the cold workspace's completion proof.
            // Refuse that evidence before attempting a replay digest comparison.
            assert_eq!(error.code(), storage_contract::ErrorCode::RecoveryRequired);
            assert_eq!(
                error.message(),
                "cold workspace evidence changed; reopen for reconciliation"
            );
        }
        assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state);
        assert_eq!(
            engine.published_receipt(original.operation_id).unwrap(),
            receipt
        );
        assert_eq!(
            paths
                .clone()
                .map(|path| fs::read(path.with_file_name("workspace.json")).unwrap()),
            manifests
        );
        assert_eq!(fs::read(source).unwrap(), payload);
        for (index, path) in paths.iter().enumerate() {
            let expected = if index < damaged_copies {
                &forged
            } else {
                &requests[index]
            };
            assert_eq!(&fs::read(path).unwrap(), expected);
        }
    }

    // Reinstating the exact original fixture evidence permits the old ID to
    // replay, without manufacturing a new publication or advancing its fence.
    for (path, bytes) in paths.iter().zip(requests) {
        fs::write(path, bytes).unwrap();
    }
    assert_eq!(engine.commit_workspace(original).unwrap(), receipt.unwrap());
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state);
    assert_eq!(
        json_codec::to_vec(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        json_codec::to_vec(&workspace).unwrap()
    );
}

#[test]
fn unrelated_saved_request_cannot_close_an_unsubmitted_workspace() {
    let (_temp, mut engine, repository, request) = saved();
    let branch = engine.state().repositories[&repository]
        .main_branch_id
        .clone();
    let pending = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(&pending.workspace_id, "pending", b"not committed")
        .unwrap();
    let mut target = request.clone();
    target.workspace_id = pending.workspace_id.clone();
    for path in request_paths(&engine, &target) {
        fs::write(path, json_codec::to_vec(&request).unwrap()).unwrap();
    }
    let root = engine.root().to_owned();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine
            .workspace(&pending.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "open"
    );
}

#[test]
fn forged_saved_request_identity_cannot_close_unsubmitted_bytes() {
    let (_temp, mut engine, repository, mut request) = saved();
    let branch = engine.state().repositories[&repository]
        .main_branch_id
        .clone();
    let pending = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(&pending.workspace_id, "pending", b"not committed")
        .unwrap();
    request.workspace_id = pending.workspace_id.clone();
    request.expected_head = pending.base_head.clone();
    request.fencing_token = pending.fencing_token;
    // Copying a published digest does not turn another manifest into its input.
    request.request_digest = Some(
        engine
            .operation(request.operation_id)
            .unwrap()
            .request_digest,
    );
    for path in request_paths(&engine, &request) {
        fs::write(path, json_codec::to_vec(&request).unwrap()).unwrap();
    }
    let root = engine.root().to_owned();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine
            .workspace(&pending.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "open"
    );
}

#[test]
fn wire_commit_reconciles_after_publication_and_system_disk_loss() {
    for missing_legacy_body in [false, true] {
        let (temp, mut engine, repository, _request) = saved();
        let branch = engine.state().repositories[&repository]
            .main_branch_id
            .clone();
        let workspace = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "wire", b"wire bytes")
            .unwrap();
        let mut body = engine
            .seal_workspace(&workspace.workspace_id, "admin")
            .map(|seal| json_codec::to_value(seal).unwrap())
            .unwrap();
        body["request_digest"] = json_codec::json!(
            storage_engine::commit_request_digest(&workspace.workspace_id, &body).unwrap()
        );
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: body["operation_id"].as_str().unwrap().parse().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        };
        engine.inject_fault_once("after_publication_3");
        assert_eq!(
            engine
                .commit_workspace_wire(request.clone(), public_envelope(body.clone()))
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InDoubt
        );
        let disks = engine
            .config()
            .disks
            .iter()
            .map(|d| d.root.clone())
            .collect::<Vec<_>>();
        if missing_legacy_body {
            // Older writes kept wire bodies only on the lost system disk.
            fs::remove_file(
                request_paths(&engine, &request)[1].with_file_name("wire-request.json"),
            )
            .unwrap();
        }
        drop(engine);
        fs::remove_dir_all(temp.path().join("system")).unwrap();
        let mut engine =
            Engine::restore_from_disks(&temp.path().join("replacement"), &disks).unwrap();
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().state,
            if missing_legacy_body {
                storage_contract::WorkspaceState::Sealed
            } else {
                storage_contract::WorkspaceState::Committed
            }
        );
        let sequence = engine.state().sequence;
        let result = engine
            .commit_workspace_wire(request, public_envelope(body))
            .unwrap();
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(engine.state().sequence, sequence);
        let retained = engine.workspace(&workspace.workspace_id).unwrap();
        assert_eq!(
            fs::read(&retained.files.values().next().unwrap().source_path).unwrap(),
            b"wire bytes"
        );
    }
}

#[test]
fn native_commit_replay_never_follows_unsafe_request_copy() {
    let (temp, mut engine, _repository, request) = saved();
    let [local, _data] = request_paths(&engine, &request);
    let external = temp.path().join("external");
    fs::write(&external, b"untouched").unwrap();
    fs::remove_file(&local).unwrap();
    std::os::unix::fs::symlink(&external, &local).unwrap();
    assert_eq!(
        engine.commit_workspace(request).unwrap_err().code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert_eq!(fs::read(external).unwrap(), b"untouched");
}

fn saved_wire() -> (tempfile::TempDir, Engine, CommitRequest, json_codec::Value) {
    let (temp, mut engine, repository, _) = saved();
    let branch = engine.state().repositories[&repository]
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "wire", b"wire bytes")
        .unwrap();
    let mut body = engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    body["request_digest"] = json_codec::json!(
        storage_engine::commit_request_digest(&workspace.workspace_id, &body).unwrap()
    );
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: body["operation_id"].as_str().unwrap().parse().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: String::new(),
        request_digest: None,
    };
    engine
        .commit_workspace_wire(request.clone(), public_envelope(body.clone()))
        .unwrap();
    (temp, engine, request, body)
}

#[test]
fn wire_receipt_cannot_close_a_different_manifest_with_the_same_workspace_id() {
    for namespace in [false, true] {
        let (_temp, engine, request, _) = saved_wire();
        let root = engine.root().to_owned();
        drop(engine);
        // A completed workspace has advanced fencing, but its unchanged sealed
        // manifest must still reconcile normally using the saved request fence.
        let engine = Engine::open(&root).unwrap();
        let mut workspace = engine.workspace(&request.workspace_id).unwrap();
        assert_eq!(workspace.state.as_str(), "committed");
        assert_eq!(workspace.fencing_token, request.fencing_token + 1);
        workspace.state = storage_contract::WorkspaceState::Sealed;
        if namespace {
            workspace.namespace_operations.push(
                storage_engine::NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Mkdir,
                    "not-published",
                    None,
                )
                .unwrap(),
            );
        } else {
            let staged = workspace.files.values_mut().next().unwrap();
            staged.size += 1;
            staged.content_hash = "0".repeat(64).parse().unwrap();
        }
        for path in request_paths(&engine, &request) {
            fs::write(
                path.with_file_name("workspace.json"),
                json_codec::to_vec(&workspace).unwrap(),
            )
            .unwrap();
        }
        drop(engine);
        let engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine
                .workspace(&request.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "sealed"
        );
        assert_eq!(
            engine.operation(request.operation_id).unwrap().commit_state,
            storage_contract::CommitState::Published
        );
    }
}

fn assert_wire_request_binding(detached: bool) {
    for change_fence in [false, true] {
        let (_temp, mut engine, mut request, body) = saved_wire();
        if change_fence {
            request.fencing_token += 1;
        } else {
            request.message = "different".into();
        }
        let error = if detached {
            engine
                .prepare_workspace_commit(request, Some(public_envelope(body)))
                .unwrap()
                .execute()
                .err()
                .expect("changed request must not replay")
        } else {
            engine
                .commit_workspace_wire(request, public_envelope(body))
                .unwrap_err()
        };
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::RequestBindingMismatch
        );
    }
}

#[test]
fn wire_replay_checks_request_message_and_fencing_against_body() {
    assert_wire_request_binding(false);
}

#[test]
fn detached_wire_replay_checks_request_message_and_fencing_against_body() {
    assert_wire_request_binding(true);
}

#[test]
fn workspace_mirrors_recover_single_corrupt_copy_without_rewriting_evidence() {
    for damaged in [0, 1] {
        let (_temp, mut engine, repository, request) = saved();
        let receipt = engine.commit_workspace(request.clone()).unwrap();
        let branch = engine.state().repositories[&repository]
            .main_branch_id
            .clone();
        let pending = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(&pending.workspace_id, "pending", b"still pending")
            .unwrap();
        let mut pending_request = request.clone();
        pending_request.workspace_id = pending.workspace_id.clone();
        let manifest_paths =
            request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
        let pending_paths =
            request_paths(&engine, &pending_request).map(|p| p.with_file_name("workspace.json"));
        let receipts = request_paths(&engine, &request).map(|p| fs::read(p).unwrap());
        let healthy = fs::read(&manifest_paths[1 - damaged]).unwrap();
        let healthy_pending = fs::read(&pending_paths[1 - damaged]).unwrap();
        let root = engine.root().to_owned();
        drop(engine);
        for path in [&manifest_paths[damaged], &pending_paths[damaged]] {
            fs::write(path, b"{truncated").unwrap();
        }
        let mut engine = Engine::open(&root).unwrap();
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), receipt);
        let retained = engine.workspace(&pending.workspace_id).unwrap();
        assert_eq!(retained.state.as_str(), "open");
        assert_eq!(
            fs::read(&retained.files.values().next().unwrap().source_path).unwrap(),
            b"still pending"
        );
        assert_eq!(fs::read(&manifest_paths[1 - damaged]).unwrap(), healthy);
        assert_eq!(
            fs::read(&pending_paths[1 - damaged]).unwrap(),
            healthy_pending
        );
        assert_eq!(fs::read(&manifest_paths[damaged]).unwrap(), b"{truncated");
        assert_eq!(fs::read(&pending_paths[damaged]).unwrap(), b"{truncated");
        assert_eq!(
            request_paths(&engine, &request).map(|p| fs::read(p).unwrap()),
            receipts
        );
    }
}

#[test]
fn workspace_mirrors_reject_both_corrupt_and_preserve_them() {
    let (_temp, engine, _, request) = saved();
    let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
    let root = engine.root().to_owned();
    drop(engine);
    for path in &paths {
        fs::write(path, b"{truncated").unwrap();
    }
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    for path in paths {
        assert_eq!(fs::read(path).unwrap(), b"{truncated");
    }
}

#[test]
fn workspace_mirrors_never_select_newest_or_first_conflicting_manifest() {
    for changed in [0, 1] {
        for field in ["owner", "content", "fence", "state", "identity"] {
            let (_temp, engine, _, request) = saved();
            let paths =
                request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
            let mut workspace = engine.workspace(&request.workspace_id).unwrap();
            match field {
                "owner" => workspace.principal = "someone-else".into(),
                "content" => {
                    workspace.files.values_mut().next().unwrap().content_hash =
                        "0".repeat(64).parse().unwrap()
                }
                "fence" => workspace.fencing_token += 50,
                "state" => workspace.state = storage_contract::WorkspaceState::Aborted,
                "identity" => workspace.workspace_id = new_id("ws"),
                _ => unreachable!(),
            }
            let root = engine.root().to_owned();
            drop(engine);
            fs::write(&paths[changed], json_codec::to_vec(&workspace).unwrap()).unwrap();
            let before = paths.clone().map(|p| fs::read(p).unwrap());
            assert_eq!(
                Engine::open(&root).err().unwrap().code(),
                storage_contract::ErrorCode::RecoveryRequired,
                "{changed} {field}"
            );
            assert_eq!(paths.map(|p| fs::read(p).unwrap()), before);
        }
    }
}

#[test]
fn workspace_mirrors_reconcile_only_published_state_and_fence_transition() {
    for stale in [0, 1] {
        let (_temp, mut engine, _, request) = saved();
        let receipt = engine.commit_workspace(request.clone()).unwrap();
        let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
        let mut workspace = engine.workspace(&request.workspace_id).unwrap();
        workspace.state = storage_contract::WorkspaceState::Open;
        workspace.fencing_token = request.fencing_token;
        let root = engine.root().to_owned();
        drop(engine);
        fs::write(&paths[stale], json_codec::to_vec(&workspace).unwrap()).unwrap();
        let mut engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine
                .workspace(&request.workspace_id)
                .unwrap()
                .fencing_token,
            request.fencing_token + 1
        );
        assert_eq!(engine.commit_workspace(request).unwrap(), receipt);
    }
}

#[test]
fn workspace_mirrors_reject_unsafe_mirror_instead_of_falling_back() {
    for linked in [0, 1] {
        let (temp, engine, _, request) = saved();
        let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
        let external = temp.path().join("external-workspace");
        fs::copy(&paths[linked], &external).unwrap();
        fs::remove_file(&paths[linked]).unwrap();
        std::os::unix::fs::symlink(&external, &paths[linked]).unwrap();
        let root = engine.root().to_owned();
        drop(engine);
        assert_eq!(
            Engine::open(&root).err().unwrap().code(),
            storage_contract::ErrorCode::UnsafePath
        );
        assert!(
            fs::symlink_metadata(&paths[linked])
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn workspace_mirrors_require_content_evidence_for_degraded_recovery() {
    let (_temp, engine, _, request) = saved();
    let workspace = engine.workspace(&request.workspace_id).unwrap();
    let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
    fs::write(&paths[0], b"{truncated").unwrap();
    let source = &workspace.files.values().next().unwrap().source_path;
    // Published staging files are read-only. Simulate disk corruption by
    // replacing the directory entry, rather than requiring write permission.
    fs::remove_file(source).unwrap();
    fs::write(source, b"wrong bytes").unwrap();
    let root = engine.root().to_owned();
    drop(engine);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(&paths[0]).unwrap(), b"{truncated");
}

#[test]
fn workspace_mirrors_reject_unsafe_staging_paths_and_request_copies() {
    for target in ["source", "request", "directory"] {
        let (temp, engine, _, request) = saved();
        let workspace = engine.workspace(&request.workspace_id).unwrap();
        let path = match target {
            "source" => workspace.files.values().next().unwrap().source_path.clone(),
            "request" => request_paths(&engine, &request)[0].clone(),
            "directory" => request_paths(&engine, &request)[0]
                .parent()
                .unwrap()
                .to_owned(),
            _ => unreachable!(),
        };
        let external = temp.path().join("external");
        fs::rename(&path, &external).unwrap();
        std::os::unix::fs::symlink(&external, &path).unwrap();
        let root = engine.root().to_owned();
        drop(engine);
        assert_eq!(
            Engine::open(&root).err().unwrap().code(),
            storage_contract::ErrorCode::UnsafePath,
            "{target}"
        );
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
}

#[test]
fn workspace_mirrors_do_not_resolve_unpublished_state_transition_by_fence() {
    let (_temp, mut engine, repository, mut request) = saved();
    let branch = engine.state().repositories[&repository]
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    request.workspace_id = workspace.workspace_id.clone();
    let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
    let mut changed = workspace;
    changed.state = storage_contract::WorkspaceState::Aborted;
    changed.fencing_token += 1;
    let root = engine.root().to_owned();
    drop(engine);
    fs::write(&paths[0], json_codec::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
}

#[test]
fn degraded_committed_workspace_requires_authoritative_receipt_binding() {
    for field in ["owner", "branch", "base", "fence"] {
        let (_temp, engine, _, request) = saved();
        let paths = request_paths(&engine, &request).map(|p| p.with_file_name("workspace.json"));
        let mut workspace = engine.workspace(&request.workspace_id).unwrap();
        match field {
            "owner" => workspace.principal = "not-the-published-owner".into(),
            "branch" => workspace.branch_id = new_id("branch"),
            "base" => workspace.base_head = new_id("commit"),
            "fence" => workspace.fencing_token += 50,
            _ => unreachable!(),
        }
        let root = engine.root().to_owned();
        drop(engine);
        fs::write(&paths[0], b"{truncated").unwrap();
        fs::write(&paths[1], json_codec::to_vec(&workspace).unwrap()).unwrap();
        assert_eq!(
            Engine::open(&root).err().unwrap().code(),
            storage_contract::ErrorCode::RecoveryRequired,
            "{field}"
        );
    }
}

#[test]
fn historical_workspace_mirror_recovery_uses_published_plan() {
    for damaged in [0, 1] {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::development([
            temp.path().join("primary"),
            temp.path().join("backup"),
            temp.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("history").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let head = engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let workspace = engine
            .create_historical_workspace(&repo.main_branch_id, &head, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file", b"historical bytes")
            .unwrap();
        let plan = engine
            .create_historical_save_plan(
                &repo.history_tree_id,
                storage_engine::HistoricalSaveOptions {
                    source_commit_id: head,
                    name: "recovered branch".into(),
                    workspace_id: Some(workspace.workspace_id.clone()),
                    primary_slot_id: None,
                    target_copies: None,
                },
                "admin",
            )
            .unwrap();
        let operation = storage_contract::OperationId::generate();
        let receipt = engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        let paths = [
            engine
                .root()
                .join("workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
            engine.config().disks[0]
                .root
                .join("staging/workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
        ];
        let root = engine.root().to_owned();
        drop(engine);
        fs::write(&paths[damaged], b"{truncated").unwrap();
        let mut engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "committed"
        );
        assert_eq!(
            engine
                .commit_historical_save_plan(&plan.plan_id, "admin", operation)
                .unwrap(),
            receipt
        );
        assert_eq!(fs::read(&paths[damaged]).unwrap(), b"{truncated");
    }
}

#[test]
fn partial_workspace_completion_uses_original_published_file_contexts() {
    for stale in [0, 1] {
        for forged in [false, true] {
            let (_temp, mut engine, repository, _) = saved();
            let branch = engine.state().repositories[&repository]
                .main_branch_id
                .clone();
            let workspace = engine.create_workspace(&branch, "admin").unwrap();
            engine
                .stage_bytes(
                    &workspace.workspace_id,
                    "first-file",
                    b"first file contents",
                )
                .unwrap();
            engine
                .seal_workspace(&workspace.workspace_id, "admin")
                .map(|seal| json_codec::to_value(seal).unwrap())
                .unwrap();
            let before = engine.workspace(&workspace.workspace_id).unwrap();
            let request = CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "first file".into(),
                request_digest: None,
            };
            let receipt = engine.commit_workspace(request.clone()).unwrap();
            let mut completed = engine.workspace(&workspace.workspace_id).unwrap();
            assert_ne!(
                before.selected_file_contexts,
                completed.selected_file_contexts
            );
            // A later public selection must not be used to reconstruct the
            // original completion receipt from the earlier save.
            let entry = engine
                .list_tree(&branch)
                .unwrap()
                .into_iter()
                .find(|entry| entry.path == "first-file")
                .unwrap();
            let alternative = engine
                .create_file_branch(
                    &entry.file_id,
                    &entry.version_id,
                    &storage_contract::BranchName::parse("later selection").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            engine
                .select_file_branch(
                    &entry.file_id,
                    alternative.result["branch_id"].as_str().unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            if forged {
                completed
                    .selected_file_contexts
                    .get_mut(&format!("{repository}/first-file"))
                    .unwrap()
                    .branch_id = new_id("branch");
            }
            let paths =
                request_paths(&engine, &request).map(|path| path.with_file_name("workspace.json"));
            let root = engine.root().to_owned();
            drop(engine);
            fs::write(&paths[stale], json_codec::to_vec(&before).unwrap()).unwrap();
            fs::write(&paths[1 - stale], json_codec::to_vec(&completed).unwrap()).unwrap();
            if forged {
                assert_eq!(
                    Engine::open(&root).err().unwrap().code(),
                    storage_contract::ErrorCode::RecoveryRequired
                );
            } else {
                let mut engine = Engine::open(&root).unwrap();
                assert_eq!(
                    engine
                        .workspace(&workspace.workspace_id)
                        .unwrap()
                        .selected_file_contexts,
                    completed.selected_file_contexts
                );
                assert_eq!(engine.commit_workspace(request).unwrap(), receipt);
            }
        }
    }
}

#[test]
fn closing_a_published_workspace_does_not_reopen_its_old_fence_on_restart() {
    let (_temp, mut engine, _, request) = saved();
    engine
        .abort_workspace(&request.workspace_id, "admin")
        .unwrap();
    let closed = engine.workspace(&request.workspace_id).unwrap();
    let root = engine.root().to_owned();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    let recovered = engine.workspace(&request.workspace_id).unwrap();
    assert_eq!(recovered.state.as_str(), "aborted");
    assert_eq!(recovered.fencing_token, closed.fencing_token);
}

#[test]
fn absent_primary_preserves_read_only_workspace_receipt_recovery() {
    for empty_mountpoint in [false, true] {
        let (temp, mut engine, _, request) = saved();
        let receipt = engine.commit_workspace(request.clone()).unwrap();
        let primary = engine.config().disks[0].root.clone();
        let root = engine.root().to_owned();
        drop(engine);
        fs::rename(&primary, temp.path().join("offline-primary")).unwrap();
        if empty_mountpoint {
            fs::create_dir(&primary).unwrap();
            fs::set_permissions(
                &primary,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )
            .unwrap();
        }
        let mut engine = Engine::open(&root).unwrap();
        assert!(engine.is_read_only());
        let replayed = engine.commit_workspace(request).unwrap();
        assert_eq!(replayed.current_health.as_str(), "UNAVAILABLE");
        // Publication identity and protection-at-publish are immutable, while
        // current health must reflect the now-absent primary.
        let mut expected = receipt;
        expected.current_health = storage_contract::CurrentHealth::Unavailable;
        assert_eq!(replayed, expected);
        if empty_mountpoint {
            assert_eq!(fs::read_dir(primary).unwrap().count(), 0);
        }
    }
}

fn public_envelope(body: json_codec::Value) -> storage_contract::PublicCommitEnvelope {
    let workspace = body["workspace_id"].as_str().unwrap().to_owned();
    storage_contract::PublicCommitEnvelope::parse(&workspace, body).unwrap()
}

#[test]
fn absent_saved_request_mirror_can_use_the_exact_surviving_copy() {
    for absent in [0, 1] {
        let (_temp, mut engine, _, request) = saved();
        let expected = engine.commit_workspace(request.clone()).unwrap();
        let paths = request_paths(&engine, &request);
        fs::remove_file(&paths[absent]).unwrap();
        assert_eq!(engine.commit_workspace(request).unwrap(), expected);
        assert!(!paths[absent].exists());
    }
}

#[test]
fn corrupt_request_mirrors_keep_the_parse_error_source() {
    use std::error::Error as _;
    let (_temp, mut engine, _, request) = saved();
    for path in request_paths(&engine, &request) {
        fs::write(path, b"{truncated").unwrap();
    }
    let error = engine.commit_workspace(request).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::RecoveryRequired);
    let mut cause = error.source();
    let mut found = false;
    while let Some(source) = cause {
        found |= source.downcast_ref::<json_codec::Error>().is_some();
        cause = source.source();
    }
    assert!(found, "parse source must survive: {error:?}");
}

#[test]
#[ignore = "requires workspace_read_error.c as LD_PRELOAD and ZXS_TEST_WORKSPACE_ERRNO=5 or 13"]
fn workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror() {
    use std::error::Error as _;
    let errno: i32 = std::env::var("ZXS_TEST_WORKSPACE_ERRNO")
        .unwrap()
        .parse()
        .unwrap();
    for name in ["commit-request.json", "workspace.json"] {
        let (_temp, mut engine, _, request) = saved();
        let path = request_paths(&engine, &request)[0].with_file_name(name);
        let original = fs::read(&path).unwrap();
        let marker = path.with_extension("read-error");
        fs::write(&marker, b"inject read errno only for this file").unwrap();
        let error = if name == "commit-request.json" {
            engine.commit_workspace(request).unwrap_err()
        } else {
            let root = engine.root().to_owned();
            drop(engine);
            Engine::open(&root)
                .err()
                .expect("I/O error must reject recovery")
        };
        assert_eq!(error.code(), storage_contract::ErrorCode::IoError);
        let mut cause = error.source();
        let mut actual = None;
        while let Some(source) = cause {
            if let Some(source) = source.downcast_ref::<std::io::Error>() {
                actual = source.raw_os_error();
            }
            cause = source.source();
        }
        assert_eq!(actual, Some(errno), "{error:?}");
        fs::remove_file(marker).unwrap();
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
#[ignore = "requires an unprivileged process for real EACCES (no chmod bypass capability)"]
fn unreadable_workspace_request_is_not_a_missing_mirror() {
    use std::{error::Error as _, os::unix::fs::PermissionsExt};
    let (_temp, mut engine, _, request) = saved();
    let path = request_paths(&engine, &request)[0].clone();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let original = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let probe = fs::File::open(&path);
    let result = engine.commit_workspace(request);
    fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(
        probe.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let error = result.unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::IoError);
    let mut cause = error.source();
    let mut denied = false;
    while let Some(source) = cause {
        denied |= source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
        cause = source.source();
    }
    assert!(denied, "permission source must survive: {error:?}");
    assert_eq!(fs::read(path).unwrap(), original);
}
