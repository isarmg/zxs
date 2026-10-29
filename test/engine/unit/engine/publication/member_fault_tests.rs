//! Parallel-only publication faults: one member of a cross-disk stage fails,
//! pauses or loses its disk while the other members of the same stage run.
//! Every case keeps the original operation's acknowledgement point: a PREPARE
//! failure is reported as is before any ACCEPT, every later failure is
//! IN_DOUBT, and a reopen replays the same decision once.
use super::*;
use crate::JournalFault;
use std::fs;
use std::sync::Barrier;
use std::time::{Duration, Instant};

// Timeouts are deadlock watchdogs only; ordering uses barriers and durable state.
const WATCHDOG: Duration = Duration::from_secs(30);

struct Fixture {
    temp: tempfile::TempDir,
    root: std::path::PathBuf,
}

fn fixture() -> (Fixture, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("parallel").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    (Fixture { temp, root }, engine, repo)
}

fn request(engine: &mut Engine, repo: &Repository, bytes: &[u8]) -> CommitRequest {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "member.txt", bytes)
        .unwrap();
    CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "member fault".into(),
        request_digest: None,
    }
}

fn kinds(disk: &DiskConfig, sequence: u64) -> Vec<RecordType> {
    crate::journal::scan(disk)
        .unwrap()
        .records
        .iter()
        .filter(|(record, _)| record.sequence == sequence)
        .map(|(record, _)| record.record_type)
        .collect()
}

fn has(disk: &DiskConfig, sequence: u64, kind: RecordType) -> bool {
    kinds(disk, sequence).contains(&kind)
}

fn candidate_of(disk: &DiskConfig, sequence: u64) -> (String, Candidate) {
    let scan = crate::journal::scan(disk).unwrap();
    let (record, _) = scan
        .records
        .iter()
        .find(|(record, _)| record.sequence == sequence)
        .unwrap();
    let candidate = load_object(disk, "candidate", &record.candidate_hash).unwrap();
    (record.candidate_hash.clone(), candidate)
}

fn journal_sizes(members: &[DiskConfig]) -> Vec<u64> {
    members
        .iter()
        .map(|disk| fs::metadata(log_path(disk)).unwrap().len())
        .collect()
}

