//! Same real worktree writer: a second saved instance outlives both HEADs.
#[path = "draft_gateway/fixture.rs"]
mod fixture;
use crate::persist::snapshot_worktree::{self as codec, HistoricalFile, Record};
use crate::{Engine, ReadFile};
use fixture::Fixture;
use name_policy::LookupPath;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::outcome::WorktreeCommitOutcome;
use storage_contract::snapshot_worktree::{
    CommitWorktree, EditWorktree, SnapshotHandle, WorktreeHandle,
};
use storage_contract::{ContentHash, OperationId, Principal};

fn principal() -> Principal {
    "admin".parse().unwrap()
}
fn create(fixture: &Fixture, engine: &mut Engine, source: SnapshotHandle) -> WorktreeHandle {
    let context = engine
        .snapshot_root_context(fixture.targets[0].repository)
        .unwrap();
    engine
        .allocate_directory_worktree(
            &AllocateWorktree {
                source,
                parent_directory_id: context.host.directory_id,
                expected_root_generation: context.host.expected_root_generation,
                operation_id: OperationId::generate().unwrap(),
                naming: WorktreeNaming::Automatic,
            },
            &principal(),
        )
        .unwrap()
}
fn save(engine: &mut Engine, target: WorktreeHandle, body: &[u8]) -> CommitWorktree {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(body).unwrap();
    file.flush().unwrap();
    let request = EditWorktree {
        target,
        path: name_policy::PortablePath::parse("first.txt").unwrap(),
        expected: engine.directory_worktree_status(target).unwrap().revision,
        size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
        operation_id: OperationId::generate().unwrap(),
    };
    let source: ReadFile = file.reopen().unwrap().into();
    let expected = engine
        .edit_directory_worktree(&request, &principal(), source)
        .unwrap();
    CommitWorktree {
        target,
        expected,
        message: "exact selected history".into(),
        operation_id: OperationId::generate().unwrap(),
    }
}
fn body(engine: &Engine, target: WorktreeHandle, path: &str) -> (Vec<u8>, (u64, u64)) {
    let mut opened = engine
        .open_directory_worktree_content(target, &LookupPath::parse(path).unwrap())
        .unwrap();
    let metadata = opened.file.metadata().unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    (bytes, (metadata.dev(), metadata.ino()))
}

