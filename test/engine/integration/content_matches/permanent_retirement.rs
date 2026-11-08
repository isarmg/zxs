//! Core execution regressions. Source only in the no-CI development slice.
use super::*;
use storage_engine::ContentState;

fn retire(pool: &mut Pool, entry: &Entry, ids: &[&str]) -> storage_engine::PermanentNodeRetirementOutcome {
    let read = super::permanent_removal::read(pool, &entry.file_id);
    pool.engine.retire_file_nodes(&read, &super::permanent_removal::selection(ids), "admin", OperationId::generate().unwrap()).unwrap()
}

#[test]
fn permanent_retirement_executes_body_cleanup_and_only_final_shared_cut_separates_files() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x")]);
    let ax = pool.entry("a");
    pool.save(&[("a", b"y"), ("b", b"y")]);
    let ay = pool.entry("a");
    pool.save(&[("a", b"private-a"), ("b", b"private-b")]);
    let first = retire(&mut pool, &ax, &[&ax.version_id]);
    assert_eq!(first.publication().kind, "file.nodes.retire");
    assert!(first.cleanup().unwrap().all_current_routes_absent());
    assert_eq!(super::permanent_removal::read(&pool, &ax.file_id).graph().unwrap().files.len(), 2);
    let second = retire(&mut pool, &ay, &[&ay.version_id]);
    assert!(second.cleanup().unwrap().all_current_routes_absent());
    let graph = super::permanent_removal::read(&pool, &ax.file_id).graph().unwrap();
    assert_eq!(graph.files.len(), 1);
    assert!(graph.versions.iter().all(|v| v.version_id.as_str() != ax.version_id && v.version_id.as_str() != ay.version_id));
    assert_eq!(pool.engine.state().content_states[&ax.version_id], ContentState::Retired);
    assert!(pool.engine.state().versions.contains_key(&ax.version_id), "old descriptors are retained authentication evidence, not physically erased metadata");
    assert!(pool.engine.file_node_version(&ax.file_id, &ax.version_id).is_err());
    assert!(pool.engine.file_history(&ax.file_id).unwrap().iter().all(|v| v.version_id != ax.version_id));
    assert!(pool.engine.open_content(&ax.version_id).is_err());
}

#[test]
fn permanent_retirement_projects_internal_ancestry_without_splitting_a_file() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"first")]);
    let first = pool.entry("a");
    pool.save(&[("a", b"middle")]);
    let middle = pool.entry("a");
    pool.save(&[("a", b"last")]);
    let last = pool.entry("a");
    retire(&mut pool, &middle, &[&middle.version_id]);
    let graph = super::permanent_removal::read(&pool, &last.file_id).graph().unwrap();
    assert_eq!(graph.files.len(), 1);
    let last_node = graph.nodes.iter().find(|n| n.node_id.as_str() == last.version_id).unwrap();
    assert_eq!(last_node.parent_node_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec![first.version_id.as_str()]);
    assert_eq!(pool.engine.file_node_parents(&last.file_id, &last.version_id).unwrap(), vec![first.version_id]);
}

#[test]
fn permanent_retirement_cold_resume_preserves_exact_receipt_and_refuses_old_node_source() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"old")]);
    let old = pool.entry("a");
    pool.save(&[("a", b"new")]);
    let captured = super::permanent_removal::read(&pool, &old.file_id);
    let selected = super::permanent_removal::selection(&[&old.version_id]);
    let operation = OperationId::generate().unwrap();
    let receipt = pool.engine.retire_file_nodes(&captured, &selected, "admin", operation).unwrap().publication().clone();
    let sequence = pool.engine.state().sequence;
    // Same original request retries before checking its now-stale read root.
    assert_eq!(pool.engine.retire_file_nodes(&captured, &selected, "admin", operation).unwrap().publication(), &receipt);
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    let resumed = pool.engine.resume_file_node_retirement(operation, "admin").unwrap();
    assert_eq!(resumed.publication(), &receipt);
    assert!(resumed.cleanup().unwrap().all_current_routes_absent());
    assert_eq!(pool.engine.state().sequence, sequence);
    assert!(pool.engine.create_file_branch(&old.file_id, &old.version_id, &BranchName::parse("revive").unwrap(), "admin", OperationId::generate().unwrap()).is_err());
    assert!(pool.engine.resume_file_node_retirement(operation, "another-actor").is_err());
    assert_eq!(pool.engine.state().content_states[&old.version_id], ContentState::Retired);
}

