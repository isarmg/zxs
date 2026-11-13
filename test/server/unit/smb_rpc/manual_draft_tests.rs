use super::*;

fn manual_engine(root: &Path) -> Engine {
    let system = root.join("state");
    let disk = root.join("p");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    Engine::initialize_manual_container_storage(
        &system, Config::development([disk]).unwrap(), OperationId::generate().unwrap(),
    ).unwrap()
}

fn manual_fixture() -> (Fixture, Arc<Shared>, Arc<LostCommitReply>, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = manual_engine(temp.path());
    let repo = engine.create_repository(
        &storage_contract::RepositoryName::parse("files").unwrap(),
        "P01", 1, "alice", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "report.txt", b"old").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "initial".into(),
        request_digest: None,
    }).unwrap();
    let (fixture, shared, backend) = fixture_backend(temp, engine);
    (fixture, shared, backend, repo.main_branch_id)
}

#[test]
fn manual_flush_close_and_reopen_writer_keep_shared_body_without_advancing_head() {
    let (_root, shared, _backend, branch) = manual_fixture();
    let identity = identity(&shared);
    let entry = session(Arc::clone(&shared), &branch).entries().unwrap().remove(0);
    let context = crate::ReadContainerEdit {
        file: entry.file_id.parse().unwrap(),
        repository: repository(&shared).parse().unwrap(),
        path: entry.path.clone(),
    };
    shared.backend.set_container_commit_mode(&identity, crate::SetContainerCommitMode {
        context: context.clone(), expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    let head = shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id;
    let mut client = session(Arc::clone(&shared), &branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"draft", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let first = client.flush(opened.handle).unwrap();
    assert_eq!(first["draft_persisted"], true);
    assert!(first.get("commit_id").is_none());
    assert_eq!(shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id, head);
    assert_eq!(client.entries().unwrap()[0].version_id, entry.version_id);
    client.close(opened.handle, false).unwrap();
    drop(file);
    let current = shared.backend.current_container_content(&identity, context.clone()).unwrap();
    assert!(current.edit.draft().is_some());
    assert_eq!(current.content.size, 5);
    let mut original = shared.backend.content(&identity, &entry.version_id).unwrap();
    let mut bytes = Vec::new();
    original.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"old");
    let (directory, descriptor) = client.open("".into(), false, false, false, false, false)
        .unwrap();
    drop(descriptor);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, true)
        .unwrap();
    let file = File::from(descriptor);
    let mut bytes = [0u8; 5];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"draft");
    client.prepare_write(opened.handle, 11, 11).unwrap();
    file.write_all_at(b"again again", 0).unwrap();
    let second = client.dirty(opened.handle).unwrap();
    assert_eq!(second["draft_persisted"], true);
    assert_ne!(second["operation_id"], first["operation_id"]);
    client.close(opened.handle, false).unwrap();
    assert_eq!(shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id, head);
    assert_eq!(client.entries().unwrap()[0].version_id, entry.version_id);
    let stat = client.stat("report.txt").unwrap();
    assert_eq!(stat.size, 11);
    let frozen = client.read_directory(directory.handle, false).unwrap();
    assert_eq!(frozen.entries[0].entry.size, 5);
    let refreshed = client.read_directory(directory.handle, true).unwrap();
    assert_eq!(refreshed.entries[0].entry.size, 11);
    client.close(directory.handle, false).unwrap();
    let (opened, descriptor) = client.open("report.txt".into(), false, false, false, false, false)
        .unwrap();
    assert!(opened.version_id.is_none());
    assert_eq!(opened.size, 11);
    drop(descriptor);
    assert_eq!(client.read(opened.handle, 1, 3).unwrap().as_ref(), b"gai");
    client.close(opened.handle, false).unwrap();
}

