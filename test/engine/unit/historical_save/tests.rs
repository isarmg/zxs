use super::*;
#[test]
fn historical_edits_materialize_only_written_files_without_changing_the_source_head() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("history").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "unchanged.txt",
            b"independent source",
        )
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "edited.txt", b"old complete file")
        .unwrap();
    let saved = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Original".into(),
            request_digest: None,
        })
        .unwrap();
    let source = saved.commit_id.unwrap();
    let historical = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &historical.workspace_id,
            "edited.txt",
            b"historical replacement",
        )
        .unwrap();
    let ordinary = engine.commit_workspace(CommitRequest {
        workspace_id: historical.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: historical.base_head.clone(),
        fencing_token: historical.fencing_token,
        message: "must refuse".into(),
        request_digest: None,
    });
    assert!(ordinary.is_err());
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source.clone(),
                name: "from history".into(),
                workspace_id: Some(historical.workspace_id),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let sequence = engine.state.sequence;
    let operation = storage_contract::OperationId::generate();
    let saved = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(engine.state.sequence, sequence + 1);
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].head_commit_id,
        source
    );
    let branch = saved.branch_id.clone().unwrap();
    let entries = engine.list_tree(&branch).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        saved.result["version_protection"].as_array().unwrap().len(),
        1
    );
    let source_entries = engine
        .namespace_entries(&engine.state.commits[&source].namespace)
        .unwrap();
    let old = source_entries
        .iter()
        .find(|entry| entry.path == "unchanged.txt")
        .unwrap();
    let new = entries
        .iter()
        .find(|entry| entry.path == "unchanged.txt")
        .unwrap();
    assert_eq!(new.version_id, old.version_id);
    assert_eq!(new.size, old.size);
    assert_eq!(
        engine.pending_file_source(&branch, &new.file_id),
        Some(old.version_id.as_str())
    );
    assert_eq!(saved.result["written_files"], 1);
    assert_eq!(saved.result["placeholder_files"], 1);
    let edited = entries
        .iter()
        .find(|entry| entry.path == "edited.txt")
        .unwrap();
    let mut content = engine.open_content(&edited.version_id).unwrap();
    let mut bytes = Vec::new();
    use std::io::Read;
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"historical replacement");
    drop(content);
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap()
            .commit_id,
        saved.commit_id
    );
}

#[test]
fn historical_placeholders_need_no_old_payload_and_written_replacements_materialize() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("purged history").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut commits = Vec::new();
    for bytes in [b"old full file".as_slice(), b"current full file".as_slice()] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        let operation = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Save".into(),
                request_digest: None,
            })
            .unwrap();
        commits.push(operation.commit_id.unwrap());
    }
    let old_version = engine
        .namespace_entries(&engine.state.commits[&commits[0]].namespace)
        .unwrap()[0]
        .version_id
        .clone();
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![old_version], "admin")
        .unwrap();
    engine
        .commit_plan(
            &purge.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let options = HistoricalSaveOptions {
        source_commit_id: commits[0].clone(),
        name: "requires missing bytes".into(),
        workspace_id: None,
        primary_slot_id: None,
        target_copies: None,
    };
    let version_count = engine.state.versions.len();
    let placeholder_plan = engine
        .create_historical_save_plan(&repo.history_tree_id, options, "admin")
        .unwrap();
    assert_eq!(placeholder_plan.input["full_copy_files"], 0);
    assert_eq!(placeholder_plan.input["placeholder_files"], 1);
    let placeholder = engine
        .commit_historical_save_plan(
            &placeholder_plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(engine.state.versions.len(), version_count);
    let inherited = engine
        .list_tree(placeholder.branch_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        inherited[0].version_id,
        engine
            .namespace_entries(&engine.state.commits[&commits[0]].namespace)
            .unwrap()[0]
            .version_id
    );
    assert_eq!(
        engine
            .open_content(&inherited[0].version_id)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ContentPurged
    );
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &commits[0], "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "file.txt",
            b"explicit complete replacement",
        )
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: commits[0].clone(),
                name: "explicit historical replacement".into(),
                workspace_id: Some(workspace.workspace_id),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let saved = engine
        .commit_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].head_commit_id,
        commits[1]
    );
    assert!(saved.branch_id.is_some());
}

