//! Full real archive differential; retained computation is not target authority.
use super::*;
use super::super::decode_tests::{Fixture, error_signature, fixture_file_bytes};
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo::tests::{
    memo_usage, memo_with_archive_headroom, memo_with_limit, move_pending_archive_lookup,
    with_pending_system_byte,
};
use std::{fs, os::unix::fs::DirBuilderExt, sync::atomic::Ordering};

fn task(fixture: &Fixture) -> NativeTask {
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    NativeTask::read(&system).unwrap()
}

fn seed(
    fixture: &Fixture,
    root: &DiskRoot,
    task: &NativeTask,
    memo: &ScopeEncodingMemo,
) -> BackupNativeControlReport {
    let before = fixture_file_bytes(fixture._temp.path());
    let stop = AtomicBool::new(false);
    let expected = super::super::super::verify_completed(task, &[root], &stop).unwrap();
    let binding = |_: &AtomicBool| -> Result<Option<[u8; 32]>> {
        panic!("archive transcript must not evaluate the SQLite binding")
    };
    let mut transaction = Transaction::new(Some(memo), &binding);
    let report = super::super::super::verify_completed_with_encodings(
        task, &[root], &stop, &mut transaction,
    ).unwrap();
    assert_eq!(canonical_json(&report).unwrap(), canonical_json(&expected).unwrap());
    // This isolated reader test commits after its real complete verification.
    // The worker still commits only after its enclosing tail gates.
    transaction.commit();
    assert_eq!(fixture_file_bytes(fixture._temp.path()), before);
    super::super::read(root, task.selection.first(), &task.archive_hash, &stop).unwrap()
}

fn compare(
    root: &DiskRoot,
    task: &NativeTask,
    memo: &ScopeEncodingMemo,
    stop: &AtomicBool,
) -> Option<ErrorCode> {
    let binding = |_: &AtomicBool| Ok(None);
    let mut transaction = Transaction::new(Some(memo), &binding);
    let expected = super::super::read(root, task.selection.first(), &task.archive_hash, stop);
    let actual = read(root, task.selection.first(), task, stop, &mut transaction);
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => {
            assert_eq!(actual, expected);
            None
        }
        (Err(expected), Err(actual)) => {
            assert_eq!(error_signature(&actual), error_signature(&expected));
            Some(actual.code())
        }
        (expected, actual) => panic!("archive trace changed admission: {expected:?}/{actual:?}"),
    }
}

#[test]
fn real_archive_trace_repeats_all_reads_and_budget_steps_before_returning_report() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let memo = ScopeEncodingMemo::new();
    let expected = seed(&fixture, &root, &task, &memo);
    let manifest = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let binding = |_: &AtomicBool| Ok(None);
    let transaction = Transaction::new(Some(&memo), &binding);
    let trace = transaction.archive(&manifest).expect("real full verifier retained its trace");
    assert_eq!(trace.steps.len(), expected.metadata_read_steps);
    let stop = AtomicBool::new(false);
    let mut target = Target {
        root: &root, stop: &stop, budget: Budget::default(), encodings: None,
    };
    assert!(replay(&trace, &mut target).unwrap().is_none());
    assert_eq!(target.budget.bytes, expected.verified_metadata_bytes);
    assert_eq!(target.budget.reads, expected.metadata_read_steps);
    assert!(compare(&root, &task, &memo, &stop).is_none());
    for (reads, bytes) in [(MAX_READS, 0), (0, MAX_BYTES)] {
        let mut original = Target {
            root: &root, stop: &stop, budget: Budget { reads, bytes }, encodings: None,
        };
        let mut retained = Target {
            root: &root, stop: &stop, budget: Budget { reads, bytes }, encodings: None,
        };
        let parsed: Manifest = decode_proof(&manifest).unwrap();
        let expected = verify(&parsed, &manifest, task.selection.first(), &mut original)
            .err().unwrap();
        let actual = replay(&trace, &mut retained).err().unwrap();
        assert_eq!(error_signature(&actual), error_signature(&expected));
        assert_eq!(retained.budget.bytes, original.budget.bytes);
        assert_eq!(retained.budget.reads, original.budget.reads);
    }
}