#[test]
fn historical_commit_forks_both_heads_moves_exact_edit_and_cold_retries_without_writes() {
    let (fixture, mut engine) = Fixture::new();
    let source = fixture.source;
    let first = create(&fixture, &mut engine, source);
    let historical = create(&fixture, &mut engine, source);
    let old_file = engine
        .file_context(&fixture.targets[0].repository.to_string(), "first.txt")
        .unwrap();
    let original_version = engine.file_branch_head(&old_file.branch_id).unwrap();
    let first_commit = save(&mut engine, first, b"first instance advances original HEAD");
    let historical_commit = save(
        &mut engine,
        historical,
        b"second instance keeps its original parent",
    );
    let sealed = body(&engine, historical, "first.txt");
    let original_copy = body(&engine, historical, "second.txt");
    let advanced = engine
        .commit_directory_worktree(&first_commit, &principal())
        .unwrap();
    assert_eq!(advanced.branch_id, source.branch_id);
    let advanced_file = engine.file_branch_head(&old_file.branch_id).unwrap();
    assert_ne!(advanced_file, original_version);
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(body(&engine, historical, "first.txt"), sealed);
    let fork = engine
        .commit_directory_worktree(&historical_commit, &principal())
        .unwrap();
    assert_eq!(fork.repository_id, source.repository_id);
    assert_ne!(fork.branch_id, source.branch_id);
    assert_ne!(fork.node_id, source.node_id);
    assert_eq!(body(&engine, historical, "first.txt"), sealed);
    let copied = body(&engine, historical, "second.txt");
    assert_eq!(copied.0, original_copy.0);
    assert_ne!(copied.1, original_copy.1);
    assert_eq!(
        engine.file_branch_head(&old_file.branch_id).unwrap(),
        advanced_file
    );
    let original = engine.operation(historical_commit.operation_id).unwrap();
    let Record::ReserveHistoricalCommit(intent) = codec::decode(&original).unwrap().unwrap().record
    else {
        panic!("actual historical original");
    };
    assert_eq!(intent.source, source);
    assert_eq!(intent.observed_head, advanced);
    assert_eq!(intent.branch_name.as_str(), "worktree-main-002");
    let HistoricalFile::Fork {
        operation: file_fork,
        ..
    } = intent.file
    else {
        panic!("the actual FILE head advanced too");
    };
    let branch_receipt = engine.operation(file_fork).unwrap();
    let ready_receipt = engine.operation(intent.ready).unwrap();
    let Record::PrepareHistoricalCommit(ready) =
        codec::decode(&ready_receipt).unwrap().unwrap().record
    else {
        panic!("actual exact FILE command");
    };
    assert!(original.sequence < branch_receipt.sequence);
    assert!(branch_receipt.sequence < ready_receipt.sequence);
    let published_file = engine.operation(ready.plan.file_operation).unwrap();
    assert_eq!(published_file.result["workspace_file_origin"]["kind"], "no_such_creation");
    assert!(ready_receipt.sequence < published_file.sequence);
    let version = &engine.state().versions[&ready.plan.edit.material.version_id.to_string()];
    assert_eq!(version.source_version_ids, [original_version]);
    assert_eq!(
        published_file.branch_id,
        Some(ready.file_branch.to_string())
    );
    assert_ne!(ready.file_branch.to_string(), old_file.branch_id);
    assert_eq!(
        engine
            .snapshot_receipts()
            .unwrap()
            .commit_head(source)
            .unwrap(),
        advanced
    );
    assert_eq!(
        engine
            .snapshot_receipts()
            .unwrap()
            .commit_head(fork)
            .unwrap(),
        fork
    );
    let again = create(&fixture, &mut engine, source);
    assert_eq!(body(&engine, again, "first.txt").0, fixture::FIRST_ORIGINAL);
    let successor = create(&fixture, &mut engine, fork);
    assert_eq!(body(&engine, successor, "first.txt").0, sealed.0);
    let successor_commit = save(&mut engine, successor, b"continue the newly published fork");
    let successor_node = engine
        .commit_directory_worktree(&successor_commit, &principal())
        .unwrap();
    assert_eq!(successor_node.branch_id, fork.branch_id);
    assert_ne!(successor_node.node_id, fork.node_id);
    let outcome = engine
        .directory_worktree_commit_receipt(
            fixture.targets[0].repository,
            historical_commit.operation_id,
            &principal(),
        )
        .unwrap();
    assert_eq!(
        outcome.result,
        WorktreeCommitOutcome::Complete { snapshot: fork }
    );
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .commit_directory_worktree(&historical_commit, &principal())
            .unwrap(),
        fork
    );
    assert_eq!(
        engine
            .commit_directory_worktree(&successor_commit, &principal())
            .unwrap(),
        successor_node
    );
    assert_eq!(
        engine
            .directory_worktree_commit_receipt(
                fixture.targets[0].repository,
                historical_commit.operation_id,
                &principal(),
            )
            .unwrap(),
        outcome
    );
    assert_eq!(
        engine.operation(historical_commit.operation_id).unwrap(),
        original
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(fixture.media(), media);
}

#[cfg(feature = "fault-injection")]
#[test]
fn historical_original_before_file_fork_stays_pending_after_cold_retry() {
    let (fixture, mut engine) = Fixture::new();
    let first = create(&fixture, &mut engine, fixture.source);
    let historical = create(&fixture, &mut engine, fixture.source);
    let first_commit = save(&mut engine, first, b"advance first");
    let second_commit = save(&mut engine, historical, b"retain second");
    engine
        .commit_directory_worktree(&first_commit, &principal())
        .unwrap();
    engine.inject_fault_once("after_worktree_historical_reservation");
    assert!(
        engine
            .commit_directory_worktree(&second_commit, &principal())
            .is_err()
    );
    let original = engine.operation(second_commit.operation_id).unwrap();
    let Record::ReserveHistoricalCommit(plan) = codec::decode(&original).unwrap().unwrap().record
    else {
        panic!("published original");
    };
    let HistoricalFile::Fork { operation, .. } = plan.file else {
        panic!("FILE fork intent");
    };
    assert!(engine.operation(operation).is_err());
    assert!(engine.operation(plan.ready).is_err());
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .directory_worktree_commit_receipt(
                fixture.targets[0].repository,
                second_commit.operation_id,
                &principal(),
            )
            .unwrap()
            .result,
        WorktreeCommitOutcome::Pending {}
    );
    assert!(matches!(crate::snapshot_worktree::commit_worktree(
        &mut engine, &second_commit, &principal(),
    ), Err(crate::snapshot_worktree::RegistrationError::Pending(id))
        if id == second_commit.operation_id));
    assert_eq!(
        engine.operation(second_commit.operation_id).unwrap(),
        original
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(fixture.media(), media);
}
