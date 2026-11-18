//! Fail closed after the real fixed members decided, but before either ledger
//! frame. In particular, matching bytes are not a substitute for a moved inode.
use super::*;
use crate::persist::layout::{snapshot, worktree};
use crate::snapshot_worktree::commit::content::Originals;
use crate::snapshot_worktree::registry::ContentRetry;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

struct OriginalNames {
    sparse: PathBuf,
    installed: PathBuf,
    ledger_commit: PathBuf,
    entity: (u64, u64),
}

fn first_promised_move(
    engine: &Engine,
    fixture: &Fixture,
    request: &CommitWorktree,
    principal: &Principal,
) -> OriginalNames {
    let receipts = engine.snapshot_receipts().unwrap();
    let Some(ContentRetry::Reserved(grant)) = receipts.retry_content(request, principal).unwrap()
    else { panic!("the native reservation must be durable"); };
    let plan = grant.plan();
    let held = Originals::retained(engine, &receipts, &grant).unwrap();
    let planned = held.project_reserved(&engine.planning_context(), principal, plan).unwrap();
    let row = &plan.rows[0];
    let payload = &planned.manifest.payloads()[&row.edit.material.version_id];
    let repository = fixture.snapshot_path();
    let ledger_commit = repository.join(snapshot::node(plan.source.node_id).as_path())
        .join(snapshot::WORKTREES.as_path())
        .join(worktree::ledger(request.target.worktree_id).as_path())
        .join(worktree::frame(plan.before.sequence.next().unwrap()).as_path());
    OriginalNames {
        sparse: held.sparse.directory().path().join(row.edit.leaf.entry().as_path()),
        installed: repository.join(snapshot::node(plan.destination_node).as_path())
            .join(snapshot::FILES.as_path()).join(snapshot::file(payload).as_path()),
        ledger_commit,
        entity: (row.edit.entity.device(), row.edit.entity.inode()),
    }
}

fn entity(path: &Path) -> (u64, u64) {
    let stat = fs::symlink_metadata(path).unwrap();
    (stat.dev(), stat.ino())
}

#[test]
fn finalized_cold_reopen_neither_consumes_two_names_nor_replaces_an_equal_byte_impostor() {
    let (fixture, mut engine, principal, request, other) = prepare();
    engine.inject_fault_once("after_worktree_content_reserve");
    assert!(engine.commit_directory_worktree(&request, &principal).is_err());
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let original = first_promised_move(&engine, &fixture, &request, &principal);
    engine.inject_fault_once("after_worktree_content_node");
    assert!(engine.commit_directory_worktree(&request, &principal).is_err());
    drop(engine);
    assert!(!original.sparse.exists());
    assert!(!original.ledger_commit.exists());
    assert_eq!(entity(&original.installed), original.entity);

    // The same exact inode at two names is not an unambiguous MOVE.
    fs::hard_link(&original.installed, &original.sparse).unwrap();
    assert!(Engine::open(&fixture.system).is_err());
    assert_eq!(entity(&original.sparse), original.entity);
    assert_eq!(entity(&original.installed), original.entity);
    assert!(!original.ledger_commit.exists());
    fs::remove_file(&original.sparse).unwrap();

    // Preserve the real inode outside both names while installing a byte-for-
    // byte copy at the destination. Recovery cannot "repair" either one.
    let preserved = fixture.system.parent().unwrap().join("preserved-original");
    fs::rename(&original.installed, &preserved).unwrap();
    fs::copy(&preserved, &original.installed).unwrap();
    let impostor = entity(&original.installed);
    assert_ne!(impostor, original.entity);
    assert_eq!(fs::read(&preserved).unwrap(), fs::read(&original.installed).unwrap());
    assert!(Engine::open(&fixture.system).is_err());
    assert_eq!(entity(&preserved), original.entity);
    assert_eq!(entity(&original.installed), impostor);
    assert!(!original.sparse.exists());
    assert!(!original.ledger_commit.exists());

    // Only the fixture operator restores the exact originally decided object.
    fs::remove_file(&original.installed).unwrap();
    fs::rename(&preserved, &original.installed).unwrap();
    let mut engine = Engine::open(&fixture.system).unwrap();
    let handle = engine.commit_directory_worktree(&request, &principal).unwrap();
    assert_closed(&engine, &request, handle, fixture.source, other);
}