fn content(engine: &Engine, version: &str) -> Vec<u8> {
    use std::io::Read;
    let mut opened = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

/// Reopen after the failure: the same operation is published exactly once,
/// a retry of the original request returns the same commit, and a further
/// reopen appends nothing.
fn assert_replayed_once(
    fixture: &Fixture,
    repo: &Repository,
    request: &CommitRequest,
    sequence: u64,
    bytes: &[u8],
    label: &str,
) {
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert!(!engine.is_read_only(), "{label}");
    let result = engine.operation(request.operation_id).unwrap();
    assert_eq!(result.commit_state, CommitState::Published, "{label}");
    assert_eq!(result.operation_id, request.operation_id, "{label}");
    assert_eq!(result.sequence, sequence, "{label}");
    let expected = canonical_json(&result).unwrap();
    let before_retry = journal_sizes(&engine.members());
    assert_eq!(
        canonical_json(&engine.commit_workspace(request.clone()).unwrap()).unwrap(),
        expected,
        "{label}"
    );
    assert_eq!(journal_sizes(&engine.members()), before_retry, "{label}");
    let files = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(files.len(), 1, "{label}");
    assert_eq!(content(&engine, &files[0].version_id), bytes, "{label}");
    assert_eq!(engine.state().versions.len(), 1, "{label}");
    let members = engine.members();
    let (_, candidate) = candidate_of(&members[0], sequence);
    let (catalog_entry, _) = crate::metadata_map::read_proof(
        &candidate.history_roots,
        &repo.history_tree_id,
        |kind, hash| {
            crate::publication_evidence::verified_graph_object_at(
                &engine.disk_resources,
                &members,
                kind,
                hash,
            )
        },
    )
    .unwrap();
    let catalog_entry = catalog_entry.unwrap();
    let published_root = engine.history_root(&repo.history_tree_id).unwrap();
    assert_eq!(
        published_root.as_str(),
        crate::history_package::catalog_entry(&repo.history_tree_id, &catalog_entry).unwrap(),
        "{label}: recovery must bind the admitted candidate's exact history catalog"
    );
    let wrote = members
        .iter()
        .map(|disk| disk.disk_id.clone())
        .collect::<Vec<_>>();
    assert_stage_boundary(
        &engine,
        &candidate,
        StageKind::PublicationProofs,
        &wrote,
        label,
    );
    let sizes = journal_sizes(&members);
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(
        engine.history_root(&repo.history_tree_id).unwrap(),
        published_root,
        "{label}: the recovered history root survives another reopen"
    );
    assert_eq!(
        canonical_json(&engine.commit_workspace(request.clone()).unwrap()).unwrap(),
        expected,
        "{label}"
    );
    assert_eq!(
        journal_sizes(&members),
        sizes,
        "{label}: replay is not repeated"
    );
}

const MEMBER_STAGES: [StageKind; 7] = [
    StageKind::Prepare,
    StageKind::Accept,
    StageKind::Finalize,
    StageKind::CommitProofs,
    StageKind::Publication,
    StageKind::Seals,
    StageKind::PublicationProofs,
];

fn stage_members(engine: &Engine, stage: StageKind) -> Vec<DiskConfig> {
    match stage {
        StageKind::CommitProofs | StageKind::PublicationProofs => engine
            .route("P01", 3)
            .unwrap()
            .iter()
            .map(|slot| engine.disk(slot).unwrap().clone())
            .collect(),
        _ => engine.members(),
    }
}

fn fault(after_write: bool) -> MemberFault {
    if after_write {
        MemberFault::FailAfterWrite
    } else {
        MemberFault::FailBeforeWrite
    }
}

/// Every task reaches the gate before any failure can close admission. This
/// makes first/middle failures and non-prefix successful sets deterministic.
fn gated_commit(
    engine: &mut Engine,
    request: &CommitRequest,
    stage: StageKind,
    failed: &[usize],
    after_write: bool,
) -> Result<OperationResult> {
    let members = stage_members(engine, stage);
    assert!(members.len() <= stages::MAX_STAGE_WORKERS);
    let (ready, arrivals) = std::sync::mpsc::channel();
    let mut releases = Vec::new();
    for (index, disk) in members.iter().enumerate() {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let member_fault = MemberFault::Gated {
            ready: ready.clone(),
            release: std::sync::Mutex::new(wait),
            fault: failed
                .contains(&index)
                .then(|| Box::new(fault(after_write))),
        };
        engine
            .inject_member_fault_once(stage, &disk.slot_id, member_fault)
            .unwrap();
    }
    drop(ready);
    std::thread::scope(|scope| {
        let commit = scope.spawn(|| engine.commit_workspace(request.clone()));
        let deadline = Instant::now() + WATCHDOG;
        let arrived = (0..members.len()).try_for_each(|_| {
            arrivals.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        });
        let returned_while_held = commit.is_finished();
        // On timeout or pre-stage failure, dropping every sender cancels all
        // started gates. Never panic while workers still wait for release.
        let released = if arrived.is_ok() {
            releases
                .into_iter()
                .map(|release| release.send(()))
                .collect::<Vec<_>>()
        } else {
            drop(releases);
            Vec::new()
        };
        let result = commit.join().unwrap();
        arrived.expect("member stage did not reach every gate before the watchdog");
        assert!(released.iter().all(|release| release.is_ok()));
        assert!(
            !returned_while_held,
            "stage returned with started tasks held"
        );
        result
    })
}

fn proof_directory(
    engine: &Engine,
    disk: &DiskConfig,
    hash: &str,
    version: &FileVersion,
) -> std::path::PathBuf {
    let root = &engine.disk_resources.get(disk).unwrap().root;
    let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
    local_proof_directory_at(root, &directory, hash).unwrap()
}

fn stage_rank(stage: StageKind) -> usize {
    MEMBER_STAGES
        .iter()
        .position(|member| *member == stage)
        .unwrap()
}

/// Check the complete durable stage prefix, including both proof families at
/// their authoritative candidate-specific paths, not just journal records.
fn assert_stage_boundary(
    engine: &Engine,
    candidate: &Candidate,
    stage: StageKind,
    wrote: &[String],
    label: &str,
) {
    let rank = stage_rank(stage);
    let hash = object_hash("candidate", candidate).unwrap();
    let version = &candidate.versions[0].version;
    for disk in engine.members() {
        let present = |milestone| {
            stage_rank(milestone) < rank || (milestone == stage && wrote.contains(&disk.disk_id))
        };
        let records = kinds(&disk, candidate.sequence);
        for (milestone, record) in [
            (StageKind::Prepare, RecordType::Prepare),
            (StageKind::Accept, RecordType::Accept),
            (StageKind::Finalize, RecordType::Finalize),
            (StageKind::Publication, RecordType::Publication),
        ] {
            assert_eq!(
                records.iter().filter(|kind| **kind == record).count(),
                usize::from(present(milestone)),
                "{label}: {record:?} on {}",
                disk.disk_id
            );
        }
        let directory = proof_directory(engine, &disk, &hash, version);
        let commit_path = directory.join("commit-proof.json");
        assert_eq!(
            commit_path.exists(),
            present(StageKind::CommitProofs),
            "{label}"
        );
        if present(StageKind::CommitProofs) {
            let proof: CommitProof = read_json(&commit_path).unwrap();
            assert_eq!(proof.candidate_hash, hash, "{label}");
            assert_eq!(proof.disk_id, disk.disk_id, "{label}");
            assert_eq!(
                proof.version_hash,
                object_hash("version", version).unwrap(),
                "{label}"
            );
        }
        let seal_path = publication_seal_path(&disk, &hash);
        assert_eq!(seal_path.exists(), present(StageKind::Seals), "{label}");
        if present(StageKind::Seals) {
            let seal: PublicationSeal = read_json(&seal_path).unwrap();
            let publication: Publication =
                load_object(&disk, "publication", &seal.publication_hash).unwrap();
            verify_publication_seal(candidate, &publication, &seal).unwrap();
        }
        let publication_path = directory.join("publication-proof.json");
        assert_eq!(
            publication_path.exists(),
            present(StageKind::PublicationProofs),
            "{label}: {}",
            disk.disk_id
        );
        if present(StageKind::PublicationProofs) {
            verify_local_publication(&directory, version).unwrap();
        }
    }
}

fn assert_completed_io(timing: &StageTiming, writes: usize, label: &str) {
    let writes = writes as u32;
    let (objects, records, proofs) = match timing.stage {
        StageKind::Prepare | StageKind::Finalize | StageKind::Publication => (writes, writes, 0),
        StageKind::Accept => (0, writes, 0),
        StageKind::CommitProofs | StageKind::Seals | StageKind::PublicationProofs => (0, 0, writes),
        _ => unreachable!(),
    };
    assert_eq!(timing.io.objects_completed, objects, "{label}");
    assert_eq!(timing.io.records_completed, records, "{label}");
    assert_eq!(timing.io.proofs_completed, proofs, "{label}");
    assert_eq!(timing.io.logical_bytes_completed > 0, writes > 0, "{label}");
    assert_eq!(timing.io.payload_source_bytes_completed, 0, "{label}");
    assert_eq!(
        timing.io.observed_bytes_read > 0,
        timing.stage == StageKind::PublicationProofs && writes > 0,
        "{label}"
    );
}

fn assert_unknown_then_retried_once(
    fixture: &Fixture,
    repo: &Repository,
    request: &CommitRequest,
    failed_sequence: u64,
    previous_root: &crate::HistoryRoot,
    label: &str,
) {
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert!(!engine.is_read_only(), "{label}");
    assert_eq!(
        &engine.history_root(&repo.history_tree_id).unwrap(),
        previous_root,
        "{label}: PREPARE-only recovery retains the published history catalog"
    );
    assert_eq!(
        engine.operation(request.operation_id).unwrap_err().code(),
        ErrorCode::OperationNotFound,
        "{label}"
    );
    assert!(
        engine.list_tree(&repo.main_branch_id).unwrap().is_empty(),
        "{label}"
    );
    let published = engine.commit_workspace(request.clone()).unwrap();
    assert_eq!(published.operation_id, request.operation_id, "{label}");
    assert!(published.sequence > failed_sequence, "{label}");
    drop(engine);
    assert_replayed_once(
        fixture,
        repo,
        request,
        published.sequence,
        b"parallel member bytes",
        label,
    );
}

fn member_failure_case(stage: StageKind, failed: &[usize], after_write: bool) {
    let label = format!("{stage:?} failed={failed:?} after_write={after_write}");
    let (fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"parallel member bytes");
    let sequence = engine.next_slot;
    let members = stage_members(&engine, stage);
    let previous_root = engine.history_root(&repo.history_tree_id).unwrap();
    let error = gated_commit(&mut engine, &request, stage, failed, after_write).unwrap_err();
    let is_prepare = stage == StageKind::Prepare;
    assert_eq!(
        error.code(),
        if is_prepare {
            ErrorCode::InjectedFailure
        } else {
            ErrorCode::InDoubt
        },
        "{label}"
    );
    assert!(
        error.message().contains(&format!(
            "member {} interrupted in stage {}",
            members[failed[0]].slot_id,
            stage.label()
        )),
        "{label}: the lowest failed member owns the primary error: {}",
        error.message()
    );
    assert_eq!(engine.is_read_only(), !is_prepare, "{label}");
    assert_eq!(
        engine.history_root(&repo.history_tree_id).unwrap(),
        previous_root,
        "{label}: a failed member barrier cannot publish the candidate's history catalog"
    );
    assert!(
        engine.list_tree(&repo.main_branch_id).unwrap().is_empty(),
        "{label}"
    );
    assert!(
        engine
            .published_receipt(request.operation_id)
            .unwrap()
            .is_none(),
        "{label}"
    );
    let report = engine.last_publication_stages().unwrap();
    assert_eq!(
        report.outcome,
        if is_prepare {
            "prepare-failed"
        } else {
            "in-doubt"
        },
        "{label}"
    );
    let last = report.stages.last().unwrap();
    assert_eq!(last.stage, stage, "{label}");
    assert!(last.parallel, "{label}");
    assert_eq!(last.workers, 3, "{label}");
    // Every task was started at the gate. A returned stage must account for
    // all of them, including successful tasks after the first failed index.
    assert_eq!(
        last.members_completed + failed.len(),
        members.len(),
        "{label}"
    );
    let wrote: Vec<String> = members
        .iter()
        .enumerate()
        .filter(|(index, _)| !failed.contains(index) || after_write)
        .map(|(_, disk)| disk.disk_id.clone())
        .collect();
    assert_completed_io(last, wrote.len(), &label);
    if !is_prepare {
        assert!(error.is_retryable(), "{label}");
        let completed = members
            .iter()
            .enumerate()
            .filter(|(index, _)| !failed.contains(index))
            .map(|(_, disk)| disk.disk_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let failed_summary = failed
            .iter()
            .map(|index| format!("{}=INJECTED_FAILURE", members[*index].disk_id))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            error.message().contains(&format!(
                "stage {}; completed [{completed}]; failed [{failed_summary}]",
                stage.label()
            )),
            "{label}: {}",
            error.message()
        );
    }
    let candidate_disk = members
        .iter()
        .find(|disk| has(disk, sequence, RecordType::Prepare))
        .unwrap();
    let (_, candidate) = candidate_of(candidate_disk, sequence);
    assert_stage_boundary(&engine, &candidate, stage, &wrote, &label);
    let accepted = members
        .iter()
        .filter(|disk| has(disk, sequence, RecordType::Accept))
        .count();
    if accepted == 0 {
        assert_eq!(
            engine.operation(request.operation_id).unwrap_err().code(),
            ErrorCode::OperationNotFound,
            "{label}"
        );
    } else {
        let pending = engine.operation(request.operation_id).unwrap();
        assert_eq!(pending.commit_state, CommitState::InDoubt, "{label}");
        assert_eq!(pending.operation_id, request.operation_id, "{label}");
        assert_eq!(pending.sequence, sequence, "{label}");
        assert_eq!(pending.request_digest, candidate.request_digest, "{label}");
    }
    if !is_prepare {
        let sizes = journal_sizes(&members);
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap_err().code(),
            ErrorCode::RecoveryRequired,
            "{label}"
        );
        assert_eq!(
            journal_sizes(&members),
            sizes,
            "{label}: pending retry cannot append"
        );
    }
    drop(engine);
    if accepted == 0 {
        assert_unknown_then_retried_once(
            &fixture,
            &repo,
            &request,
            sequence,
            &previous_root,
            &label,
        );
    } else {
        assert_replayed_once(
            &fixture,
            &repo,
            &request,
            sequence,
            b"parallel member bytes",
            &label,
        );
    }
}