#[test]
fn historical_namespace_deletion_is_recorded_only_on_execution_and_never_inherited() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("historical deletion").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut commits = Vec::new();
    for bytes in [b"old body".as_slice(), b"current body".as_slice()] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        commits.push(
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: storage_contract::OperationId::generate(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "source".into(),
                    request_digest: None,
                })
                .unwrap()
                .commit_id
                .unwrap(),
        );
    }
    let old_version = engine
        .namespace_entries(&engine.state.commits[&commits[0]].namespace)
        .unwrap()[0]
        .version_id
        .clone();
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![old_version.clone()], "admin")
        .unwrap();
    engine
        .commit_plan(
            &purge.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.state.content_states[&old_version],
        storage_contract::ContentState::Purged
    );
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &commits[0], "admin")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "file.txt",
                None,
            )
            .unwrap(),
        )
        .unwrap();
    let before = engine.state.commits.clone();
    let version_count = engine.state.versions.len();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: commits[0].clone(),
                name: "delete old file".into(),
                workspace_id: Some(workspace.workspace_id),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    assert_eq!(
        engine.state.commits, before,
        "planning must not append operation facts"
    );
    assert_eq!(plan.input["full_copy_files"], 0);
    assert_eq!(plan.input["placeholder_files"], 0);
    let saved = engine
        .commit_historical_save_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let commit = saved.commit_id.unwrap();
    let historical = &engine.state.commits[&commit];
    assert_eq!(historical.namespace.entries, 0);
    let facts = historical.namespace_deletions.as_ref().unwrap();
    assert_eq!(facts.paths.len(), 1);
    assert_eq!(facts.paths[0].path, "file.txt");
    assert_eq!(facts.paths[0].kind, NamespaceEntryKind::File);
    assert_eq!(saved.result["full_copy_files"], 0);
    assert_eq!(engine.state.versions.len(), version_count);
    assert_eq!(
        engine.state.content_states[&old_version],
        storage_contract::ContentState::Purged
    );
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].head_commit_id,
        commits[1]
    );
    let branch = engine
        .create_branch(
            &commit,
            &storage_contract::BranchName::parse("ordinary copy").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine.state.commits[&branch.head_commit_id]
            .namespace_deletions
            .is_none()
    );
    let copy = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: commit,
                name: "historical copy".into(),
                workspace_id: None,
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let copied = engine
        .commit_historical_save_plan(
            &copy.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine.state.commits[copied.commit_id.as_ref().unwrap()]
            .namespace_deletions
            .is_none()
    );
    assert_eq!(engine.state.versions.len(), version_count);
}

fn retirement_fixture() -> (tempfile::TempDir, Engine, Repository, String) {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([root.path().join("primary")]);
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("historical retirement").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let source = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    (root, engine, repo, source)
}

fn retirement_plan(
    engine: &mut Engine,
    repo: &Repository,
    source: &str,
    name: &str,
) -> (Workspace, Plan, StagedFile) {
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, source, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(
            &workspace.workspace_id,
            "retained.txt",
            b"historical retirement bytes",
        )
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source.into(),
                name: name.into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    (workspace, plan, staged)
}

#[test]
fn published_historical_workspaces_release_records_without_losing_bytes_or_replay() {
    let (root, mut engine, repo, source) = retirement_fixture();
    let mut saved = Vec::new();
    let mut previous_bytes = 0;
    for index in 0..4 {
        let (workspace, plan, staged) =
            retirement_plan(&mut engine, &repo, &source, &format!("retired {index}"));
        let operation = OperationId::generate();
        let result = engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        let capacity = engine.workspace_capacity("admin");
        assert_eq!(capacity.principal.records.used, 0);
        assert_eq!(capacity.principal.cold_records, index + 1);
        assert!(capacity.principal.metadata_bytes.used > previous_bytes);
        previous_bytes = capacity.principal.metadata_bytes.used;
        assert!(!engine.workspaces.contains_key(&workspace.workspace_id));
        assert_eq!(staged.source_path.extension().unwrap(), "full");
        assert_eq!(
            std::fs::read(&staged.source_path).unwrap(),
            b"historical retirement bytes"
        );
        saved.push((workspace, plan, staged, result));
        drop(engine);
        engine = Engine::open(&root.path().join("system")).unwrap();
        assert_eq!(engine.workspace_capacity("admin"), capacity);
        for (workspace, plan, staged, result) in &saved {
            let retained = engine.workspace(&workspace.workspace_id).unwrap();
            assert_eq!(retained.state, WorkspaceState::Committed);
            assert_eq!(retained.fencing_token, workspace.fencing_token + 1);
            assert!(!engine.workspaces.contains_key(&workspace.workspace_id));
            assert_eq!(
                std::fs::read(&staged.source_path).unwrap(),
                b"historical retirement bytes"
            );
            assert_eq!(
                engine
                    .commit_historical_save_plan(&plan.plan_id, "admin", result.operation_id)
                    .unwrap(),
                *result
            );
        }
    }
}