fn shared_alias_fixture() -> (Fixture, Arc<Shared>, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = manual_engine(temp.path());
    let repo = engine.create_repository(&storage_contract::RepositoryName::parse("aliases").unwrap(),
        "P01", 1, "alice", OperationId::generate().unwrap()).unwrap();
    for (path, bytes) in [("a/report.txt", b"original".as_slice()), ("b/keep", b"keep".as_slice())] {
        let work = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
        engine.stage_bytes(&work.workspace_id, path, bytes).unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: work.workspace_id, operation_id: OperationId::generate().unwrap(),
            principal: "alice".into(), expected_head: work.base_head,
            fencing_token: work.fencing_token, message: "fixture content".into(), request_digest: None,
        }).unwrap();
    }
    let head = engine.state().branches[&repo.main_branch_id].head_commit_id.clone();
    engine.initialize_directory_git(&repo.repository_id,
        &storage_contract::directory_git::DirectoryGitInitializeBody {
            path: "".into(), expected_namespace_head: head,
            operation_id: OperationId::generate().unwrap(),
        }, "alice").unwrap();
    let status = engine.directory_git_status(&repo.repository_id, "").unwrap();
    engine.stage_directory_git_owned(&repo.repository_id,
        &storage_contract::directory_git::DirectoryGitStageOwnedBody {
            directory_id: status.directory_id,
            directory_branch_id: status.directory_branch_id.unwrap(),
            expected_tree_id: status.working_tree_id.unwrap(),
            expected_index_generation: status.index_generation,
            operation_id: OperationId::generate().unwrap(),
        }, "alice").unwrap();
    let status = engine.directory_git_status(&repo.repository_id, "").unwrap();
    engine.commit_directory_changes(storage_engine::DirectoryCommitRequest {
        repository_id: &repo.repository_id,
        directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
        expected_tree_id: status.working_tree_id.as_ref().unwrap(),
        expected_index_generation: status.index_generation, message: "fixture checkpoint",
        principal: "alice", operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    let main = status.directory_branch_id.unwrap();
    let transfer = engine.create_directory_branches(&repo.repository_id, None,
        &storage_contract::BranchName::parse("transfer").unwrap(), "alice",
        OperationId::generate().unwrap()).unwrap().result["directory_branch_id"]
        .as_str().unwrap().to_owned();
    let source = engine.state().directory_branches[&transfer].entries.iter()
        .find(|entry| entry.path() == "a/report.txt").unwrap().clone();
    let request = storage_contract::DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Move,
        items: vec![storage_contract::DirectoryNamespaceItem {
            source: storage_contract::DirectoryNamespaceSource {
                directory_branch_id: transfer.clone(),
                expected_tree_id: engine.state().directory_branches[&transfer].head_tree_id.clone(),
                path: "a/report.txt".into(), entry_id: source.entry_id,
            },
            target: Some(storage_contract::DirectoryNamespaceTarget {
                directory_branch_id: main.clone(),
                expected_tree_id: engine.state().directory_branches[&main].head_tree_id.clone(),
                path: "b/report.txt".into(),
            }),
        }],
    };
    engine.directory_namespace_batch(&repo.repository_id, &request, "alice",
        OperationId::generate().unwrap()).unwrap();
    let (fixture, shared, _) = fixture_backend(temp, engine);
    (fixture, shared, repo.main_branch_id)
}

#[test]
fn writing_shared_draft_through_another_occurrence_retains_original_source_proof() {
    let (_fixture, shared, branch) = shared_alias_fixture();
    let identity = identity(&shared);
    let mut client = session(Arc::clone(&shared), &branch);
    let entries = client.entries().unwrap();
    let original = entries.iter().find(|entry| entry.path() == "a/report.txt").unwrap();
    let alias = entries.iter().find(|entry| entry.path() == "b/report.txt").unwrap();
    assert_eq!(original.file_id, alias.file_id);
    // Moving one retained branch occurrence preserves its entry identity.
    // The two current occurrences are distinguished by their path as well.
    assert_eq!(original.entry_id, alias.entry_id);
    assert_ne!(
        (&original.entry_id, original.path()),
        (&alias.entry_id, alias.path()),
    );
    let context = crate::ReadContainerEdit {
        file: original.file_id.parse().unwrap(), repository: repository(&shared).parse().unwrap(),
        path: original.path.clone(),
    };
    shared.backend.set_container_commit_mode(&identity, crate::SetContainerCommitMode {
        context: context.clone(), expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    let head = shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id;
    let mut first_base = None;
    for (path, bytes) in [("a/report.txt", b"first!!!"), ("b/report.txt", b"second!!")] {
        let (opened, descriptor) = client.open(path.into(), true, false, false, false, false)
            .unwrap();
        let file = File::from(descriptor);
        client.prepare_write(opened.handle, 8, 8).unwrap();
        file.write_all_at(bytes, 0).unwrap();
        client.dirty(opened.handle).unwrap();
        assert_eq!(client.flush(opened.handle).unwrap()["draft_persisted"], true);
        client.close(opened.handle, false).unwrap();
        let edit = shared.backend.read_container_edit(&identity, context.clone()).unwrap();
        let base = edit.draft().unwrap().base.clone();
        assert_eq!(base.entry.to_string(), original.entry_id);
        if let Some(first) = &first_base { assert_eq!(&base, first); }
        else { first_base = Some(base); }
        assert_eq!(shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id, head);
    }
    let mut current = shared.backend.current_container_content(&identity, context).unwrap().content;
    let mut bytes = Vec::new();
    current.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"second!!");
}

#[path = "manual_capacity_tests.rs"]
mod capacity;