#[test]
fn first_middle_and_last_member_failures_preserve_every_stage_barrier() {
    for stage in MEMBER_STAGES {
        for failed in 0..3 {
            for after_write in [false, true] {
                member_failure_case(stage, &[failed], after_write);
            }
        }
    }
}

#[test]
fn simultaneous_member_failures_preserve_non_prefix_results_and_the_same_decision() {
    for stage in MEMBER_STAGES {
        for after_write in [false, true] {
            member_failure_case(stage, &[0, 2], after_write);
        }
    }
    // N=3, Q=2: the single-member matrix covers two durable ACCEPTs, the
    // pair above covers one, and all-member failures cover zero or three.
    // Recovery may adopt one ACCEPT, but never treats PREPARE as a decision.
    for after_write in [false, true] {
        member_failure_case(StageKind::Accept, &[0, 1, 2], after_write);
    }
}

#[test]
fn dropping_gate_releases_cancels_every_started_member_before_join() {
    let (_fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"cancelled member gates");
    let sequence = engine.next_slot;
    let members = engine.members();
    let (ready, arrivals) = std::sync::mpsc::channel();
    let mut releases = Vec::new();
    for disk in &members {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        engine
            .inject_member_fault_once(
                StageKind::Prepare,
                &disk.slot_id,
                MemberFault::Gated {
                    ready: ready.clone(),
                    release: std::sync::Mutex::new(wait),
                    fault: None,
                },
            )
            .unwrap();
    }
    drop(ready);
    let error = std::thread::scope(|scope| {
        let commit = scope.spawn(|| engine.commit_workspace(request));
        let deadline = Instant::now() + WATCHDOG;
        let arrived = (0..members.len()).try_for_each(|_| {
            arrivals.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        });
        drop(releases);
        let result = commit.join().unwrap();
        arrived.unwrap();
        result.unwrap_err()
    });
    assert_eq!(error.code(), ErrorCode::InjectedFailure);
    for disk in &members {
        assert!(kinds(disk, sequence).is_empty());
    }
    let report = engine.last_publication_stages().unwrap();
    let stage = report.stages.last().unwrap();
    assert_eq!(stage.stage, StageKind::Prepare);
    assert_eq!(stage.members_completed, 0);
    assert_eq!(stage.io, StageIo::default());
}