#[test]
fn permanent_retirement_rejects_current_head_and_open_workspace_without_publishing() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"old")]);
    let old = pool.entry("a");
    let read = super::permanent_removal::read(&pool, &old.file_id);
    let before = pool.engine.state().sequence;
    assert!(pool.engine.retire_file_nodes(&read, &super::permanent_removal::selection(&[&old.version_id]), "admin", OperationId::generate().unwrap()).is_err());
    assert_eq!(pool.engine.state().sequence, before);
    pool.save(&[("a", b"new")]);
    let _workspace = pool.engine.create_workspace(&pool.branch, "admin").unwrap();
    let read = super::permanent_removal::read(&pool, &old.file_id);
    let before = pool.engine.state().sequence;
    assert!(pool.engine.retire_file_nodes(&read, &super::permanent_removal::selection(&[&old.version_id]), "admin", OperationId::generate().unwrap()).is_err());
    assert_eq!(pool.engine.state().sequence, before);
    assert_ne!(pool.engine.state().content_states[&old.version_id], ContentState::Retired);
}

#[test]
fn permanent_retirement_refuses_lossy_portable_export_and_stale_new_request() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"old")]);
    let old = pool.entry("a");
    pool.save(&[("a", b"middle")]);
    let stale = super::permanent_removal::read(&pool, &old.file_id);
    pool.save(&[("a", b"latest")]);
    assert!(pool.engine.retire_file_nodes(&stale, &super::permanent_removal::selection(&[&old.version_id]), "admin", OperationId::generate().unwrap()).is_err());
    retire(&mut pool, &old, &[&old.version_id]);
    let destination = pool._temp.path().join("no-lossy-export");
    assert_eq!(pool.engine.export_history_package(&pool.history, &destination).unwrap_err().code(), ErrorCode::HistoryPackageInvalid);
    assert!(!destination.exists());
}

#[test]
fn repeated_equal_versions_keep_the_last_parallel_witness_live() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared")]);
    let first = pool.entry("a");
    pool.save(&[("a", b"between")]);
    pool.save(&[("a", b"shared")]);
    let repeated = pool.entry("a");
    pool.save(&[("a", b"private")]);
    let first_result = retire(&mut pool, &first, &[&first.version_id]);
    assert_eq!(first_result.separation().unwrap().file_components().len(), 1);
    assert_eq!(super::permanent_removal::read(&pool, &first.file_id).graph().unwrap().files.len(), 2);
    let last_result = retire(&mut pool, &repeated, &[&repeated.version_id]);
    assert_eq!(last_result.separation().unwrap().original_files().len(), 2);
    assert_eq!(last_result.separation().unwrap().file_components().len(), 2);
    assert_eq!(super::permanent_removal::read(&pool, &first.file_id).graph().unwrap().files.len(), 1);
}

#[test]
fn transitive_paths_and_original_partitions_survive_later_retirement_reconnect_and_cold_open() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x"), ("c", b"private-c")]);
    let ax = pool.entry("a");
    pool.save(&[("a", b"z"), ("c", b"z")]);
    let az = pool.entry("a");
    pool.save(&[("b", b"y"), ("c", b"y")]);
    pool.save(&[("a", b"private-a"), ("b", b"private-b"), ("c", b"private-c-final")]);
    let first = retire(&mut pool, &ax, &[&ax.version_id]);
    let first_id = first.publication().operation_id;
    assert_eq!(first.separation().unwrap().original_files().len(), 3);
    assert_eq!(first.separation().unwrap().file_components().len(), 1, "a-c-b still connects all files");
    let second = retire(&mut pool, &az, &[&az.version_id]);
    let second_id = second.publication().operation_id;
    let original_root = second.separation().unwrap().source_history_root();
    let original_partition = second.separation().unwrap().file_components().to_vec();
    assert_eq!(original_partition.len(), 2);
    assert_eq!(super::permanent_removal::read(&pool, &ax.file_id).graph().unwrap().files.len(), 1);
    pool.save(&[("a", b"y")]);
    assert_eq!(super::permanent_removal::read(&pool, &ax.file_id).graph().unwrap().files.len(), 3);
    assert_eq!(pool.engine.state().content_states[&ax.version_id], ContentState::Retired);
    assert_eq!(pool.engine.state().content_states[&az.version_id], ContentState::Retired);
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    let old = pool.engine.resume_file_node_retirement(first_id, "admin").unwrap();
    assert_eq!(old.separation().unwrap().file_components().len(), 1);
    let resumed = pool.engine.resume_file_node_retirement(second_id, "admin").unwrap();
    assert_eq!(resumed.separation().unwrap().source_history_root(), original_root);
    assert_eq!(resumed.separation().unwrap().file_components(), original_partition);
    assert_eq!(super::permanent_removal::read(&pool, &ax.file_id).graph().unwrap().files.len(), 3);
}
