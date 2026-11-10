//! The former admission and receipt schemas are independent regression oracles.
use super::*;

fn reference(
    state: &State,
    repository: &Repository,
    view: Option<&crate::engine::CurrentNamespaces>,
) -> Result<Commit> {
    let main = state
        .branches
        .get(&repository.main_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "directory branch missing",
            )
        })?;
    if main.state != BranchState::Active {
        return Err(Error::new(
            storage_contract::ErrorCode::BranchNotActive,
            "directory is not writable",
        ));
    }
    let mut current = state
        .commits
        .get(&main.head_commit_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory head missing",
            )
        })?;
    if let Some(view) = view {
        view.require_branch_write(state, &main.branch_id)?;
        current.entries = view
            .branch_entries(&main.branch_id, &current.commit_id)?
            .to_vec();
        let live: BTreeSet<&str> = current
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .map(|entry| entry.file_id.as_str())
            .collect();
        current
            .pending_file_versions
            .retain(|file, _| live.contains(file.as_str()));
    }
    Ok(current)
}

fn fixture(current: bool) -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..4).map(|i| temp.path().join(format!("disk-{i}")))).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"retained bytes")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "file namespace fixture".into(),
            request_digest: None,
        })
        .unwrap();
    if current {
        engine
            .begin_disk_exit(
                &config.disks[3].disk_id,
                crate::MembershipExitAction::Deregister,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        assert!(
            engine
                .resume_membership_joint_decision()
                .unwrap()
                .all_certificates_synchronized()
        );
        engine.activate_pending_disk_exit().unwrap();
    }
    (temp, engine, repo)
}

#[test]
fn writable_file_namespace_matches_both_original_admissions_without_side_effects() {
    for current in [false, true] {
        let (temp, mut engine, repo) = fixture(current);
        let view = engine.admitted_current_namespaces().unwrap();
        assert_eq!(view.is_some(), current);
        let before = json_codec::to_vec(engine.state()).unwrap();
        let expected = reference(engine.state(), &repo, view).unwrap();
        assert_eq!(
            writable_file_namespace(engine.state(), &repo, view).unwrap(),
            expected
        );
        assert_eq!(json_codec::to_vec(engine.state()).unwrap(), before);
        for defect in 0..4 {
            let mut candidate = engine.state().clone();
            match defect {
                0 => {
                    candidate.branches.remove(&repo.main_branch_id);
                }
                1 => {
                    candidate
                        .branches
                        .get_mut(&repo.main_branch_id)
                        .unwrap()
                        .state = BranchState::Archived;
                }
                2 => {
                    candidate
                        .branches
                        .get_mut(&repo.main_branch_id)
                        .unwrap()
                        .state = BranchState::Merged;
                }
                3 => {
                    let head = candidate.branches[&repo.main_branch_id]
                        .head_commit_id
                        .clone();
                    candidate.commits.remove(&head);
                }
                _ => unreachable!(),
            }
            let expected = reference(&candidate, &repo, view).unwrap_err();
            let actual = writable_file_namespace(&candidate, &repo, view).unwrap_err();
            assert_eq!(actual.code(), expected.code());
            assert_eq!(actual.to_string(), expected.to_string());
        }
        let original = engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("file.txt")
            .unwrap()
            .clone();
        let contexts = engine.state().file_contexts.clone();
        let versions = engine.state().versions.clone();
        let operation = OperationId::generate().unwrap();
        let name = storage_contract::BranchName::parse("pending draft").unwrap();
        let created = engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                &name,
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(
            engine
                .create_file_branch(
                    &original.file_id,
                    &original.version_id,
                    &name,
                    "admin",
                    operation
                )
                .unwrap(),
            created
        );
        assert_eq!(created.result, expected_receipt(&created, &original));
        assert_eq!(engine.state().file_contexts, contexts);
        assert_eq!(engine.state().versions, versions);
        let from_node_operation = OperationId::generate().unwrap();
        let from_node_name = storage_contract::BranchName::parse("from node").unwrap();
        let from_node = engine
            .create_file_branch_from_node(
                &original.file_id,
                &original.version_id,
                &original.version_id,
                &from_node_name,
                "admin",
                from_node_operation,
            )
            .unwrap();
        let mut expected = expected_receipt(&from_node, &original);
        expected["source_node_id"] = json!(original.version_id);
        assert_eq!(from_node.result, expected);
        assert_eq!(
            engine.state().branches[from_node.branch_id.as_ref().unwrap()]
                .file_node_id
                .as_deref(),
            Some(original.version_id.as_str()),
        );
        assert_eq!(engine.state().file_contexts, contexts);
        assert_eq!(engine.state().versions, versions);
        let selected = engine
            .select_file_branch(
                &original.file_id,
                created.branch_id.as_ref().unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        assert!(selected.branch_id.is_some());
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(
            engine
                .create_file_branch(
                    &original.file_id,
                    &original.version_id,
                    &name,
                    "admin",
                    operation
                )
                .unwrap(),
            created
        );
        let before_retry = json_codec::to_vec(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_file_branch_from_node(
                    &original.file_id,
                    &original.version_id,
                    &original.version_id,
                    &from_node_name,
                    "admin",
                    from_node_operation,
                )
                .unwrap(),
            from_node
        );
        assert_eq!(json_codec::to_vec(engine.state()).unwrap(), before_retry);
        let view = engine.admitted_current_namespaces().unwrap();
        assert_eq!(
            writable_file_namespace(engine.state(), &repo, view).unwrap(),
            reference(engine.state(), &repo, view).unwrap()
        );
        assert_eq!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get("file.txt")
                .unwrap()
                .version_id,
            original.version_id
        );
    }
}

fn expected_receipt(result: &OperationResult, source: &Entry) -> json_codec::Value {
    json!({
        "file_id": source.file_id,
        "branch_id": result.branch_id.as_ref().unwrap(),
        "commit_id": result.commit_id.as_ref().unwrap(),
        "history_tree_id": result.history_tree_id.as_ref().unwrap(),
        "copied_files": "0",
        "placeholder_files": "0",
        "materialized": false,
        "inherited": true,
        "own_version_id": null,
        "version_id": source.version_id,
        "source_version_id": source.version_id,
        "content_state": "PENDING",
        "selected": false,
    })
}