#[test]
fn large_manifest_metadata_selects_serial_verification_without_large_body_allocation() {
    let (_fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"small scheduling fixture");
    let sequence = engine.next_slot;
    engine.commit_workspace(request).unwrap();
    let (hash, mut candidate) = candidate_of(&engine.members()[0], sequence);
    let caller = std::thread::current().id();
    for (logical_size, expected_workers) in [(24, 3), (1_u64 << 40, 1)] {
        // Only exercise scheduling admission from authenticated-size metadata.
        // The callback below performs no checksum or body I/O, so this is not
        // evidence of a full 1 TiB publication/recovery execution.
        candidate.versions[0].version.logical_size = logical_size;
        candidate.versions[0].version.checksum_chunk_bytes = 65_536;
        let armed = engine.take_member_fault(StageKind::CommitProofs);
        let targets = group_proof_targets(&engine, &candidate, ProofDisks::Required).unwrap();
        let mut stages = StageSequence::new(&hash, targets.len());
        let threads = engine
            .run_disk_stage(
                &mut stages,
                StageKind::CommitProofs,
                &armed,
                &targets,
                |_| TaskOutcome {
                    value: Ok(std::thread::current().id()),
                    io: StageIo::default(),
                },
            )
            .unwrap();
        let report = stages.finish("scheduling-test");
        let timing = &report.stages[0];
        assert_eq!(timing.workers, expected_workers);
        assert_eq!(timing.parallel, expected_workers > 1);
        assert_eq!(timing.members_completed, 3);
        assert_eq!(threads.len(), 3);
        assert_eq!(
            threads.iter().all(|thread| *thread == caller),
            expected_workers == 1
        );
    }
}

