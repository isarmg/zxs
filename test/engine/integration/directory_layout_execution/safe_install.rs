use super::*;
use std::sync::{Arc, Barrier};

fn rename(
    engine: &mut Engine,
    repository: &str,
    operation: OperationId,
) -> storage_engine::Result<storage_engine::OperationResult> {
    engine.file_namespace(
        repository,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename,
            "file.txt",
            Some("renamed.txt"),
        )
        .unwrap(),
        "admin",
        operation,
    )
}

#[test]
fn process_death_at_each_owned_install_boundary_recovers_the_same_decision() {
    const CHILD: &str = "ZXS_OWNED_DATABASE_INSTALL_CHILD";
    if let Ok(input) = std::env::var(CHILD) {
        let (system, repository, operation, point): (PathBuf, String, OperationId, String) =
            json_codec::from_str(&input).unwrap();
        let mut engine = Engine::open(&system).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        engine.inject_pause_once(&point, Arc::clone(&barrier));
        thread::spawn(move || {
            barrier.wait();
            std::process::exit(86);
        });
        let _ = rename(&mut engine, &repository, operation);
        panic!("production installer did not reach {point}");
    }
    for point in [
        "after_database_stage",
        "after_database_intent_prepare",
        "after_database_intent",
        "after_database_publish",
    ] {
        let fixture = active();
        let original = fixture.entry("file.txt");
        let root = fixture.root();
        let prior = fs::read(root.join(NAME)).unwrap();
        let operation = op();
        let Fixture {
            temporary,
            primary,
            engine,
            repository,
        } = fixture;
        drop(engine);
        let system = temporary.path().join("system");
        let input = json_codec::to_string(&(
            system.clone(),
            repository.repository_id.clone(),
            operation,
            point,
        ))
        .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sqlite_install_recovery_review::safe_install::process_death_at_each_owned_install_boundary_recovers_the_same_decision", "--nocapture"])
            .env(CHILD, input).status().unwrap();
        assert_eq!(status.code(), Some(86), "{point}");
        // Exhaust the new-operation retention allowance while the original
        // decision is pending. Its own orphan must not be an administrative
        // reason to abandon recovery; the real disk reserve still applies.
        let quota_marker = if point == "after_database_stage" {
            let retained = primary
                .join("histories")
                .join(&repository.history_tree_id)
                .join(".zxs-layout/database-installs");
            let used: u64 = file_snapshot(&retained)
                .values()
                .map(|bytes| bytes.len() as u64)
                .sum();
            let path = retained.join("quota-boundary-unknown");
            fs::File::create(&path)
                .unwrap()
                .set_len(256 * 1024 * 1024 - used)
                .unwrap();
            Some(path)
        } else {
            None
        };
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".state-atomic-")
        }));
        let inspection = Engine::open_for_recovery(&system).unwrap();
        assert!(inspection.is_read_only());
        assert_eq!(
            inspection
                .operation_record(operation)
                .unwrap()
                .result()
                .commit_state,
            storage_engine::CommitState::InDoubt
        );
        drop(inspection);
        let engine = Engine::open(&system).unwrap();
        let fixture = Fixture {
            temporary,
            primary,
            engine,
            repository,
        };
        assert_eq!(
            fixture
                .engine
                .operation_record(operation)
                .unwrap()
                .result()
                .commit_state,
            storage_engine::CommitState::Published,
            "{point}"
        );
        assert_eq!(fixture.entry("renamed.txt").file_id, original.file_id);
        fixture.assert_body(&root.join("renamed.txt"), &original, b"published bytes");
        assert_eq!(
            read_snapshot(&fs::read(root.join(NAME)).unwrap()).sequence,
            fixture.engine.state().sequence
        );
        // Retained old bytes or the orphan are not discarded by successful replay.
        let history = fixture.history();
        let retained_root = history.join(".zxs-layout/database-installs");
        if let Some(marker) = &quota_marker {
            assert!(fs::metadata(marker).unwrap().len() > 0);
        }
        let retained = installation_payloads(&retained_root);
        assert!(retained.values().any(|bytes| bytes == &prior), "{point}");
        let sequence = fixture.engine.state().sequence;
        let mut fixture = fixture.reopen();
        assert_eq!(fixture.engine.state().sequence, sequence);
        if quota_marker.is_some() {
            let next = fixture.engine.file_namespace(
                &fixture.repository.repository_id,
                NamespaceOperation::parse(
                    NamespaceOperationKind::Rename,
                    "renamed.txt",
                    Some("again.txt"),
                )
                .unwrap(),
                "admin",
                op(),
            );
            assert_eq!(next.unwrap_err().code(), ErrorCode::ResourceLimit);
            assert_eq!(fixture.engine.state().sequence, sequence);
        }
    }
}