#[test]
fn changed_missing_and_cancelled_archive_inputs_keep_the_original_first_failure() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let memo = ScopeEncodingMemo::new();
    seed(&fixture, &root, &task, &memo);
    let before = fixture_file_bytes(fixture._temp.path());
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&bytes).unwrap();
    let first = object_path(root.path(), "candidate", &manifest.publications[0].candidate).unwrap();
    let last = object_path(
        root.path(), "publication", &manifest.publications.last().unwrap().publication,
    ).unwrap();
    let original = fs::read(&first).unwrap();
    let later = fs::read(&last).unwrap();
    let stop = AtomicBool::new(false);
    let mut wrong_task_boundary = task.selection.first().clone();
    wrong_task_boundary.request.expected_sequence += 1;
    let binding = |_: &AtomicBool| Ok(None);
    let mut boundary = Transaction::new(Some(&memo), &binding);
    let expected = super::super::read(
        &root, &wrong_task_boundary, &task.archive_hash, &stop,
    ).unwrap_err();
    let actual = read(&root, &wrong_task_boundary, &task, &stop, &mut boundary).unwrap_err();
    assert_eq!(error_signature(&actual), error_signature(&expected));
    drop(boundary);
    for bad in [b"{}".as_slice(), b"{\"x\":1,\"x\":2}", b"not json"] {
        fs::write(&first, bad).unwrap();
        fs::remove_file(&last).unwrap();
        assert!(compare(&root, &task, &memo, &stop).is_some());
        assert_eq!(fs::read(&first).unwrap(), bad);
        assert!(!last.exists());
        fs::write(&last, &later).unwrap();
    }
    let mut whitespace = b" \n".to_vec();
    whitespace.extend_from_slice(&original);
    fs::write(&first, &whitespace).unwrap();
    assert!(compare(&root, &task, &memo, &stop).is_none());
    fs::write(&first, &original).unwrap();
    fs::remove_file(&first).unwrap();
    assert!(compare(&root, &task, &memo, &stop).is_some());
    fs::write(&first, &original).unwrap();
    let journal = journal_path(root.path(), 0);
    let journal_bytes = fs::read(&journal).unwrap();
    fs::write(&journal, &journal_bytes[..journal_bytes.len() - 1]).unwrap();
    assert!(compare(&root, &task, &memo, &stop).is_some());
    fs::write(&journal, journal_bytes).unwrap();
    stop.store(true, Ordering::Relaxed);
    assert_eq!(compare(&root, &task, &memo, &stop), Some(ErrorCode::OperationAborted));
    stop.store(false, Ordering::Relaxed);
    assert!(compare(&root, &task, &memo, &stop).is_none());
    assert_eq!(fixture_file_bytes(fixture._temp.path()), before);
}