#[test]
fn a_member_failure_during_prepare_is_reported_as_is_before_any_accept() {
    for (case, member_fault) in [
        ("before-write", MemberFault::FailBeforeWrite),
        ("after-write", MemberFault::FailAfterWrite),
        (
            "torn-frame",
            MemberFault::Journal(JournalFault::PartialWrite),
        ),
        ("failed-sync", MemberFault::Journal(JournalFault::Sync)),
    ] {
        let journal_fault = matches!(member_fault, MemberFault::Journal(_));
        let (fixture, mut engine, repo) = fixture();
        let request = request(&mut engine, &repo, b"prepare member bytes");
        let sequence = engine.next_slot;
        let members = engine.members();
        let epoch = engine.metadata_integrity.capture();
        engine
            .inject_member_fault_once(StageKind::Prepare, &members[2].slot_id, member_fault)
            .unwrap();
        let error = engine.commit_workspace(request.clone()).unwrap_err();
        assert_ne!(error.code(), ErrorCode::InDoubt, "{case}");
        if !journal_fault {
            assert_eq!(error.code(), ErrorCode::InjectedFailure, "{case}");
            assert!(
                error.message().contains("interrupted in stage prepare"),
                "{case}: the raw member error is returned unchanged"
            );
            assert!(!error.message().contains("query operation"), "{case}");
        }
        // The decision slot was consumed even though nothing was decided.
        assert_eq!(engine.next_slot, sequence + 1, "{case}");
        // A failed journal write fences the engine exactly like a serial
        // append did; a member that failed without touching its journal does not.
        assert_eq!(engine.is_read_only(), journal_fault, "{case}");
        assert_eq!(
            !epoch.matches(&engine.metadata_integrity),
            journal_fault,
            "{case}"
        );
        let report = engine.last_publication_stages().unwrap();
        assert_eq!(report.outcome, "prepare-failed", "{case}");
        assert_eq!(report.stages.last().unwrap().stage, StageKind::Prepare);
        for (index, disk) in members.iter().enumerate() {
            let records = kinds(disk, sequence);
            assert_eq!(
                records.contains(&RecordType::Prepare),
                index != 2 || case == "after-write" || case == "failed-sync",
                "{case}: {}",
                disk.disk_id
            );
            assert!(!records.contains(&RecordType::Accept), "{case}");
        }
        drop(engine);
        let mut engine = Engine::open(&fixture.root).unwrap();
        assert!(!engine.is_read_only(), "{case}");
        assert_eq!(
            engine.operation(request.operation_id).unwrap_err().code(),
            ErrorCode::OperationNotFound,
            "{case}"
        );
        let published = engine.commit_workspace(request.clone()).unwrap();
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap().commit_id,
            published.commit_id,
            "{case}"
        );
        let files = engine.list_tree(&repo.main_branch_id).unwrap();
        assert_eq!(
            content(&engine, &files[0].version_id),
            b"prepare member bytes"
        );
    }
}

