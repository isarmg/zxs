//! Real Born + no-past-node Unborn, independent preparation, emergency fence,
//! native receipt/cold reopen and stable ancestor/time-window observations.
use super::Fixture;
use crate::Engine;
use storage_contract::automatic_observation::{Position, Request, Scope};
use storage_contract::file_draft::{BirthRequest, CommitMode, DraftCommand, DraftLocation};
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::{OperationId, Principal, Timestamp};

fn mode(engine: &mut Engine, target: &DraftLocation, principal: &Principal, mode: CommitMode) {
    let record = &engine.state().file_containers[&target.file].working;
    let unborn = record.unborn().is_some();
    let command = DraftCommand::SetMode { target: target.clone(), expected: record.revision(), mode: mode.into() };
    let op = OperationId::generate().unwrap();
    if unborn { engine.change_unborn_working_mode(&command, principal, op).unwrap(); }
    else { engine.change_file_working(&command, principal, op).unwrap(); }
}

fn save(engine: &mut Engine, target: &DraftLocation, principal: &Principal, bytes: &[u8])
    -> crate::OperationResult {
    let mut handle = engine.open_native_working(target, principal, true).unwrap();
    assert_eq!(engine.write_native_working(&mut handle, principal, 0, bytes).unwrap(), bytes.len());
    engine.flush_native_working(&mut handle, principal).unwrap().unwrap()
}

fn scope(target: &DraftLocation, directory: NamespaceDirectoryId, start: u64, end: u64) -> Request {
    Request { scope: Scope { repository: target.repository, directory,
        start: Timestamp::from_unix_seconds(start), end: Timestamp::from_unix_seconds(end) }, position: Position::First }
}

#[test]
fn native_save_preparation_does_not_hold_manual_switch_and_a_stale_worker_has_no_node() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let actor = "admin".parse().unwrap();
    mode(&mut engine, target, &actor, CommitMode::Automatic);
    let saved = save(&mut engine, target, &actor, b"first replacement");
    let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    assert!(engine.prepare_automatic_file(target.file).is_err());
    let count = engine.state().versions.len();
    mode(&mut engine, target, &actor, CommitMode::Manual);
    let current = engine.file_working_state(target).unwrap();
    assert!(engine.finish_automatic_file(prepared).is_err());
    assert_eq!(engine.state().versions.len(), count);
    assert_eq!(engine.file_working_state(target).unwrap(), current);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert!(current.incremental().is_some());
    let query = scope(target, NamespaceDirectoryId::Repository(target.repository), 0, u64::MAX);
    assert!(engine.automatic_node_page(&query).unwrap().0.rows.is_empty());
}

