//! Published workspaces retain the destination of files absent from their base.
use super::*;
use crate::engine::native_workspace_commit_completed;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Published selections").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (temporary, engine, repo)
}

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> Workspace {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "fresh.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "published selection".into(),
            request_digest: None,
        })
        .unwrap();
    engine.workspace(&workspace.workspace_id).unwrap()
}

#[test]
fn published_new_file_context_fences_same_version_branch_switch() {
    for reopen in [false, true] {
        let (temporary, mut engine, repo) = fixture();
        let workspace = save(&mut engine, &repo.main_branch_id, b"old");
        let key = directory::context_key_from_key(&repo.repository_id, "fresh.txt");
        assert!(
            find_unordered(
                &engine
                    .namespace_entries(&engine.state().commits[&workspace.base_head].namespace)
                    .unwrap(),
                "fresh.txt"
            )
            .is_none()
        );
        assert_eq!(workspace.state, WorkspaceState::Committed);
        assert_eq!(
            workspace.selected_file_contexts.get(&key),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .unwrap()
                .as_ref()
        );
        assert!(workspace.selected_file_contexts.contains_key(&key));
        if reopen {
            drop(engine);
            engine = Engine::open(&temporary.path().join("system")).unwrap();
        }
        engine
            .validate_workspace_file_selection(
                &workspace.workspace_id,
                &name_policy::PortablePath::parse("fresh.txt").unwrap(),
            )
            .unwrap();
        let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let draft = engine
            .create_file_branch(
                &source.file_id,
                &source.version_id,
                &storage_contract::BranchName::parse("same version draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        let directory_generations = engine.state().directory_selection_generations.clone();
        engine
            .select_file_branch(
                &source.file_id,
                &draft,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let selected = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        assert_eq!(
            (&selected.entry_id, &selected.version_id, &selected.path),
            (&source.entry_id, &source.version_id, &source.path)
        );
        assert_eq!(
            engine.state().directory_selection_generations,
            directory_generations
        );
        assert_eq!(
            engine.pending_file_source(&draft, &source.file_id),
            Some(source.version_id.as_str())
        );
        assert_ne!(
            workspace.selected_file_contexts.get(&key),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .unwrap()
                .as_ref()
        );
        assert_eq!(
            engine
                .validate_workspace_file_selection(
                    &workspace.workspace_id,
                    &name_policy::PortablePath::parse("fresh.txt").unwrap()
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FileBranchConflict
        );
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .selected_file_contexts,
            workspace.selected_file_contexts
        );
    }
}

#[test]
fn published_workspace_uses_post_save_context_and_fences_selection_round_trips() {
    let (_temporary, mut engine, repo) = fixture();
    let first = save(&mut engine, &repo.main_branch_id, b"old");
    engine
        .validate_workspace_file_selection(
            &first.workspace_id,
            &name_policy::PortablePath::parse("fresh.txt").unwrap(),
        )
        .unwrap();
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let draft = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &storage_contract::BranchName::parse("first write").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(
            &source.file_id,
            &draft,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut completed = first;
    for bytes in [b"one".as_slice(), b"two".as_slice()] {
        completed = save(&mut engine, &repo.main_branch_id, bytes);
        assert_eq!(
            completed
                .selected_file_contexts
                .get(&directory::context_key_from_key(
                    &repo.repository_id,
                    "fresh.txt"
                )),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .unwrap()
                .as_ref()
        );
        engine
            .validate_workspace_file_selection(
                &completed.workspace_id,
                &name_policy::PortablePath::parse("fresh.txt").unwrap(),
            )
            .unwrap();
        let next = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        assert_eq!(
            next.selected_file_contexts,
            completed.selected_file_contexts
        );
        engine.abort_workspace(&next.workspace_id, "admin").unwrap();
    }
    let current = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let alternative = engine
        .create_file_branch(
            &current.file_id,
            &current.version_id,
            &storage_contract::BranchName::parse("round trip").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    for selected in [&alternative, &draft] {
        engine
            .select_file_branch(
                &current.file_id,
                selected,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id,
        current.version_id
    );
    assert_eq!(
        engine
            .file_context(&repo.repository_id, "fresh.txt")
            .unwrap()
            .unwrap()
            .branch_id,
        draft
    );
    assert_eq!(
        engine
            .validate_workspace_file_selection(
                &completed.workspace_id,
                &name_policy::PortablePath::parse("fresh.txt").unwrap()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
}

#[test]
fn recovery_refreshes_both_precompletion_mirrors_from_original_publication() {
    for wire in [false, true] {
        let (temporary, mut engine, repo) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "fresh.txt", b"old")
            .unwrap();
        let body = wire.then(|| {
            let mut body = engine
                .seal_workspace(&workspace.workspace_id, "admin")
                .map(|seal| json_codec::to_value(seal).unwrap())
                .unwrap();
            body["request_digest"] =
                json!(crate::wire::commit_request_digest(&workspace.workspace_id, &body).unwrap());
            body
        });
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: body
                .as_ref()
                .map(|body| body["operation_id"].as_str().unwrap().parse().unwrap())
                .unwrap_or_else(storage_contract::OperationId::generate),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        };
        let before = engine.workspace(&workspace.workspace_id).unwrap();
        let manifests = [
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
        let original_manifests = manifests.clone().map(|path| std::fs::read(path).unwrap());
        let key = directory::context_key_from_key(&repo.repository_id, "fresh.txt");
        assert!(!before.selected_file_contexts.contains_key(&key));
        engine.inject_fault_once("after_local_publications");
        let result = if let Some(body) = body.clone() {
            engine.commit_workspace_wire(request.clone(), public_envelope(body))
        } else {
            engine.commit_workspace(request.clone())
        };
        let error = result.unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
        assert!(error.published_result().is_none());
        assert_eq!(
            manifests.clone().map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
        drop(engine);

        let root = temporary.path().join("system");
        let mut engine = Engine::open(&root).unwrap();
        let completed = engine.workspace(&workspace.workspace_id).unwrap();
        let published_context = engine
            .file_context(&repo.repository_id, "fresh.txt")
            .unwrap()
            .unwrap();
        assert_eq!(completed.state, WorkspaceState::Committed);
        assert_eq!(completed.fencing_token, before.fencing_token + 1);
        assert_eq!(
            completed.selected_file_contexts.get(&key),
            Some(&published_context)
        );
        engine
            .validate_workspace_file_selection(
                &workspace.workspace_id,
                &name_policy::PortablePath::parse("fresh.txt").unwrap(),
            )
            .unwrap();
        // Recovery does not rewrite either manifest. A later reopen must still
        // use the original publication, not a newer same-content selection.
        assert_eq!(
            manifests.clone().map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
        let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let draft = engine
            .create_file_branch(
                &source.file_id,
                &source.version_id,
                &storage_contract::BranchName::parse("later draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        engine
            .select_file_branch(
                &source.file_id,
                &draft,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        assert_ne!(
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .unwrap()
                .unwrap(),
            published_context
        );
        drop(engine);

        let mut engine = Engine::open(&root).unwrap();
        let recovered = engine.workspace(&workspace.workspace_id).unwrap();
        assert_eq!(
            recovered.selected_file_contexts,
            completed.selected_file_contexts
        );
        assert_eq!(
            engine
                .validate_workspace_file_selection(
                    &workspace.workspace_id,
                    &name_policy::PortablePath::parse("fresh.txt").unwrap()
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FileBranchConflict
        );
        let replayed = if let Some(body) = body {
            engine.commit_workspace_wire(request.clone(), public_envelope(body))
        } else {
            engine.commit_workspace(request.clone())
        }
        .unwrap();
        assert_eq!(replayed.operation_id, request.operation_id);
        assert_eq!(
            replayed.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(
            manifests.map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
    }
}

fn public_envelope(body: json_codec::Value) -> storage_contract::PublicCommitEnvelope {
    let workspace = body["workspace_id"].as_str().unwrap().to_owned();
    storage_contract::PublicCommitEnvelope::parse(&workspace, body).unwrap()
}

#[test]
fn published_workspace_completion_errors_keep_receipt_source_and_write_fence() {
    use std::error::Error as _;

    // The budget failure occurs before persist_workspace enters its I/O closure.
    for failed_stage in ["budget", "primary", "system"] {
        let (temporary, mut engine, repo) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "saved.txt", b"published bytes")
            .unwrap();
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "original request".into(),
            request_digest: None,
        };
        let plan = engine
            .plan_workspace_commit_inner(request.clone(), None)
            .unwrap();
        engine.persist_workspace_commit_request(&plan).unwrap();
        let MutationPlan::Prepared {
            next,
            prepared,
            namespace,
            result,
        } = plan.mutation
        else {
            panic!("invariant: first submission prepares a new publication")
        };
        engine
            .persist_with_objects(*next, prepared, namespace, result.clone())
            .unwrap();
        let before = engine.workspace(&request.workspace_id).unwrap();
        let directories = [
            engine.workspace_data_directory(&before).unwrap(),
            engine.root().join("workspaces").join(&request.workspace_id),
        ];
        let requests = directories
            .clone()
            .map(|path| std::fs::read(path.join("commit-request.json")).unwrap());
        let saved_config = engine.config.clone();
        let obstruction = if failed_stage == "budget" {
            let mut config = engine.config().clone();
            config.max_retained_workspace_metadata_bytes = 1;
            engine.config = ValidatedConfig::try_from(config).unwrap();
            None
        } else {
            let path = directories[usize::from(failed_stage == "system")].join("workspace.json");
            let retained = path.with_extension("retained");
            std::fs::rename(&path, &retained).unwrap();
            std::fs::create_dir(&path).unwrap();
            Some((path, retained))
        };
        assert!(!engine.is_read_only());
        let error = engine
            .complete_workspace_commit(plan.workspace, result.clone())
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert!(!error.is_retryable());
        assert_eq!(error.published_result(), Some(&result));
        assert!(error.to_string().contains("is already published"));
        assert!(
            error
                .to_string()
                .contains(&request.operation_id.to_string())
        );
        let Error::PublishedWorkspaceReconciliation(context) = &error else {
            panic!("published completion context was lost: {error}")
        };
        assert_eq!(context.workspace_id(), request.workspace_id);
        assert_eq!(context.principal(), request.principal);
        let source = context.source().unwrap().downcast_ref::<Error>().unwrap();
        assert_eq!(
            source.code(),
            if failed_stage == "budget" {
                ErrorCode::ResourceLimit
            } else {
                ErrorCode::UnsafePath
            }
        );
        assert!(engine.is_read_only());
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        assert_eq!(
            canonical_json(&engine.workspace(&request.workspace_id).unwrap()).unwrap(),
            canonical_json(&before).unwrap()
        );
        assert_eq!(engine.operation(request.operation_id).unwrap(), result);
        assert!(
            !native_workspace_commit_completed(&before, &request, &result).unwrap(),
            "the permanent receipt does not complete terminal workspace mirrors"
        );
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), result);
        assert!(
            engine.is_read_only(),
            "receipt replay is not local reconciliation"
        );
        assert_eq!(
            engine
                .create_workspace(&repo.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(
            directories.map(|path| std::fs::read(path.join("commit-request.json")).unwrap()),
            requests
        );
        for file in before.files.values() {
            assert_eq!(
                std::fs::read(&file.source_path).unwrap(),
                b"published bytes"
            );
        }
        if let Some((path, retained)) = obstruction {
            std::fs::remove_dir(&path).unwrap();
            std::fs::rename(retained, path).unwrap();
        }
        engine.config = saved_config;
        drop(engine);
        let engine = Engine::open(&temporary.path().join("system")).unwrap();
        assert!(!engine.is_read_only());
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        assert_eq!(engine.operation(request.operation_id).unwrap(), result);
        assert_eq!(
            engine.workspace(&request.workspace_id).unwrap().state,
            WorkspaceState::Committed
        );
        assert!(
            native_workspace_commit_completed(
                &engine.workspace_view(&request.workspace_id).unwrap(),
                &request,
                &result,
            )
            .unwrap()
        );
    }
}

#[test]
fn native_workspace_completion_requires_bound_terminal_fence_not_global_readiness() {
    let (temporary, mut engine, repo) = fixture();
    let workspace = save(&mut engine, &repo.main_branch_id, b"original");
    let request: CommitRequest = json_codec::from_slice(
        &std::fs::read(
            temporary
                .path()
                .join("system/workspaces")
                .join(&workspace.workspace_id)
                .join("commit-request.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let receipt = engine.operation(request.operation_id).unwrap();
    assert!(native_workspace_commit_completed(&workspace, &request, &receipt).unwrap());
    assert!(engine.native_workspace_commit_completed(&request).unwrap());
    engine.blocked = true;
    assert!(engine.native_workspace_commit_completed(&request).unwrap());
    engine.blocked = false;
    let mut foreign = request.clone();
    foreign.principal = "other-actor".into();
    assert_eq!(
        engine
            .native_workspace_commit_completed(&foreign)
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
    let mut absent = request.clone();
    absent.operation_id = OperationId::generate();
    assert_eq!(
        engine
            .native_workspace_commit_completed(&absent)
            .unwrap_err()
            .code(),
        ErrorCode::OperationNotFound
    );

    for (state, fence, expected) in [
        (WorkspaceState::Open, request.fencing_token, false),
        (WorkspaceState::Sealed, request.fencing_token, false),
        (WorkspaceState::Committed, request.fencing_token, false),
        (WorkspaceState::Committed, request.fencing_token + 1, true),
        (WorkspaceState::Committed, request.fencing_token + 2, false),
        (WorkspaceState::Aborted, request.fencing_token + 1, false),
        (WorkspaceState::Aborted, request.fencing_token + 2, true),
        (WorkspaceState::Aborted, request.fencing_token + 3, true),
    ] {
        let mut observed = workspace.clone();
        observed.state = state;
        observed.fencing_token = fence;
        assert_eq!(
            native_workspace_commit_completed(&observed, &request, &receipt).unwrap(),
            expected,
            "{state:?} at {fence}"
        );
    }
    for field in [
        "operation",
        "workspace",
        "principal",
        "base",
        "fence",
        "message",
        "digest",
    ] {
        let mut changed = request.clone();
        match field {
            "operation" => changed.operation_id = OperationId::generate(),
            "workspace" => changed.workspace_id = "other-workspace".into(),
            "principal" => changed.principal = "other-actor".into(),
            "base" => changed.expected_head = "other-base".into(),
            "fence" => changed.fencing_token += 1,
            "message" => changed.message = "other-message".into(),
            "digest" => changed.request_digest = Some("other-digest".into()),
            _ => unreachable!(),
        }
        assert!(!native_workspace_commit_completed(&workspace, &changed, &receipt).unwrap());
    }
    for field in ["kind", "branch", "digest", "state"] {
        let mut changed = receipt.clone();
        match field {
            "kind" => changed.kind = "other-kind".into(),
            "branch" => changed.branch_id = Some("other-branch".into()),
            "digest" => changed.request_digest = "other-digest".into(),
            "state" => changed.commit_state = CommitState::InDoubt,
            _ => unreachable!(),
        }
        assert!(!native_workspace_commit_completed(&workspace, &request, &changed).unwrap());
    }
    let mut changed = workspace.clone();
    changed.files.values_mut().next().unwrap().size += 1;
    assert!(!native_workspace_commit_completed(&changed, &request, &receipt).unwrap());

    for fence in [u64::MAX - 1, u64::MAX] {
        let mut original = request.clone();
        original.fencing_token = fence;
        let mut published = receipt.clone();
        published.request_digest = native_commit_digest(&workspace, &original).unwrap();
        let mut observed = workspace.clone();
        observed.state = WorkspaceState::Aborted;
        observed.fencing_token = u64::MAX;
        assert!(!native_workspace_commit_completed(&observed, &original, &published).unwrap());
        observed.state = WorkspaceState::Committed;
        assert_eq!(
            native_workspace_commit_completed(&observed, &original, &published).unwrap(),
            fence == u64::MAX - 1
        );
    }
    for _ in 0..2 {
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        assert!(
            native_workspace_commit_completed(
                &engine.workspace_view(&workspace.workspace_id).unwrap(),
                &request,
                &receipt,
            )
            .unwrap()
        );
    }
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert!(
        native_workspace_commit_completed(
            &engine.workspace_view(&workspace.workspace_id).unwrap(),
            &request,
            &receipt,
        )
        .unwrap()
    );
}

#[test]
fn published_workspace_context_preserves_concrete_io_and_native_diagnostics() {
    use std::error::Error as _;

    let (_temporary, mut engine, repo) = fixture();
    let workspace = save(&mut engine, &repo.main_branch_id, b"original");
    let result = engine
        .state()
        .operations
        .tail()
        .iter()
        .map(|entry| &entry.result)
        .find(|result| result.kind == "workspace.commit")
        .unwrap();
    for raw in [5, 20, 28] {
        let source = Error::Filesystem(platform_io::fs_safety::Error::Io(
            std::io::Error::from_raw_os_error(raw),
        ));
        let source_code = source.code();
        let error = Error::PublishedWorkspaceReconciliation(Box::new(
            PublishedWorkspaceReconciliation::new(result.clone(), &workspace, source),
        ));
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert_eq!(error.published_result(), Some(result));
        let Error::PublishedWorkspaceReconciliation(context) = &error else {
            unreachable!()
        };
        let source = context.source().unwrap().downcast_ref::<Error>().unwrap();
        assert_eq!(source.code(), source_code);
        assert_eq!(
            source
                .source()
                .unwrap()
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(raw)
        );
        assert!(error.to_string().contains("is already published"));
        assert!(error.to_string().contains(&source.to_string()));
        assert!(!error.message().contains(&source.to_string()));
    }
}