#[test]
fn a_journal_device_fault_on_one_member_after_accept_starts_is_in_doubt() {
    for (stage, journal_fault, torn) in [
        (StageKind::Accept, JournalFault::PartialWrite, true),
        (StageKind::Publication, JournalFault::Sync, false),
    ] {
        let label = format!("{stage:?}");
        let (fixture, mut engine, repo) = fixture();
        let request = request(&mut engine, &repo, b"journal fault bytes");
        let sequence = engine.next_slot;
        let members = engine.members();
        engine
            .inject_member_fault_once(
                stage,
                &members[2].slot_id,
                MemberFault::Journal(journal_fault),
            )
            .unwrap();
        let error = engine.commit_workspace(request.clone()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InDoubt, "{label}");
        assert!(engine.is_read_only(), "{label}");
        let kind = match stage {
            StageKind::Accept => RecordType::Accept,
            _ => RecordType::Publication,
        };
        assert!(has(&members[0], sequence, kind), "{label}");
        assert!(has(&members[1], sequence, kind), "{label}");
        assert_eq!(crate::journal::scan(&members[2]).unwrap().torn_tail, torn);
        drop(engine);
        assert_replayed_once(
            &fixture,
            &repo,
            &request,
            sequence,
            b"journal fault bytes",
            &label,
        );
    }
}

/// Poll durable state until `done` holds; the deadline is a watchdog only.
fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WATCHDOG;
    while !done() {
        assert!(Instant::now() < deadline, "watchdog expired");
        std::thread::yield_now();
    }
}

#[test]
fn a_slow_member_holds_the_stage_barrier_until_it_finishes() {
    let (fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"slow member bytes");
    let sequence = engine.next_slot;
    let members = engine.members();
    let barrier = Arc::new(Barrier::new(2));
    engine
        .inject_member_fault_once(
            StageKind::Accept,
            &members[2].slot_id,
            MemberFault::Pause(Arc::clone(&barrier)),
        )
        .unwrap();
    let result = std::thread::scope(|scope| {
        let commit = scope.spawn(|| engine.commit_workspace(request.clone()));
        barrier.wait();
        // Members 1 and 2 finish their ACCEPT while member 3 is still held.
        wait_until(|| {
            has(&members[0], sequence, RecordType::Accept)
                && has(&members[1], sequence, RecordType::Accept)
        });
        assert!(!has(&members[2], sequence, RecordType::Accept));
        for disk in &members {
            assert!(
                !has(disk, sequence, RecordType::Finalize),
                "no FINALIZE before the slowest member's ACCEPT"
            );
        }
        assert!(!commit.is_finished());
        barrier.wait();
        commit.join().unwrap()
    });
    result.unwrap();
    for disk in &members {
        assert_eq!(
            kinds(disk, sequence),
            [
                RecordType::Prepare,
                RecordType::Accept,
                RecordType::Finalize,
                RecordType::Publication,
            ]
        );
    }
    let report = engine.last_publication_stages().unwrap();
    let accept = report
        .stages
        .iter()
        .find(|stage| stage.stage == StageKind::Accept)
        .unwrap();
    assert!(accept.parallel);
    assert_eq!(accept.workers, 3);
    assert_eq!(accept.members_completed, 3);
    drop(engine);
    drop(fixture);
}