#[test]
fn fallback_consumes_the_actual_changed_read_even_after_bytes_and_stop_change() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let memo = ScopeEncodingMemo::new();
    seed(&fixture, &root, &task, &memo);
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&bytes).unwrap();
    let binding = |_: &AtomicBool| Ok(None);
    let transaction = Transaction::new(Some(&memo), &binding);
    let trace = transaction.archive(&bytes).unwrap();
    let path = object_path(root.path(), "candidate", &manifest.publications[0].candidate).unwrap();
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"not json").unwrap();
    let stop = AtomicBool::new(false);
    let mut baseline = Target {
        root: &root, stop: &stop, budget: Budget::default(), encodings: None,
    };
    let expected = verify(&manifest, &bytes, task.selection.first(), &mut baseline).err().unwrap();
    let mut target = Target {
        root: &root, stop: &stop, budget: Budget::default(), encodings: None,
    };
    let prefix = replay(&trace, &mut target).unwrap().unwrap();
    fs::write(&path, &original).unwrap();
    stop.store(true, Ordering::Relaxed);
    target.budget = Budget::default();
    let mut fallback = Replay { target, prefix };
    let actual = verify(&manifest, &bytes, task.selection.first(), &mut fallback).err().unwrap();
    assert_eq!(error_signature(&actual), error_signature(&expected));
    assert_eq!(fallback.budget().bytes, baseline.budget.bytes);
    assert_eq!(fallback.budget().reads, baseline.budget.reads);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn archive_trace_refusal_and_failed_transaction_leave_original_admission_unchanged() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let before = fixture_file_bytes(fixture._temp.path());
    for memo in [
        memo_with_limit(1, 256),
        memo_with_archive_headroom(&bytes, 0),
        memo_with_limit(32 * 1024 * 1024, 0),
    ] {
        seed(&fixture, &root, &task, &memo);
        let binding = |_: &AtomicBool| Ok(None);
        let transaction = Transaction::new(Some(&memo), &binding);
        assert!(transaction.archive(&bytes).is_none());
        assert_eq!((memo_usage(&memo).2, memo_usage(&memo).3), (0, 0));
        assert!(compare(&root, &task, &memo, &AtomicBool::new(false)).is_none());
    }
    let shared = memo_with_limit(32 * 1024 * 1024, 1);
    with_pending_system_byte(&shared, || {
        seed(&fixture, &root, &task, &shared);
        assert_eq!(memo_usage(&shared).1, 0);
        assert_eq!(memo_usage(&shared).3, 1);
    });
    assert_eq!(memo_usage(&shared), (0, 0, 0, 0));
    seed(&fixture, &root, &task, &shared);
    let binding = |_: &AtomicBool| Ok(None);
    let transaction = Transaction::new(Some(&shared), &binding);
    assert!(transaction.archive(&bytes).is_some());
    assert_eq!(memo_usage(&shared).1, 1);
    drop(transaction);
    let memo = ScopeEncodingMemo::new();
    {
        let binding = |_: &AtomicBool| Ok(None);
        let mut transaction = Transaction::new(Some(&memo), &binding);
        read(&root, task.selection.first(), &task, &AtomicBool::new(false), &mut transaction)
            .unwrap();
        assert!(memo_usage(&memo).3 > 0);
        assert!(transaction.archive(&bytes).is_some());
        let independent = Transaction::new(Some(&memo), &binding);
        assert!(independent.archive(&bytes).is_none());
        // A later registry/name/stop error drops this exact uncommitted account.
    }
    assert_eq!(memo_usage(&memo), (0, 0, 0, 0));
    assert_eq!(fixture_file_bytes(fixture._temp.path()), before);
}