#[test]
fn actual_automatic_nodes_bind_original_save_cause_stable_parent_and_half_open_time_after_cold_open() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let actor: Principal = "admin".parse().unwrap();
    let repo = fixture.targets[0].repository;
    let branch = engine.state().repositories[&repo.to_string()].main_branch_id.clone();
    let parent = engine.list_tree(&branch).unwrap().into_iter()
        .find(|item| item.path.as_str() == "Folder/Child").unwrap();
    let main = &fixture.targets[0];
    let unborn = engine.create_unborn_file(&BirthRequest {
        repository: repo, path: name_policy::PortablePath::parse("Folder/Child/new.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&branch].head_commit_id.parse().unwrap(),
    }, &actor, OperationId::generate().unwrap()).unwrap();
    let nested = crate::persist::file_draft::birth::decode(&unborn).unwrap().target().clone();
    let version_count = engine.state().versions.len();
    let mut receipts = Vec::new();
    for target in [main, &nested] {
        mode(&mut engine, target, &actor, CommitMode::Automatic);
        let saved = save(&mut engine, target, &actor, b"actual automatic publication");
        let version_before = engine.state().versions.len();
        let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
        assert_eq!(engine.state().versions.len(), version_before);
        let node = engine.finish_automatic_file(prepared).unwrap();
        let working = engine.file_working_state(target).unwrap();
        let controller = working.automatic_controller().unwrap();
        let cause = controller.last_node().unwrap();
        assert_eq!(cause.save.operation, saved.operation_id);
        assert_eq!(cause.operation, node.operation_id);
        assert_eq!(cause.sequence.get(), node.sequence);
        assert!(controller.pending_save().is_none() && working.draft().is_none());
        assert!(engine.prepare_automatic_file(target.file).is_err());
        receipts.push(node);
    }
    assert_eq!(engine.state().versions.len(), version_count + 2);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    let window = scope(main, NamespaceDirectoryId::Repository(repo), 0, u64::MAX);
    let (all, more) = engine.automatic_node_page(&window).unwrap();
    assert_eq!(all.rows.len(), 2); assert!(more.is_none() && all.next.is_none());
    let sub = scope(&nested, NamespaceDirectoryId::Entry(parent.entry_id.parse().unwrap()), 0, u64::MAX);
    let page = engine.automatic_node_page(&sub).unwrap().0;
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].key.file, nested.file);
    assert_eq!(page.rows[0].operation, receipts[1].operation_id);
    let t = page.rows[0].published_at.unix_seconds();
    assert!(engine.automatic_node_page(&scope(&nested, sub.scope.directory, 0, t)).unwrap().0.rows.is_empty());
    assert_eq!(engine.automatic_node_page(&scope(&nested, sub.scope.directory, t, t+1)).unwrap().0.rows, page.rows);
    for receipt in receipts { assert_eq!(engine.operation(receipt.operation_id).unwrap(), receipt); }
}

#[test]
fn emergency_before_copy_keeps_all_existing_material_and_the_saved_bytes_after_reopen() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let principal = "admin".parse().unwrap();
    mode(&mut engine, target, &principal, CommitMode::Automatic);
    let saved = save(&mut engine, target, &principal, b"newest durable replacement");
    let expected = super::read_draft(&engine, target);
    let job = engine.prepare_automatic_file(target.file).unwrap();
    mode(&mut engine, target, &principal, CommitMode::Manual);
    let before = fixture.media();
    let version_count = engine.state().versions.len();
    assert!(job.execute().is_err());
    assert_eq!(fixture.media(), before);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().versions.len(), version_count);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert_eq!(super::read_draft(&engine, target), expected);
    let query = scope(target, NamespaceDirectoryId::Repository(target.repository), 0, u64::MAX);
    assert!(engine.automatic_node_page(&query).unwrap().0.rows.is_empty());
}

#[test]
fn emergency_after_real_unborn_full_keeps_zero_history_and_the_exact_saved_body() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let branch = &engine.state().repositories[&repository.to_string()].main_branch_id;
    let head = engine.state().branches[branch].head_commit_id.parse().unwrap();
    let birth = engine.create_unborn_file(&BirthRequest {
        repository,
        path: name_policy::PortablePath::parse("first-only.txt").unwrap(),
        expected_namespace_head: head,
    }, &principal, OperationId::generate().unwrap()).unwrap();
    let target = crate::persist::file_draft::birth::decode(&birth).unwrap().target().clone();
    mode(&mut engine, &target, &principal, CommitMode::Automatic);
    let saved = save(&mut engine, &target, &principal, &vec![67; 150_000]);
    let prepared = engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
    let before = engine.state().versions.len();
    mode(&mut engine, &target, &principal, CommitMode::Manual);
    let current = engine.unborn_working_state(&target).unwrap();
    assert!(current.automatic_controller().is_none());
    let media = fixture.media();
    assert!(engine.finish_automatic_file(prepared).is_err());
    assert_eq!(fixture.media(), media);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().versions.len(), before);
    assert_eq!(engine.unborn_working_state(&target).unwrap(), current);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    let handle = engine.open_native_working(&target, &principal, false).unwrap();
    let mut bytes = vec![0; 150_000];
    let count = engine.read_native_working(&handle, &mut bytes, 0).unwrap();
    assert_eq!(count, bytes.len());
    assert!(bytes.iter().all(|byte| *byte == 67));
}