#[test]
fn a_disk_lost_inside_a_stage_stays_in_doubt_until_its_member_returns() {
    let (fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"lost member bytes");
    let sequence = engine.next_slot;
    let members = engine.members();
    let lost = members[2].clone();
    let hidden = fixture.temp.path().join("unplugged");
    let barrier = Arc::new(Barrier::new(2));
    engine
        .inject_member_fault_once(
            StageKind::Finalize,
            &lost.slot_id,
            MemberFault::Pause(Arc::clone(&barrier)),
        )
        .unwrap();
    let objects = hidden.join("metadata/objects");
    let result = std::thread::scope(|scope| {
        let commit = scope.spawn(|| engine.commit_workspace(request.clone()));
        barrier.wait();
        // The disk leaves its configured path while its member is inside the
        // stage. A rename keeps held descriptors valid, so the directory its
        // FINALIZE object needs is also replaced by a file: that member's
        // write fails while the other members of the stage complete.
        fs::rename(&lost.root, &hidden).unwrap();
        fs::rename(objects.join("finalize"), objects.join("finalize.offline")).unwrap();
        fs::write(objects.join("finalize"), b"not a directory").unwrap();
        barrier.wait();
        commit.join().unwrap()
    });
    let error = result.unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(error.message().contains("stage finalize"));
    assert!(engine.is_read_only());
    assert!(has(&members[0], sequence, RecordType::Finalize));
    assert!(has(&members[1], sequence, RecordType::Finalize));
    drop(engine);
    fs::remove_file(objects.join("finalize")).unwrap();
    fs::rename(objects.join("finalize.offline"), objects.join("finalize")).unwrap();
    // Two of three members form the recovery quorum: the decided operation is
    // visible as in doubt and nothing is published or writable.
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert!(engine.is_read_only());
    let pending = engine.operation(request.operation_id).unwrap();
    assert_eq!(pending.commit_state, CommitState::InDoubt);
    assert_eq!(pending.operation_id, request.operation_id);
    assert_eq!(pending.sequence, sequence);
    let online = &members[..2];
    let sizes = journal_sizes(online);
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(journal_sizes(online), sizes);
    assert_eq!(
        canonical_json(&engine.operation(request.operation_id).unwrap()).unwrap(),
        canonical_json(&pending).unwrap()
    );
    assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
    drop(engine);
    fs::rename(&hidden, &lost.root).unwrap();
    assert_replayed_once(
        &fixture,
        &repo,
        &request,
        sequence,
        b"lost member bytes",
        "lost disk",
    );
}

#[test]
fn an_armed_member_failpoint_runs_its_stage_serially_in_member_order() {
    let (_fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"serial fallback");
    engine.inject_fault_once("after_accept_2");
    assert_eq!(
        engine.commit_workspace(request).unwrap_err().code(),
        ErrorCode::InDoubt
    );
    let report = engine.last_publication_stages().unwrap();
    let prepare = &report.stages[report.stages.len() - 2];
    assert_eq!(prepare.stage, StageKind::Prepare);
    assert!(prepare.parallel);
    let accept = report.stages.last().unwrap();
    assert_eq!(accept.stage, StageKind::Accept);
    assert!(!accept.parallel);
    assert_eq!(accept.workers, 1);
    assert_eq!(accept.members_completed, 2);
}

