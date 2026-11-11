use crate::{
    Config, Engine, MembershipExitAction, OperationId, ReadView, RepositoryName, TerminalManagement,
};
use std::sync::Arc;
use storage_contract::{ContentState, Entry, VersionId};

fn counters(read: &ReadView<'_>) -> (u64, u64) {
    (read.sequence(), read.topology_generation())
}

fn save_version(engine: &mut Engine, branch: &str, body: &[u8]) -> Entry {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "read.txt", body)
        .unwrap();
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "version read fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(branch)
        .unwrap()
        .get("read.txt")
        .unwrap()
        .clone()
}

#[test]
fn version_metadata_borrows_exact_record_and_preserves_sparse_disposition() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("version metadata").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let entry = save_version(&mut engine, &repo.main_branch_id, b"published");
    let id: VersionId = entry.version_id.parse().unwrap();
    assert!(engine.version_metadata(VersionId::generate().unwrap()).is_none());
    // These fixtures exercise the query's exact sparse-data semantics, not admission.
    for disposition in [
        None,
        Some(ContentState::Available),
        Some(ContentState::Purged),
        Some(ContentState::Migrated),
        Some(ContentState::Retired),
    ] {
        let state = Arc::make_mut(&mut engine.state);
        match disposition {
            Some(value) => {
                state.content_states.insert(entry.version_id.clone(), value);
            }
            None => {
                state.content_states.remove(&entry.version_id);
            }
        }
        let (record, stored) = engine.version_metadata(id).unwrap();
        assert!(std::ptr::eq(record, &engine.state().versions[&entry.version_id]));
        assert_eq!(stored, disposition);
        assert_eq!(record.logical_size, 9);
    }
}

#[test]
fn version_metadata_keeps_both_publications_after_cold_open() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("retained metadata").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let first = save_version(&mut engine, &repo.main_branch_id, b"first");
    let id: VersionId = first.version_id.parse().unwrap();
    let (record, state) = engine.version_metadata(id).unwrap();
    let frozen = record.clone();
    let second = save_version(&mut engine, &repo.main_branch_id, b"second");
    let second_id: VersionId = second.version_id.parse().unwrap();
    assert_eq!(engine.version_metadata(id), Some((&frozen, state)));
    let second_record = engine.version_metadata(second_id).unwrap().0.clone();
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.version_metadata(id), Some((&frozen, state)));
    assert_eq!(engine.version_metadata(second_id).unwrap().0, &second_record);
}

#[test]
fn published_counters_follow_repository_creation_replay_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    assert_eq!(counters(&engine.read()), (0, 1));

    let name = RepositoryName::parse("counter publication").unwrap();
    let operation = OperationId::generate().unwrap();
    let repository = engine
        .create_repository(&name, "P01", 1, "admin", operation)
        .unwrap();
    let read = engine.read();
    assert_eq!(counters(&read), (1, 1));
    assert_eq!(counters(&engine.read()), counters(&read));

    let replay = engine
        .create_repository(&name, "P01", 1, "admin", operation)
        .unwrap();
    assert_eq!(replay, repository);
    assert_eq!(counters(&engine.read()), (1, 1));
    drop(engine);

    let reopened = Engine::open(&system).unwrap();
    assert_eq!(counters(&reopened.read()), (1, 1));
}

#[test]
fn terminal_counters_follow_the_terminal_publication_and_cold_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("primary");
    let config = Config::development([disk.clone()]).unwrap();
    let disk_id = config.disks[0].disk_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("retained").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = counters(&engine.read());
    engine
        .begin_disk_exit(
            &disk_id,
            MembershipExitAction::Deregister,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(counters(&engine.read()), before);
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let terminal = exit.complete(&mut engine).unwrap();
    let read = terminal.read();
    // The membership fence consumes a log slot before terminal publication.
    let expected = (before.0 + 2, before.1 + 1);
    assert_eq!(counters(&read), expected);
    assert_eq!(read.sequence(), terminal.result().sequence);
    assert_eq!(counters(&terminal.read()), counters(&read));
    drop(terminal);
    drop(exit);
    drop(engine);

    std::fs::rename(&disk, temp.path().join("retired-primary")).unwrap();
    let reopened = TerminalManagement::open(&system).unwrap();
    assert_eq!(counters(&reopened.read()), expected);
}
