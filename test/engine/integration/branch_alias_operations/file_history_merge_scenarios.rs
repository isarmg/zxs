use super::*;
use std::os::unix::fs::DirBuilderExt;
use storage_contract::{BranchName, ErrorCode, FileHistoryMergeInput, OperationId};

fn fork(p: &mut Pool, file: &str, node: &str, name: &str) -> String {
    let version = p.engine.file_node_version(file, node).unwrap();
    p.engine
        .create_file_branch_from_node(
            file,
            &version,
            node,
            &BranchName::parse(name).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .into()
}

fn select(p: &mut Pool, file: &str, branch: &str) {
    p.engine
        .select_file_branch_at(
            &p.repo.repository_id,
            &name_policy::PortablePath::parse("report.txt").unwrap(),
            file,
            branch,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
}

fn merge(
    p: &mut Pool,
    file: &str,
    inputs: &[FileHistoryMergeInput],
    chosen: &str,
    name: &str,
    operation: OperationId,
) -> storage_engine::Result<OperationResult> {
    p.engine
        .merge_file_history(storage_engine::FileHistoryMergeRequest {
            file_id: file,
            inputs,
            chosen_node_id: chosen,
            name: &BranchName::parse(name).unwrap(),
            location: None,
            principal: "admin",
            operation_id: operation,
        })
}

#[test]
fn historical_nodes_on_one_branch_merge_without_changing_original_heads_or_default() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let source = fork(&mut p, &base.file_id, &base.version_id, "source");
    select(&mut p, &base.file_id, &source);
    p.save("report.txt", b"middle");
    let middle = p.engine.file_branch_head(&source).unwrap();
    p.save("report.txt", b"newer must survive");
    let latest = p.engine.file_branch_head(&source).unwrap();
    let inputs = [&base.version_id, &middle].map(|node| FileHistoryMergeInput {
        branch_id: source.clone(),
        source_node_id: node.clone(),
        expected_node_id: latest.clone(),
    });
    let original_parents = inputs
        .iter()
        .map(|input| {
            (
                input.source_node_id.clone(),
                p.engine
                    .file_node_parents(&base.file_id, &input.source_node_id)
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let before = p.engine.state().clone();
    let operation = OperationId::generate().unwrap();
    let receipt = merge(
        &mut p,
        &base.file_id,
        &inputs,
        &middle,
        "Historical result",
        operation,
    )
    .unwrap();
    let target = receipt.result["branch_id"].as_str().unwrap().to_owned();
    let result_node = receipt.result["node_id"].as_str().unwrap();
    assert!(result_node.starts_with("fmeta_"));
    assert!(
        inputs
            .iter()
            .all(|input| input.source_node_id != result_node)
    );
    assert_eq!(receipt.result["reused_node"], false);
    assert_eq!(
        p.engine.state().file_metadata_nodes[result_node].version_id,
        middle
    );
    for (node, parents) in original_parents {
        assert_eq!(
            p.engine.file_node_parents(&base.file_id, &node).unwrap(),
            parents
        );
    }
    assert_eq!(p.engine.state().file_node_parents, before.file_node_parents);
    for (id, node) in &before.file_metadata_nodes {
        assert_eq!(&p.engine.state().file_metadata_nodes[id], node);
    }
    assert_eq!(
        receipt.result["directory_branch_ids"],
        json_codec::json!([])
    );
    assert_eq!(p.engine.state().versions.len(), before.versions.len());
    assert_eq!(p.engine.state().branches.len(), before.branches.len() + 2);
    for (id, branch) in &before.branches {
        assert_eq!(&p.engine.state().branches[id], branch);
    }
    assert_eq!(
        p.engine.state().directory_branches,
        before.directory_branches
    );
    assert_eq!(p.engine.state().file_contexts, before.file_contexts);
    assert_eq!(p.current_entry("report.txt").version_id, latest);
    let event = &p.engine.state().file_branch_merges[receipt.result["merge_id"].as_str().unwrap()];
    assert_eq!(
        event.input_node_ids,
        [base.version_id.clone(), middle.clone()]
    );
    assert!(event.input_branch_ids.iter().all(|id| id != &source));
    let helper = &p.engine.state().branches[&event.input_branch_ids[1]];
    assert_eq!(helper.name, format!("history-merge-{operation}-2"));
    assert_eq!(helper.state, storage_contract::BranchState::Merged);
    let after = storage_engine::canonical_json(p.engine.state()).unwrap();
    assert_eq!(
        merge(
            &mut p,
            &base.file_id,
            &inputs,
            &middle,
            "Historical result",
            operation
        )
        .unwrap()
        .result,
        receipt.result
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        after
    );
    assert_eq!(
        merge(
            &mut p,
            &base.file_id,
            &inputs,
            &middle,
            "different",
            operation
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyKeyReused
    );
    // tempfile's default directory mode depends on umask; exports require a
    // private parent. Create this test-owned parent without relaxing admission.
    let exports = p._temporary.path().join("exports");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&exports)
        .unwrap();
    let package = exports.join("history-package");
    p.engine
        .export_history_package(&p.repo.history_tree_id, &package)
        .unwrap();
    storage_engine::offline_check_history_package(&package).unwrap();
    select(&mut p, &base.file_id, &target);
    p.save("report.txt", b"new result edit");
    let result_latest = p.engine.file_branch_head(&target).unwrap();
    p.reopen(|engine| {
        assert_eq!(engine.file_branch_head(&source).unwrap(), latest);
        assert_eq!(engine.file_branch_head(&target).unwrap(), result_latest);
    });
}

#[test]
fn historical_merge_preconditions_and_late_name_collision_leave_no_branches() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let source = fork(&mut p, &base.file_id, &base.version_id, "source");
    select(&mut p, &base.file_id, &source);
    p.save("report.txt", b"latest");
    let latest = p.engine.file_branch_head(&source).unwrap();
    let inputs = [&base.version_id, &latest].map(|node| FileHistoryMergeInput {
        branch_id: source.clone(),
        source_node_id: node.clone(),
        expected_node_id: latest.clone(),
    });
    let operation = OperationId::generate().unwrap();
    fork(
        &mut p,
        &base.file_id,
        &base.version_id,
        &format!("history-merge-{operation}-2"),
    );
    let before = storage_engine::canonical_json(p.engine.state()).unwrap();
    assert_eq!(
        merge(
            &mut p,
            &base.file_id,
            &inputs,
            &base.version_id,
            "new result",
            operation
        )
        .unwrap_err()
        .code(),
        ErrorCode::NameCollision
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before
    );
    let mut stale = inputs.clone();
    stale[0].expected_node_id = base.version_id.clone();
    let mut duplicate = inputs.clone();
    duplicate[1].source_node_id = base.version_id.clone();
    let mut missing = inputs.clone();
    missing[1].source_node_id = "ver_missing".into();
    for (invalid, code) in [
        (stale, ErrorCode::FileBranchConflict),
        (duplicate, ErrorCode::InvalidRequest),
        (missing, ErrorCode::FileNodeNotFound),
    ] {
        assert_eq!(
            merge(
                &mut p,
                &base.file_id,
                &invalid,
                &base.version_id,
                "new result",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
            code
        );
        assert_eq!(
            storage_engine::canonical_json(p.engine.state()).unwrap(),
            before
        );
    }
    let unrelated = fork(&mut p, &base.file_id, &base.version_id, "unrelated");
    let mut invalid = inputs.clone();
    invalid[1].branch_id = unrelated;
    invalid[1].expected_node_id = base.version_id.clone();
    let before = storage_engine::canonical_json(p.engine.state()).unwrap();
    assert_eq!(
        merge(
            &mut p,
            &base.file_id,
            &invalid,
            &base.version_id,
            "new result",
            OperationId::generate().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before
    );
}

#[test]
fn historical_merge_reads_frozen_aliases_and_can_adopt_an_outside_result_node() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let left = fork(&mut p, &base.file_id, &base.version_id, "left");
    let right = fork(&mut p, &base.file_id, &base.version_id, "right");
    select(&mut p, &base.file_id, &right);
    p.save("report.txt", b"right");
    let right_head = p.engine.file_branch_head(&right).unwrap();
    p.engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &[
                storage_contract::FileMergeInput {
                    branch_id: left.clone(),
                    expected_node_id: base.version_id.clone(),
                },
                storage_contract::FileMergeInput {
                    branch_id: right.clone(),
                    expected_node_id: right_head.clone(),
                },
            ],
            chosen_node_id: &base.version_id,
            surviving_branch_id: &left,
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    p.save("report.txt", b"outside chosen body");
    let chosen = p.engine.file_branch_head(&left).unwrap();
    let inputs = [&base.version_id, &right_head].map(|node| FileHistoryMergeInput {
        branch_id: right.clone(),
        source_node_id: node.clone(),
        expected_node_id: right_head.clone(),
    });
    let before = p.engine.state().clone();
    let result = merge(
        &mut p,
        &base.file_id,
        &inputs,
        &chosen,
        "From alias",
        OperationId::generate().unwrap(),
    )
    .unwrap();
    let node = result.result["node_id"].as_str().unwrap();
    assert!(node.starts_with("fmeta_"));
    assert_eq!(result.result["reused_node"], false);
    assert_eq!(
        p.engine.state().file_metadata_nodes[node].version_id,
        chosen
    );
    assert_eq!(p.engine.state().versions.len(), before.versions.len());
    for (id, branch) in &before.branches {
        assert_eq!(&p.engine.state().branches[id], branch);
    }
    assert_eq!(p.engine.state().file_branch_aliases[&right], left);
}

#[test]
fn historical_merge_preserves_archived_source_state() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    p.save("report.txt", b"later");
    let latest = p.current_entry("report.txt");
    let archived = fork(&mut p, &base.file_id, &latest.version_id, "Archived source");
    p.engine
        .archive_branch(&archived, "admin", OperationId::generate().unwrap())
        .unwrap();
    let inputs = [&base.version_id, &latest.version_id].map(|node| FileHistoryMergeInput {
        branch_id: archived.clone(),
        source_node_id: node.clone(),
        expected_node_id: latest.version_id.clone(),
    });
    let before = p.engine.state().branches[&archived].clone();
    merge(
        &mut p,
        &base.file_id,
        &inputs,
        &base.version_id,
        "From archive",
        OperationId::generate().unwrap(),
    )
    .unwrap();
    assert_eq!(p.engine.state().branches[&archived], before);
    assert_eq!(p.current_entry("report.txt").version_id, latest.version_id);
}

fn source_request(
    p: &Pool,
    file: &str,
    node: &str,
) -> storage_contract::file_history_merge::FileHistorySourceRequest {
    let view = p.engine.metadata_view();
    storage_contract::file_history_merge::FileHistorySourceRequest::parse(
        file,
        node,
        view.history_root(&p.repo.history_tree_id).unwrap(),
    )
    .unwrap()
}
#[test]
fn historical_source_lookup_proves_other_branch_ancestors_without_changing_history_pages() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let no_branch = source_request(&p, &base.file_id, &base.version_id);
    let read = p
        .engine
        .capture_file_history_source(no_branch, storage_engine::HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(
        read.source().unwrap_err().code(),
        ErrorCode::FileBranchRequired
    );
    let left = fork(&mut p, &base.file_id, &base.version_id, "Left");
    select(&mut p, &base.file_id, &left);
    p.save("report.txt", b"left ancestor");
    let ancestor = p.engine.file_branch_head(&left).unwrap();
    p.save("report.txt", b"left later");
    let head = p.engine.file_branch_head(&left).unwrap();
    let right = fork(&mut p, &base.file_id, &base.version_id, "Right");
    select(&mut p, &base.file_id, &right);
    p.save("report.txt", b"right later");
    let request = source_request(&p, &base.file_id, &ancestor);
    let before = storage_engine::canonical_json(p.engine.state()).unwrap();
    let read = p
        .engine
        .capture_file_history_source(
            request.clone(),
            storage_engine::HistoryWindowCancellation::new(),
        )
        .unwrap();
    let found = read.source().unwrap();
    found.validate(&request).unwrap();
    assert_eq!(found.branch.branch_id.as_str(), left);
    assert_eq!(found.branch.node_id.as_ref().unwrap().as_str(), head);
    p.engine.recheck_file_history_source(&read).unwrap();
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before
    );
    p.engine
        .archive_branch(&left, "admin", OperationId::generate().unwrap())
        .unwrap();
    assert_eq!(
        p.engine
            .recheck_file_history_source(&read)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    let request = source_request(&p, &base.file_id, &ancestor);
    let cancel = storage_engine::HistoryWindowCancellation::new();
    let read = p
        .engine
        .capture_file_history_source(request, cancel.clone())
        .unwrap();
    assert!(read.source().unwrap().branch.read_only);
    cancel.cancel();
    assert_eq!(
        read.source().unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    // The proven branch and exact head are accepted unchanged by the atomic merge.
    let inputs = [
        FileHistoryMergeInput {
            branch_id: left,
            source_node_id: ancestor.clone(),
            expected_node_id: head,
        },
        FileHistoryMergeInput {
            branch_id: right.clone(),
            source_node_id: p.engine.file_branch_head(&right).unwrap(),
            expected_node_id: p.engine.file_branch_head(&right).unwrap(),
        },
    ];
    let result = merge(
        &mut p,
        &base.file_id,
        &inputs,
        &ancestor,
        "Proven source merge",
        OperationId::generate().unwrap(),
    )
    .unwrap();
    let event = &p.engine.state().file_branch_merges[result.result["merge_id"].as_str().unwrap()];
    for input in &inputs {
        assert!(event.parent_node_ids.contains(&input.source_node_id));
    }
    assert!(event.parent_node_ids.contains(&ancestor));
    assert!(!event.reused_node);
}