#[test]
fn pending_archive_reuse_is_local_and_keeps_fresh_reads_cancellation_and_rollback() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let before = fixture_file_bytes(fixture._temp.path());
    let memo = ScopeEncodingMemo::new();
    let stop = AtomicBool::new(false);
    let binding = |_: &AtomicBool| Ok(None);
    let mut transaction = Transaction::new(Some(&memo), &binding);
    let expected = read(&root, task.selection.first(), &task, &stop, &mut transaction).unwrap();
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let trace = transaction.archive(&bytes).unwrap();
    let usage = memo_usage(&memo);
    assert_eq!((usage.0, usage.1), (0, 0));
    assert!(usage.3 > 0);
    assert_eq!(read(
        &root, task.selection.first(), &task, &stop, &mut transaction,
    ).unwrap(), expected);
    assert!(Arc::ptr_eq(&trace, &transaction.archive(&bytes).unwrap()));
    assert_eq!(memo_usage(&memo), usage);
    assert!(Transaction::new(Some(&memo), &binding).archive(&bytes).is_none());

    let manifest: Manifest = decode_proof(&bytes).unwrap();
    let path = object_path(root.path(), "candidate", &manifest.publications[0].candidate).unwrap();
    let original = fs::read(&path).unwrap();
    let mut damaged = original.clone();
    damaged[0] = b'?';
    fs::write(&path, &damaged).unwrap();
    let reference = super::super::read(
        &root, task.selection.first(), &task.archive_hash, &stop,
    ).unwrap_err();
    let failure = read(
        &root, task.selection.first(), &task, &stop, &mut transaction,
    ).unwrap_err();
    assert_eq!(error_signature(&failure), error_signature(&reference));
    assert_eq!(fs::read(&path).unwrap(), damaged);
    fs::write(&path, original).unwrap();
    stop.store(true, Ordering::Relaxed);
    assert_eq!(read(
        &root, task.selection.first(), &task, &stop, &mut transaction,
    ).unwrap_err().code(), ErrorCode::OperationAborted);
    drop(transaction);
    assert_eq!(memo_usage(&memo), (0, 0, 0, 0));
    assert_eq!(fixture_file_bytes(fixture._temp.path()), before);
}

#[test]
fn pending_archive_key_collision_does_not_replace_the_exact_manifest_proof() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let memo = ScopeEncodingMemo::new();
    let stop = AtomicBool::new(false);
    let binding = |_: &AtomicBool| Ok(None);
    let mut transaction = Transaction::new(Some(&memo), &binding);
    let expected = read(&root, task.selection.first(), &task, &stop, &mut transaction).unwrap();
    let path = manifest_path(root.path());
    let bytes = root.read_bytes(&path, MAX_MANIFEST).unwrap();
    let trace = transaction.archive(&bytes).unwrap();
    let usage = memo_usage(&memo);
    let mut different_raw = bytes.clone();
    different_raw.push(b'\n');
    move_pending_archive_lookup(&mut transaction, &bytes, &different_raw);
    assert!(!transaction.archive(&different_raw).unwrap().matches(&different_raw));
    fs::write(&path, &different_raw).unwrap();
    let reference = super::super::read(
        &root, task.selection.first(), &task.archive_hash, &stop,
    ).unwrap();
    assert_eq!(reference.verified_metadata_bytes, expected.verified_metadata_bytes + 1);
    assert_eq!(read(
        &root, task.selection.first(), &task, &stop, &mut transaction,
    ).unwrap(), reference);
    assert!(Arc::ptr_eq(&trace, &transaction.archive(&different_raw).unwrap()));
    assert_eq!(memo_usage(&memo), usage);
    fs::write(path, bytes).unwrap();
    drop(transaction);
    assert_eq!(memo_usage(&memo), (0, 0, 0, 0));
}

#[test]
fn warm_archive_trace_does_not_bypass_the_outer_actual_target_binding() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let task = task(&fixture);
    let memo = ScopeEncodingMemo::new();
    seed(&fixture, &root, &task, &memo);
    let parked = fixture.target.with_file_name("parked-target");
    fs::rename(&fixture.target, &parked).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&fixture.target).unwrap();
    let before = fixture_file_bytes(fixture._temp.path());
    let stop = AtomicBool::new(false);
    let expected = super::super::super::verify_completed(&task, &[&root], &stop).unwrap_err();
    let binding = |_: &AtomicBool| Ok(None);
    let mut transaction = Transaction::new(Some(&memo), &binding);
    let actual = super::super::super::verify_completed_with_encodings(
        &task, &[&root], &stop, &mut transaction,
    ).unwrap_err();
    assert_eq!(error_signature(&actual), error_signature(&expected));
    assert_eq!(actual.code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(fixture_file_bytes(fixture._temp.path()), before);
    drop(transaction);
    fs::remove_dir(&fixture.target).unwrap();
    fs::rename(&parked, &fixture.target).unwrap();
    assert!(compare(&root, &task, &memo, &stop).is_none());
}
