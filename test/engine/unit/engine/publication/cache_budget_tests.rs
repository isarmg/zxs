use super::*;

#[test]
fn cache_encoder_rejects_before_accept_and_reopens_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository("cache", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"bytes")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let file = next.versions.values().next().unwrap().file_id.clone();
    // Valid metadata must reach the deliberately tiny cache budget; an invalid
    // oversized note would fail history validation before exercising the encoder.
    let note = "n".repeat(storage_contract::MAX_NOTE_BYTES);
    storage_contract::validate_note(&note).unwrap();
    next.file_notes.insert(file, note);
    let mut result = next.operations.values().next().unwrap().clone();
    result.operation_id = new_id("op");
    result.sequence = next.sequence;
    next.operations
        .insert(result.operation_id.clone(), result.clone());
    let logs: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect();
    let error = engine
        .prepare_publication_bounded(next.clone(), Vec::new(), result.clone(), MAX_METADATA, 128)
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine
            .members()
            .iter()
            .map(|disk| std::fs::read(log_path(disk)).unwrap())
            .collect::<Vec<_>>(),
        logs
    );
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine.operation(&result.operation_id).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound
    );
    drop(engine);
    let mut engine = Engine::open_for_recovery(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let detached = engine
        .prepare_publication_bounded(next, Vec::new(), result.clone(), MAX_METADATA, MAX_METADATA)
        .unwrap();
    engine.inject_fault_once("after_accept_1");
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        storage_contract::ErrorCode::InDoubt
    );
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(&result.operation_id).unwrap().operation_id,
        result.operation_id
    );
}

#[test]
fn payload_cache_rejection_preserves_names_and_retries_installed_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository("cache", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"published bytes")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let old_entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let old_version = engine.state.versions[&old_entry.version_id].clone();
    let original_paths: Vec<_> = engine
        .config
        .disks
        .iter()
        .map(|disk| {
            let directory = version_dir(disk, &repo.history_tree_id, &old_entry.version_id);
            let content = crate::container_storage::content_path(&directory, &old_version).unwrap();
            let cache = content
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("container.json");
            let cache_bytes = std::fs::read(&cache).unwrap();
            (directory, content, cache, cache_bytes)
        })
        .collect();
    let before = canonical_json(engine.state()).unwrap();
    let original_snapshot = engine.snapshot.clone();
    let original_slot = engine.next_slot;
    let logs: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let source = root.join("unpublished-source");
    std::fs::write(&source, b"replacement bytes").unwrap();
    let mut pending = Vec::new();
    let version = engine
        .build_version(
            &mut next,
            &mut pending,
            crate::business::VersionBuild {
                history: &repo.history_tree_id,
                file_id: &old_entry.file_id,
                path: "renamed.txt",
                source: &source,
                primary: "P01",
                copies: 3,
                source_ids: vec![old_entry.version_id.clone()],
                checkpoint: false,
            },
        )
        .unwrap();
    let original = next.commits[&next.branches[&repo.main_branch_id].head_commit_id].clone();
    let mut commit = original.clone();
    commit.commit_id = new_id("commit");
    commit.parent_commit_ids = vec![original.commit_id];
    commit.sequence = next.sequence;
    commit.kind = "content".into();
    commit.entries = vec![Entry {
        entry_id: new_id("entry"),
        path: "renamed.txt".into(),
        name: "renamed.txt".into(),
        lookup_key: "renamed.txt".into(),
        kind: NamespaceEntryKind::File,
        file_id: old_entry.file_id.clone(),
        version_id: version.version_id.clone(),
        size: version.logical_size,
    }];
    next.branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .head_commit_id = commit.commit_id.clone();
    next.commits.insert(commit.commit_id.clone(), commit);
    next.file_names
        .insert(old_entry.file_id.clone(), "renamed.txt".into());
    next.file_locations
        .insert(old_entry.file_id.clone(), "renamed.txt".into());
    let mut result = next.operations.values().next().unwrap().clone();
    result.operation_id = new_id("op");
    result.sequence = next.sequence;
    next.operations
        .insert(result.operation_id.clone(), result.clone());
    let retry_pending = pending
        .iter()
        .map(|payload| Prepared {
            version: payload.version.clone(),
            placement: payload.placement.clone(),
            source: payload.source.clone(),
        })
        .collect();
    let error = engine
        .prepare_publication_bounded(next.clone(), pending, result.clone(), MAX_METADATA, 128)
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.snapshot, original_snapshot);
    assert_eq!(engine.next_slot, original_slot);
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].path,
        "file.txt"
    );
    assert_eq!(
        engine
            .members()
            .iter()
            .map(|disk| std::fs::read(log_path(disk)).unwrap())
            .collect::<Vec<_>>(),
        logs
    );
    for (directory, content, cache, cache_bytes) in &original_paths {
        assert_eq!(
            crate::container_storage::content_path(directory, &old_version).unwrap(),
            *content
        );
        assert_eq!(std::fs::read(content).unwrap(), b"published bytes");
        assert_eq!(std::fs::read(cache).unwrap(), *cache_bytes);
    }
    for disk in &engine.config.disks {
        // Installation happened, but no publication proof or visible rename did.
        let directory = version_dir(disk, &repo.history_tree_id, &version.version_id);
        verify_payload(&directory, &version).unwrap();
        assert!(!directory.join("commit-proof.json").exists());
    }
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let detached = engine
        .prepare_publication_bounded(
            next,
            retry_pending,
            result.clone(),
            MAX_METADATA,
            MAX_METADATA,
        )
        .unwrap();
    engine.decide_publication(detached).unwrap();
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].path,
        "renamed.txt"
    );
    for disk in &engine.config.disks {
        let directory = version_dir(disk, &repo.history_tree_id, &version.version_id);
        let content = crate::container_storage::content_path(&directory, &version).unwrap();
        assert_eq!(
            content
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .file_name()
                .unwrap(),
            "renamed.txt"
        );
        assert_eq!(std::fs::read(content).unwrap(), b"replacement bytes");
    }
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(&result.operation_id).unwrap().operation_id,
        result.operation_id
    );
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id,
        version.version_id
    );
}
