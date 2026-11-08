use super::*;
use std::collections::BTreeSet;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId, HistoryVersionId};
use storage_contract::shared_history::SharedHistoryRequest;
use storage_engine::{PermanentNodeRemovalAnalysis, PermanentNodeRemovalSelection};

pub(super) fn selection(ids: &[&str]) -> PermanentNodeRemovalSelection {
    PermanentNodeRemovalSelection::new(
        ids.iter()
            .map(|id| HistoryVersionId::parse(id).unwrap())
            .collect(),
    )
    .unwrap()
}
pub(super) fn read(pool: &Pool, file: &str) -> storage_engine::SharedHistoryRead {
    pool.engine
        .capture_shared_history(
            SharedHistoryRequest::parse(
                file,
                pool.engine
                    .metadata_view()
                    .history_root(&pool.history)
                    .unwrap(),
            )
            .unwrap(),
            HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn analyze(pool: &Pool, file: &str, ids: &[&str]) -> PermanentNodeRemovalAnalysis {
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let captured = read(pool, file);
    let result = captured
        .permanent_removal_analysis(&selection(ids))
        .unwrap();
    pool.engine.recheck_shared_history(&captured).unwrap();
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    result
}
fn members(analysis: &PermanentNodeRemovalAnalysis) -> BTreeSet<String> {
    analysis
        .file_components()
        .iter()
        .flatten()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn permanent_removal_analysis_only_separates_files_after_every_remaining_content_path_is_gone() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x")]);
    let ax = pool.entry("a");
    pool.save(&[("a", b"y"), ("b", b"y"), ("c", b"y")]);
    let ay = pool.entry("a");
    let b = pool.entry("b");
    let c = pool.entry("c");
    pool.save(&[("a", b"private-a")]);
    let one = analyze(&pool, &ax.file_id, &[&ax.version_id]);
    assert!(!one.would_separate_files());
    assert_eq!(one.file_components()[0].len(), 3);
    let both = analyze(&pool, &ax.file_id, &[&ay.version_id, &ax.version_id]);
    assert!(both.would_separate_files());
    assert_eq!(both.file_components().len(), 2);
    assert_eq!(
        members(&both),
        BTreeSet::from([ax.file_id.clone(), b.file_id.clone(), c.file_id.clone()])
    );
    assert!(
        both.file_components()
            .iter()
            .any(|files| files.len() == 1 && files[0].as_str() == ax.file_id)
    );
    assert!(both.file_components().iter().any(|files| {
        files.len() == 2
            && files
                .iter()
                .all(|id| id.as_str() == b.file_id || id.as_str() == c.file_id)
    }));
    assert!(both.retained_commit_entries() >= 2);
    assert_eq!(both.retirements().len(), 2);
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(
        analyze(&pool, &ax.file_id, &[&ax.version_id, &ay.version_id]),
        both
    );
}

#[test]
fn permanent_removal_analysis_keeps_alternative_cross_file_paths_until_the_cut_is_complete() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x")]);
    let ax = pool.entry("a");
    pool.save(&[("b", b"y"), ("c", b"y")]);
    pool.save(&[("c", b"z"), ("a", b"z")]);
    let cz = pool.entry("c");
    let one = analyze(&pool, &ax.file_id, &[&ax.version_id]);
    assert_eq!(one.file_components().len(), 1);
    assert_eq!(one.file_components()[0].len(), 3);
    let cut = analyze(&pool, &ax.file_id, &[&ax.version_id, &cz.version_id]);
    assert_eq!(cut.file_components().len(), 2);
    assert!(
        cut.file_components()
            .iter()
            .any(|files| files == &[HistoryFileId::parse(&ax.file_id).unwrap()])
    );
    assert_eq!(read(&pool, &ax.file_id).graph().unwrap().files.len(), 3);
}

#[test]
fn permanent_removal_analysis_keeps_one_file_identity_and_projects_through_internal_deleted_nodes()
{
    let mut pool = Pool::new();
    pool.save(&[("a", b"first")]);
    let first = pool.entry("a");
    pool.save(&[("a", b"middle")]);
    let middle = pool.entry("a");
    pool.save(&[("a", b"last")]);
    let last = pool.entry("a");
    let graph = read(&pool, &first.file_id).graph().unwrap();
    assert!(
        graph
            .nodes
            .iter()
            .find(|n| n.node_id.as_str() == last.version_id)
            .unwrap()
            .parent_node_ids
            .iter()
            .any(|p| p.as_str() == middle.version_id)
    );
    let result = analyze(&pool, &first.file_id, &[&middle.version_id]);
    assert!(!result.would_separate_files());
    assert_eq!(
        result.file_components(),
        &[vec![HistoryFileId::parse(&first.file_id).unwrap()]]
    );
    let changed = result
        .parent_projection()
        .iter()
        .find(|p| p.node_id().as_str() == last.version_id)
        .unwrap();
    assert_eq!(
        changed
            .parent_node_ids()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec![first.version_id.as_str()]
    );
    assert!(
        pool.engine
            .state()
            .versions
            .contains_key(&middle.version_id)
    );
    assert_eq!(pool.engine.state().plans.len(), 0);
}

#[test]
fn permanent_removal_analysis_couples_body_metadata_without_treating_soft_delete_as_retirement() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"same"), ("b", b"same")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    let branch = pool
        .engine
        .create_file_branch(
            &a.file_id,
            &a.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = pool.engine.file_branch_head(&branch).unwrap();
    let tag = pool
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &a.file_id,
            targets: &[FileTagTarget {
                branch_id: branch.clone(),
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["kept".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let tag_node = tag.result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    pool.engine
        .soft_delete_file(storage_engine::DeleteFileRequest {
            target: storage_engine::FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&pool.repository).unwrap(),
                file_id: &HistoryFileId::parse(&a.file_id).unwrap(),
                path: &name_policy::LookupPath::parse("a").unwrap(),
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let graph = read(&pool, &b.file_id).graph().unwrap();
    assert_eq!(graph.files.len(), 2);
    let deletion = graph
        .nodes
        .iter()
        .find(|n| n.kind == storage_contract::shared_history::SharedHistoryNodeKind::Deletion)
        .unwrap();
    assert!(HistoryVersionId::parse(deletion.node_id.as_str()).is_err());
    let result = analyze(&pool, &b.file_id, &[&a.version_id]);
    assert_eq!(result.retirements().len(), 1);
    let coupled = result.retirements()[0]
        .event_node_ids()
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    assert!(coupled.contains(a.version_id.as_str()));
    assert!(coupled.contains(tag_node.as_str()));
    assert!(!coupled.contains(deletion.node_id.as_str()));
    assert_eq!(
        result.frozen_files(),
        &[HistoryFileId::parse(&a.file_id).unwrap()]
    );
    assert!(
        result
            .affected_branch_heads()
            .iter()
            .any(|id| id.as_str() == branch)
    );
    pool.save(&[("b", b"still editable")]);
    assert_eq!(read(&pool, &b.file_id).graph().unwrap().files.len(), 2);
}

#[test]
fn permanent_removal_analysis_never_converts_body_purge_into_live_node_removal() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"same"), ("b", b"same")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    pool.save(&[("a", b"next-a"), ("b", b"next-b")]);
    let plan = pool
        .engine
        .create_purge_plan(
            &pool.history,
            vec![a.version_id.clone(), b.version_id.clone()],
            "admin",
        )
        .unwrap();
    pool.engine
        .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
        .unwrap();
    assert_eq!(read(&pool, &a.file_id).graph().unwrap().files.len(), 2);
    let result = analyze(&pool, &a.file_id, &[&a.version_id]);
    assert!(result.would_separate_files());
    assert_eq!(
        result.retirements()[0].content_state(),
        Some(storage_engine::ContentState::Purged)
    );
    assert!(pool.engine.state().versions.contains_key(&a.version_id));
    assert!(pool.engine.state().versions.contains_key(&b.version_id));
    assert_eq!(read(&pool, &a.file_id).graph().unwrap().files.len(), 2);
}

#[test]
fn permanent_removal_analysis_preserves_repeated_content_and_excludes_empty_file_links() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x"), ("empty", b"")]);
    let first = pool.entry("a");
    let b = pool.entry("b");
    pool.save(&[("a", b"y")]);
    pool.save(&[("a", b"x")]);
    let last = pool.entry("a");
    assert_ne!(first.version_id, last.version_id);
    let one = analyze(&pool, &first.file_id, &[&first.version_id]);
    assert_eq!(one.file_components().len(), 1);
    assert_eq!(
        members(&one),
        BTreeSet::from([first.file_id.clone(), b.file_id.clone()])
    );
    let both = analyze(
        &pool,
        &first.file_id,
        &[&first.version_id, &last.version_id],
    );
    assert_eq!(both.file_components().len(), 2);
    assert!(both.file_components().iter().all(|files| files.len() == 1));
    assert_eq!(read(&pool, &first.file_id).graph().unwrap().files.len(), 2);
}

#[test]
fn permanent_removal_analysis_rejects_bad_selection_and_stale_or_cancelled_observations() {
    assert!(PermanentNodeRemovalSelection::new(vec![]).is_err());
    let id = HistoryVersionId::parse("ver_00000000000000000000000000000001").unwrap();
    assert!(PermanentNodeRemovalSelection::new(vec![id.clone(), id.clone()]).is_err());
    assert!(PermanentNodeRemovalSelection::new(vec![id; 513]).is_err());
    let mut pool = Pool::new();
    pool.save(&[("a", b"a"), ("unrelated", b"other")]);
    let a = pool.entry("a");
    let other = pool.entry("unrelated");
    let captured = read(&pool, &a.file_id);
    assert_eq!(
        captured
            .permanent_removal_analysis(&selection(&[&other.version_id]))
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
    let request = SharedHistoryRequest::parse(
        &a.file_id,
        pool.engine
            .metadata_view()
            .history_root(&pool.history)
            .unwrap(),
    )
    .unwrap();
    let cancellation = HistoryWindowCancellation::new();
    let cancelled = pool
        .engine
        .capture_shared_history(request, cancellation.clone())
        .unwrap();
    cancellation.cancel();
    assert_eq!(
        cancelled
            .permanent_removal_analysis(&selection(&[&a.version_id]))
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    captured
        .permanent_removal_analysis(&selection(&[&a.version_id]))
        .unwrap();
    pool.save(&[("a", b"changed")]);
    assert_eq!(
        pool.engine
            .recheck_shared_history(&captured)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
}