#[test]
fn the_stage_report_records_completed_logical_work_and_serial_stage_offsets() {
    let (_fixture, mut engine, repo) = fixture();
    let request = request(&mut engine, &repo, b"accounted bytes");
    let sequence = engine.next_slot;
    let members = engine.members();
    let before = journal_sizes(&members);
    engine.commit_workspace(request).unwrap();
    let growth: u64 = journal_sizes(&members)
        .iter()
        .zip(&before)
        .map(|(after, before)| after - before)
        .sum();
    let report = engine.last_publication_stages().unwrap();
    assert_eq!(report.outcome, "published");
    assert_eq!(report.members, 3);
    assert_eq!(
        report
            .stages
            .iter()
            .map(|stage| stage.stage)
            .collect::<Vec<_>>(),
        [
            StageKind::PayloadCopy,
            StageKind::SnapshotInstall,
            StageKind::ManifestInstall,
            StageKind::Prepare,
            StageKind::Accept,
            StageKind::Finalize,
            StageKind::Names,
            StageKind::CommitProofs,
            StageKind::Publication,
            StageKind::Seals,
            StageKind::PublicationProofs,
        ]
    );
    for pair in report.stages.windows(2) {
        assert!(
            pair[1].started_at >= pair[0].started_at + pair[0].elapsed,
            "{:?} starts before {:?} finished",
            pair[1].stage,
            pair[0].stage
        );
    }
    let stage = |kind| {
        report
            .stages
            .iter()
            .find(|stage| stage.stage == kind)
            .unwrap()
    };
    let (hash, candidate) = candidate_of(&members[0], sequence);
    assert_eq!(report.candidate_hash, hash);
    let object_size = |kind: &str, object: &str| -> u64 {
        members
            .iter()
            .map(|disk| fs::metadata(object_path(disk, kind, object)).unwrap().len())
            .sum()
    };
    let records = crate::journal::scan(&members[0]).unwrap().records;
    let object_of = |kind| {
        records
            .iter()
            .find(|(record, _)| record.sequence == sequence && record.record_type == kind)
            .unwrap()
            .0
            .object_hash
            .clone()
    };
    let objects = object_size("candidate", &hash)
        + object_size("finalize", &object_of(RecordType::Finalize))
        + object_size("publication", &object_of(RecordType::Publication));
    let journal_stages = [
        StageKind::Prepare,
        StageKind::Accept,
        StageKind::Finalize,
        StageKind::Publication,
    ];
    for kind in journal_stages {
        let timing = stage(kind);
        assert!(timing.parallel, "{kind:?}");
        assert_eq!(timing.workers, 3, "{kind:?}");
        assert_eq!(timing.members_completed, 3, "{kind:?}");
        assert_eq!(timing.io.records_completed, 3, "{kind:?}");
    }
    assert_eq!(stage(StageKind::Accept).io.objects_completed, 0);
    for kind in [
        StageKind::Prepare,
        StageKind::Finalize,
        StageKind::Publication,
    ] {
        assert_eq!(stage(kind).io.objects_completed, 3, "{kind:?}");
    }
    // In this successful no-replay case, logical completed frame/object
    // lengths equal the durable growth. This does not count syscalls, partial
    // writes, repeated reads or compare-and-sync costs.
    let reported: u64 = journal_stages
        .iter()
        .map(|kind| stage(*kind).io.logical_bytes_completed)
        .sum();
    assert_eq!(reported, growth + objects);
    let version = &candidate.versions[0].version;
    let mut seals = 0;
    let mut proofs = 0;
    let mut commit_proofs = 0;
    for disk in &members {
        seals += fs::metadata(publication_seal_path(disk, &hash))
            .unwrap()
            .len();
        let directory = proof_directory(&engine, disk, &hash, version);
        commit_proofs += fs::metadata(directory.join("commit-proof.json"))
            .unwrap()
            .len();
        proofs += fs::metadata(directory.join("publication-proof.json"))
            .unwrap()
            .len();
    }
    assert_eq!(stage(StageKind::Seals).io.proofs_completed, 3);
    assert_eq!(stage(StageKind::Seals).io.logical_bytes_completed, seals);
    assert_eq!(stage(StageKind::CommitProofs).io.proofs_completed, 3);
    assert_eq!(stage(StageKind::PublicationProofs).io.proofs_completed, 3);
    assert_eq!(
        stage(StageKind::PublicationProofs)
            .io
            .logical_bytes_completed,
        proofs
    );
    assert_eq!(
        stage(StageKind::PublicationProofs).io.observed_bytes_read,
        commit_proofs
    );
    let payload = stage(StageKind::PayloadCopy);
    assert_eq!(payload.io.logical_bytes_completed, 3 * version.logical_size);
    assert_eq!(
        payload.io.payload_source_bytes_completed,
        version.logical_size
    );
    assert_eq!(report.total_io().records_completed, 12);

    // Replay admission uses the same candidate identity held by the stage
    // sequence; a foreign sequence must reject before any member work.
    let publication = load_object(
        &members[0],
        "publication",
        &object_of(RecordType::Publication),
    )
    .unwrap();
    let next = std::sync::Arc::clone(&engine.state);
    let before_replay = journal_sizes(&members);
    let mut foreign = StageSequence::new("foreign-candidate", members.len());
    let error = engine
        .complete_published_candidate(
            &candidate,
            &next,
            publication,
            |_, _| panic!("a foreign stage sequence must not consult member receipts"),
            AppendMode::Recovery,
            &mut foreign,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert_eq!(journal_sizes(&members), before_replay);
    assert!(foreign.finish("rejected").stages.is_empty());
}