struct PausedBoundary {
    release: mpsc::SyncSender<()>,
    witness: thread::JoinHandle<()>,
}
impl PausedBoundary {
    fn wait(self) {
        self.release.send(()).unwrap();
        self.witness.join().unwrap();
    }
}

struct PausedRename {
    temporary: tempfile::TempDir,
    operation: OperationId,
    barrier: PausedBoundary,
    worker: thread::JoinHandle<(
        Engine,
        storage_engine::Result<storage_engine::OperationResult>,
    )>,
}

fn pause_rename(fixture: Fixture) -> PausedRename {
    let Fixture {
        temporary,
        primary: _,
        mut engine,
        repository,
    } = fixture;
    let operation = op();
    let barrier = Arc::new(Barrier::new(2));
    engine.inject_pause_once("before_database_publish", Arc::clone(&barrier));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let boundary = Arc::clone(&barrier);
    let witness = thread::spawn(move || {
        boundary.wait();
        ready_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        boundary.wait();
    });
    let repository_id = repository.repository_id.clone();
    let worker = thread::spawn(move || {
        let result = rename(&mut engine, &repository_id, operation);
        (engine, result)
    });
    ready_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("production reached install boundary");
    PausedRename {
        temporary,
        operation,
        barrier: PausedBoundary {
            release: release_tx,
            witness,
        },
        worker,
    }
}

#[test]
fn retained_entity_cannot_be_redirected_by_replacing_its_named_directory() {
    let fixture = active();
    let root = fixture.root();
    let original_identity = identity(&root);
    let PausedRename {
        temporary,
        operation: _,
        barrier,
        worker,
    } = pause_rename(fixture);
    let preserved = temporary.path().join("original-entity");
    fs::rename(&root, &preserved).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    fs::write(root.join(NAME), b"unrelated directory database").unwrap();
    let unknown = file_snapshot(&root);
    barrier.wait();
    let (engine, result) = worker.join().unwrap();
    assert_eq!(result.unwrap_err().code(), ErrorCode::InDoubt);
    assert_eq!(file_snapshot(&root), unknown);
    assert_eq!(identity(&preserved), original_identity);
    assert!(
        read_snapshot(&fs::read(preserved.join(NAME)).unwrap()).sequence > engine.state().sequence
    );
}

