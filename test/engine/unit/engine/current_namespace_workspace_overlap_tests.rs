//! Combined normal saves use uploads-before-ordered-edits semantics. Synthetic
//! projection cases isolate namespace rules; the final case uses actual D3.
use super::*;

fn stage_rename(fixture: &mut Fixture, workspace: &Workspace, from: &str, to: &str) {
    fixture
        .engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        )
        .unwrap();
}
fn entry(fixture: &Fixture, path: &str) -> Entry {
    fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}
fn bytes(engine: &Engine, version: &str) -> Vec<u8> {
    let mut body = Vec::new();
    engine
        .open_content(version)
        .unwrap()
        .file
        .read_to_end(&mut body)
        .unwrap();
    body
}

#[test]
fn membership_registry_workspace_overlap_write_rename_keeps_new_body_node_and_receipt() {
    for (projected, file_manager) in [(false, true), (true, false), (true, true)] {
        let mut fixture = Fixture::new(file_manager);
        if projected {
            fixture.project(false);
        }
        let original = entry(&fixture, "dir/b");
        let commits = fixture.engine.state.commits.clone();
        let trees = fixture.engine.state.directory_trees.clone();
        let workspace = fixture
            .engine
            .create_workspace(&fixture.repo.main_branch_id, "admin")
            .unwrap();
        fixture
            .engine
            .stage_bytes(&workspace.workspace_id, "dir/b", b"new renamed body")
            .unwrap();
        stage_rename(&mut fixture, &workspace, "dir", "staging");
        stage_rename(&mut fixture, &workspace, "staging", "Moved");
        let request = request(&workspace);
        let result = fixture.engine.commit_workspace(request.clone()).unwrap();
        let current = entry(&fixture, "Moved/b");
        assert_eq!(current.file_id, original.file_id);
        assert_eq!(current.entry_id, original.entry_id);
        assert_ne!(current.version_id, original.version_id);
        assert_eq!(
            bytes(&fixture.engine, &current.version_id),
            b"new renamed body"
        );
        assert_eq!(bytes(&fixture.engine, &original.version_id), b"retained");
        let context = fixture
            .engine
            .file_context(&fixture.repo.repository_id, "Moved/b")
            .unwrap();
        let node = &fixture.engine.state.file_path_nodes[context.node_id.as_ref().unwrap()];
        assert_eq!(node.version_id, current.version_id);
        assert_eq!(node.parent_node_ids, vec![current.version_id.clone()]);
        assert_eq!(node.previous_path, "dir/b");
        assert_eq!(
            fixture.engine.state.branches[&context.branch_id]
                .file_node_id
                .as_deref(),
            Some(node.node_id.as_str())
        );
        assert!(
            fixture.engine.state.branches[&context.branch_id]
                .pending_file_versions
                .is_empty()
        );
        assert_eq!(fixture.engine.commit_workspace(request).unwrap(), result);
        for (id, commit) in commits.iter() {
            assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
        }
        for (id, tree) in trees.iter() {
            assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
        }
        if projected {
            assert!(
                fixture
                    .engine
                    .list_tree(&fixture.repo.main_branch_id)
                    .unwrap()
                    .iter()
                    .all(|entry| entry.entry_id != fixture.removed.entry_id)
            );
        }
    }
}

#[test]
fn membership_registry_workspace_overlap_deletions_preserve_unordered_staged_uploads() {
    for file_manager in [false, true] {
        let mut fixture = Fixture::new(file_manager);
        fixture.project(false);
        let workspace = fixture
            .engine
            .create_workspace(&fixture.repo.main_branch_id, "admin")
            .unwrap();
        let old = fixture
            .engine
            .stage_bytes(&workspace.workspace_id, "dir/b", b"retained staged edit")
            .unwrap();
        let fresh = fixture
            .engine
            .stage_bytes(
                &workspace.workspace_id,
                "dir/fresh",
                b"retained staged new file",
            )
            .unwrap();
        stage_rename(&mut fixture, &workspace, "dir", "removed");
        fixture
            .engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "removed", None)
                    .unwrap(),
            )
            .unwrap();
        let before = canonical_json(fixture.engine.state()).unwrap();
        assert_eq!(
            fixture
                .engine
                .commit_workspace(request(&workspace))
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceDependencyBusy
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        assert_eq!(
            std::fs::read(old.source_path).unwrap(),
            b"retained staged edit"
        );
        assert_eq!(
            std::fs::read(fresh.source_path).unwrap(),
            b"retained staged new file"
        );
        assert_eq!(
            bytes(&fixture.engine, &fixture.retained.version_id),
            b"retained"
        );
    }
}