#[test]
fn published_historical_abort_stays_cold_while_unpublished_work_consumes_the_limit() {
    let (root, mut engine, repo, source) = retirement_fixture();
    let (workspace, plan, staged) = retirement_plan(&mut engine, &repo, &source, "published");
    let result = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", OperationId::generate())
        .unwrap();
    let unpublished = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &unpublished.workspace_id,
            "unpublished.txt",
            b"not published",
        )
        .unwrap();
    engine
        .abort_workspace(&unpublished.workspace_id, "admin")
        .unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    for _ in 0..2 {
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
    }
    let closed = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(closed.state, WorkspaceState::Aborted);
    assert_eq!(closed.fencing_token, workspace.fencing_token + 3);
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.cold_records, 1);
    drop(engine);
    let mut engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        canonical_json(&closed).unwrap()
    );
    assert!(engine.workspaces.contains_key(&unpublished.workspace_id));
    assert_eq!(
        std::fs::read(&staged.source_path).unwrap(),
        b"historical retirement bytes"
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", result.operation_id)
            .unwrap(),
        result
    );
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .fencing_token,
        closed.fencing_token + 1
    );
    assert_eq!(engine.workspace_capacity("admin"), capacity);
}

#[test]
fn failed_historical_completion_mirror_does_not_release_resident_quota() {
    let (_root, mut engine, repo, source) = retirement_fixture();
    let (workspace, plan, staged) = retirement_plan(&mut engine, &repo, &source, "mirror failure");
    let manifest = engine
        .root
        .join("workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let retained = manifest.with_extension("retained");
    std::fs::rename(&manifest, &retained).unwrap();
    std::fs::create_dir(&manifest).unwrap();
    let operation = OperationId::generate();
    assert!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .is_err()
    );
    let result = engine.published_receipt(operation).unwrap().unwrap();
    assert_eq!(result.kind, "history.historical_save");
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.records.used, 1);
    assert_eq!(capacity.principal.cold_records, 0);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(
        std::fs::read(&staged.source_path).unwrap(),
        b"historical retirement bytes"
    );
    std::fs::remove_dir(&manifest).unwrap();
    std::fs::rename(retained, manifest).unwrap();
}

#[test]
fn cold_historical_workspace_remains_a_full_payload_recovery_source() {
    use std::io::{Read, Seek};
    use std::os::unix::fs::MetadataExt;
    let (root, mut engine, repo, source) = retirement_fixture();
    let (workspace, plan, staged) = retirement_plan(&mut engine, &repo, &source, "recoverable");
    let result = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", OperationId::generate())
        .unwrap();
    let branch = result.branch_id.as_ref().unwrap();
    let version_id = engine.list_tree(branch).unwrap()[0].version_id.clone();
    let version = engine.state.versions[&version_id].clone();
    let placement = engine.state.placements[&version_id].clone();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    for slot in &placement.route_slot_ids {
        let disk = engine.disk(slot).unwrap();
        let held = engine.disk_resources.get(disk).unwrap();
        let directory = crate::disk::version_dir(disk, &version.history_tree_id, &version_id);
        let body = crate::container_storage::content_path_at(
            &held.root,
            &directory,
            &version,
            &mut |_| Ok(()),
        )
        .unwrap();
        std::fs::remove_file(body).unwrap();
    }
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
    assert!(!engine.workspaces.contains_key(&workspace.workspace_id));
    let recovery = engine
        .pending_payload_source(&version, &placement)
        .unwrap()
        .unwrap();
    let mut recovered_source = recovery.open().unwrap();
    assert_eq!(
        recovered_source.metadata().unwrap().ino(),
        std::fs::metadata(&staged.source_path).unwrap().ino(),
    );
    let mut actual = Vec::new();
    recovered_source.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, b"historical retirement bytes");
    let members = engine.members();
    let hash = crate::journal::scan(&members[0])
        .unwrap()
        .records
        .into_iter()
        .find(|(record, _)| {
            record.sequence == result.sequence
                && record.record_type == crate::journal::RecordType::Accept
        })
        .unwrap()
        .0
        .candidate_hash;
    let candidate: crate::journal::Candidate = crate::publication_evidence::verified_object_at(
        &engine.disk_resources,
        &members,
        "candidate",
        &hash,
    )
    .unwrap();
    engine.repair_pending_payloads(&candidate).unwrap();
    for slot in &placement.route_slot_ids {
        let disk = engine.disk(slot).unwrap();
        let directory = crate::disk::version_dir(disk, &version.history_tree_id, &version_id);
        let mut body = engine
            .disk_resources
            .get(disk)
            .unwrap()
            .verify_payload(&directory, &version)
            .unwrap();
        body.rewind().unwrap();
        let mut repaired = Vec::new();
        body.read_to_end(&mut repaired).unwrap();
        assert_eq!(repaired, b"historical retirement bytes");
    }
    assert_eq!(
        std::fs::read(staged.source_path).unwrap(),
        b"historical retirement bytes"
    );
}