#[test]
fn exchange_preserves_late_foreign_inode_and_cold_recovery_does_not_bypass_it() {
    let fixture = active();
    let root = fixture.root();
    let history = root.parent().unwrap().parent().unwrap().to_path_buf();
    let PausedRename {
        temporary,
        operation,
        barrier,
        worker,
    } = pause_rename(fixture);
    let foreign = temporary.path().join("foreign-db");
    fs::write(&foreign, b"unknown must survive exchange").unwrap();
    let inode = fs::metadata(&foreign).unwrap().ino();
    fs::rename(&foreign, root.join(NAME)).unwrap();
    barrier.wait();
    let (engine, result) = worker.join().unwrap();
    assert_eq!(result.unwrap_err().code(), ErrorCode::InDoubt);
    let retained = file_snapshot(&history.join(".zxs-layout/database-installs"));
    let kept = retained
        .iter()
        .find(|(_, bytes)| bytes.as_slice() == b"unknown must survive exchange")
        .expect("foreign bytes retained")
        .0;
    assert_eq!(
        fs::metadata(history.join(".zxs-layout/database-installs").join(kept))
            .unwrap()
            .ino(),
        inode
    );
    drop(engine);
    assert!(Engine::open(&temporary.path().join("system")).is_err());
    let inspection = Engine::open_for_recovery(&temporary.path().join("system")).unwrap();
    assert_eq!(
        inspection
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        storage_engine::CommitState::InDoubt
    );
    assert_eq!(
        fs::read(history.join(".zxs-layout/database-installs").join(kept)).unwrap(),
        b"unknown must survive exchange"
    );
}

#[test]
fn first_install_does_not_clobber_a_late_unknown_target() {
    let fixture = active();
    let root = fixture.root();
    fs::remove_file(root.join(NAME)).unwrap(); // explicit fixture-only missing cache
    let PausedRename {
        temporary: _temporary,
        operation: _,
        barrier,
        worker,
    } = pause_rename(fixture);
    fs::write(root.join(NAME), b"newly arrived foreign target").unwrap();
    barrier.wait();
    let (_engine, result) = worker.join().unwrap();
    assert_eq!(result.unwrap_err().code(), ErrorCode::InDoubt);
    assert_eq!(
        fs::read(root.join(NAME)).unwrap(),
        b"newly arrived foreign target"
    );
}

#[test]
fn exchange_detects_changed_bytes_on_the_same_old_inode_and_retains_them() {
    let fixture = active();
    let root = fixture.root();
    let history = fixture.history();
    let inode = fs::metadata(root.join(NAME)).unwrap().ino();
    let PausedRename {
        temporary: _temporary,
        operation: _,
        barrier,
        worker,
    } = pause_rename(fixture);
    fs::write(root.join(NAME), b"same inode changed after validation").unwrap();
    assert_eq!(fs::metadata(root.join(NAME)).unwrap().ino(), inode);
    barrier.wait();
    let (_engine, result) = worker.join().unwrap();
    assert_eq!(result.unwrap_err().code(), ErrorCode::InDoubt);
    let retained = file_snapshot(&history.join(".zxs-layout/database-installs"));
    let path = retained
        .iter()
        .find(|(_, bytes)| bytes.as_slice() == b"same inode changed after validation")
        .unwrap()
        .0;
    assert_eq!(
        fs::metadata(history.join(".zxs-layout/database-installs").join(path))
            .unwrap()
            .ino(),
        inode
    );
}

#[test]
fn retention_budget_refuses_a_new_decision_without_deleting_unknown_material() {
    let mut fixture = active();
    let unknown = fixture
        .history()
        .join(".zxs-layout/database-installs/unknown-retained");
    let file = fs::File::create(&unknown).unwrap();
    file.set_len(256 * 1024 * 1024).unwrap();
    let sequence = fixture.engine.state().sequence;
    let operation = op();
    let error = rename(
        &mut fixture.engine,
        &fixture.repository.repository_id,
        operation,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert_eq!(fs::metadata(&unknown).unwrap().len(), 256 * 1024 * 1024);
    assert!(!fixture.engine.state().operations.contains_key(&operation));
}

fn installation_payloads(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for generation in fs::read_dir(root).unwrap() {
        let generation = generation.unwrap();
        if !generation.file_type().unwrap().is_dir() {
            continue;
        }
        for attempt in fs::read_dir(generation.path()).unwrap() {
            let path = attempt.unwrap().path().join("payload");
            if path.is_file() {
                let bytes = fs::read(&path).unwrap();
                result.insert(path, bytes);
            }
        }
    }
    result
}