#[test]
fn membership_registry_workspace_overlap_new_upload_moves_without_inheriting_hidden_identity() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/a", b"new independent file")
        .unwrap();
    stage_rename(&mut fixture, &workspace, "dir/a", "Fresh.txt");
    fixture
        .engine
        .commit_workspace(request(&workspace))
        .unwrap();
    let current = entry(&fixture, "Fresh.txt");
    assert_ne!(current.entry_id, fixture.removed.entry_id);
    assert_ne!(current.file_id, fixture.removed.file_id);
    assert_eq!(
        bytes(&fixture.engine, &current.version_id),
        b"new independent file"
    );
    assert_eq!(
        fixture.engine.state.file_locations[&current.file_id],
        "Fresh.txt"
    );
    assert_eq!(
        fixture.engine.state.file_names[&current.file_id],
        "Fresh.txt"
    );
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "Fresh.txt",
        b"follow-up save",
    );
    assert_eq!(entry(&fixture, "Fresh.txt").file_id, current.file_id);
}

#[test]
fn membership_registry_workspace_overlap_recovery_rejects_duplicate_and_reused_body() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(
            &workspace.workspace_id,
            "dir/b",
            b"published relocated body",
        )
        .unwrap();
    stage_rename(&mut fixture, &workspace, "dir/b", "moved");
    let previous = Arc::clone(&fixture.engine.state);
    let before_snapshot = fixture.engine.snapshot.clone();
    let source = fixture.engine.current_namespaces.clone();
    fixture
        .engine
        .commit_workspace(request(&workspace))
        .unwrap();
    let state: State =
        json_codec::from_slice(&canonical_json(fixture.engine.state()).unwrap()).unwrap();
    let snapshot = fixture.engine.snapshot.clone().unwrap();
    let moved = entry(&fixture, "moved");
    fixture.engine.state = Arc::clone(&previous);
    fixture.engine.snapshot = before_snapshot.clone();
    fixture.engine.current_namespaces = source;
    for historical in [false, true] {
        let mut forged = state.clone();
        let head = forged.branches[&fixture.repo.main_branch_id]
            .head_commit_id
            .clone();
        let mut commit = forged.commits[&head].clone();
        let mut duplicate = moved.clone();
        duplicate.relocate(name_policy::PortablePath::parse("copied").unwrap());
        if historical {
            duplicate.version_id = fixture.retained.version_id.clone();
            duplicate.size = fixture.retained.size;
        }
        commit.entries.push(duplicate);
        commit
            .entries
            .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
        forged.commits.insert(head, commit);
        assert_eq!(
            fixture
                .engine
                .install_recovered_namespace_state(forged, snapshot.clone())
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(fixture.engine.snapshot, before_snapshot);
    }
    fixture
        .engine
        .install_recovered_namespace_state(state, snapshot)
        .unwrap();
    assert_eq!(entry(&fixture, "moved"), moved);
    assert_eq!(
        bytes(&fixture.engine, &moved.version_id),
        b"published relocated body"
    );
}

#[test]
fn membership_registry_workspace_overlap_actual_d3_preserves_target_and_cold_restart() {
    let mut fixture = Fixture::new(true);
    let target = fixture.engine.config.disks[0].clone();
    let operation = op();
    fixture
        .engine
        .begin_disk_exit(
            &target.disk_id,
            MembershipExitAction::Deregister,
            "admin",
            operation,
        )
        .unwrap();
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    let joint = fixture.engine.resume_membership_joint_decision().unwrap();
    assert!(joint.certificate.is_some());
    fixture.engine.activate_pending_disk_exit().unwrap();
    let detached = fixture._temp.path().join("untouched-detached");
    std::fs::rename(&target.root, &detached).unwrap();
    std::fs::write(&target.root, b"excluded root must not be probed").unwrap();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/b", b"after actual D3")
        .unwrap();
    stage_rename(&mut fixture, &workspace, "dir", "after-exit");
    let request = request(&workspace);
    let result = fixture.engine.commit_workspace(request.clone()).unwrap();
    let moved = entry(&fixture, "after-exit/b");
    let system = fixture._temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system, RecoveryMode::CompletePending).unwrap();
    assert_eq!(
        engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("after-exit/b"),
        Some(&moved)
    );
    assert_eq!(bytes(&engine, &moved.version_id), b"after actual D3");
    assert_eq!(engine.commit_workspace(request).unwrap(), result);
    assert_eq!(
        std::fs::read(&target.root).unwrap(),
        b"excluded root must not be probed"
    );
    assert!(detached.is_dir());
}
