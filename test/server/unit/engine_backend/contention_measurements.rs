//! Explicit, process-isolated measurement of the real Backend admission mutex.
//! These elapsed acquisitions include scheduler delay and are not owner hold times.
use super::*;
use std::{
    io::Read,
    sync::mpsc,
    time::{Duration, Instant},
};
use storage_contract::{Config, OperationId};

const BODY_BYTES: usize = 64 * 1024 * 1024;
const READS: usize = 32;

#[derive(Clone, Copy)]
enum ReadKind {
    Directory,
    History,
    Receipt,
}

fn command(
    workspace: storage_contract::Workspace,
    operation_id: OperationId,
) -> crate::CommitWorkspace {
    crate::CommitWorkspace {
        workspace_id: workspace.workspace_id,
        operation_id,
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "contention measurement".into(),
        request_digest: None,
    }
}

fn reads(
    kind: ReadKind,
    backend: &EngineBackend,
    owner: &Identity,
    branch: &str,
    file: &str,
    receipt: &storage_contract::OperationResult,
    epoch: Instant,
) -> Vec<Value> {
    (0..READS)
        .map(|_| {
            let start = epoch.elapsed().as_nanos();
            match kind {
                ReadKind::Directory => {
                    let page = backend
                        .read_directory(
                            owner,
                            crate::ReadDirectory {
                                branch,
                                entry_id: branch,
                                path: None,
                                selection: crate::DirectorySelection::Page {
                                    commit: None,
                                    after: None,
                                },
                            },
                        )
                        .unwrap();
                    // These short, fixed-size names fit the byte budget. All scales
                    // exceed the real protocol's 256-entry first-page limit.
                    assert_eq!(
                        page.entries.len(),
                        smb_protocol::protocol::MAX_DIRECTORY_PAGE_ENTRIES
                    );
                    assert!(page.more);
                    for (index, entry) in page.entries.iter().enumerate() {
                        assert_eq!(entry.path.as_str(), format!("file-{index:06}.txt"));
                        assert_eq!(entry.kind, storage_contract::NamespaceEntryKind::File);
                    }
                    assert!(
                        page.entries
                            .windows(2)
                            .all(|pair| pair[0].path.key() < pair[1].path.key())
                    );
                }
                ReadKind::History => {
                    let history = backend.file_history(owner, file).unwrap();
                    assert!(!history.is_empty());
                    assert!(history.iter().all(|version| version.file_id == file));
                }
                ReadKind::Receipt => {
                    let observed = backend.operation(owner, receipt.operation_id).unwrap();
                    assert_eq!(observed.operation_id, receipt.operation_id);
                    assert_eq!(observed.request_digest, receipt.request_digest);
                    assert_eq!(observed.sequence, receipt.sequence);
                    assert_eq!(observed.result, receipt.result);
                }
            }
            json!({"start_ns": start, "end_ns": epoch.elapsed().as_nanos()})
        })
        .collect()
}

fn measure(files: usize) {
    let temporary = crate::tests::private_tempdir();
    let root = temporary.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temporary.path().join("disk")])).unwrap();
    let owner = install_test_administrator(&mut engine, "owner");
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Contention").unwrap(),
            "P01",
            1,
            &owner.principal,
            OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, &owner.principal)
        .unwrap();
    for index in 0..files {
        engine
            .stage_bytes(
                &workspace.workspace_id,
                &format!("file-{index:06}.txt"),
                b"initial",
            )
            .unwrap();
    }
    let backend = EngineBackend::new(engine);
    let initial = backend
        .commit_workspace(&owner, command(workspace, OperationId::generate()))
        .unwrap();
    let path = name_policy::LookupPath::parse("file-000000.txt").unwrap();
    let original = backend
        .lookup_current(&owner, &repository.main_branch_id, &path)
        .unwrap()
        .unwrap();
    let body = vec![0x5a; BODY_BYTES];
    let workspace = {
        let mut engine = backend.engine.lock().unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, &owner.principal)
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, path.as_str(), &body)
            .unwrap();
        workspace
    };
    let commit = command(workspace, OperationId::generate());
    let epoch = Instant::now();
    let (writer_started, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let (published, writer_interval, query_samples, acquisitions, probe_complete) =
        std::thread::scope(|scope| {
            let writer_backend = &backend;
            let writer_owner = &owner;
            let writer_command = commit.clone();
            let writer = scope.spawn(move || {
                writer_started.send(()).unwrap();
                let start = epoch.elapsed().as_nanos();
                let result = writer_backend
                    .commit_workspace(writer_owner, writer_command)
                    .unwrap();
                let end = epoch.elapsed().as_nanos();
                done.send(()).unwrap();
                (result, json!({"start_ns":start,"end_ns":end}))
            });
            started.recv_timeout(Duration::from_secs(30)).unwrap();
            let readers: Vec<_> = [ReadKind::Directory, ReadKind::History, ReadKind::Receipt]
                .into_iter()
                .map(|kind| {
                    scope.spawn({
                        let backend = &backend;
                        let owner = &owner;
                        let branch = &repository.main_branch_id;
                        let file = &original.file_id;
                        let initial = &initial;
                        move || reads(kind, backend, owner, branch, file, initial, epoch)
                    })
                })
                .collect();
            // Sample the actual admission mutex, not a stand-in. The 5 ms pause
            // limits interference; 12,000 is a sample-attempt cap, not a wall-time
            // bound, because acquiring the mutex can itself block. The process
            // runner owns the independent wall-time and resource deadlines.
            let mut acquisitions = Vec::new();
            let mut probe_complete = false;
            for _ in 0..12_000 {
                match finished.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) => {
                        probe_complete = true;
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        panic!("writer failed before completion")
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let start = epoch.elapsed().as_nanos();
                        let guard = backend.engine.lock().unwrap();
                        let acquired = epoch.elapsed().as_nanos();
                        drop(guard);
                        acquisitions.push(json!({"start_ns":start,"acquired_ns":acquired}));
                    }
                }
            }
            let queries: Vec<_> = readers
                .into_iter()
                .map(|reader| reader.join().unwrap())
                .collect();
            let (published, interval) = writer.join().unwrap();
            (published, interval, queries, acquisitions, probe_complete)
        });
    assert_eq!(
        backend
            .operation(&owner, published.operation_id)
            .unwrap()
            .result,
        published.result
    );
    let replay = backend.commit_workspace(&owner, commit).unwrap();
    assert_eq!(replay.result, published.result);
    assert_eq!(replay.sequence, published.sequence);
    assert_eq!(replay.commit_id, published.commit_id);
    assert_eq!(replay.request_digest, published.request_digest);
    assert_eq!(replay.commit_state, published.commit_state);
    assert_eq!(
        replay.protection_at_publish,
        published.protection_at_publish
    );
    let current = backend
        .lookup_current(&owner, &repository.main_branch_id, &path)
        .unwrap()
        .unwrap();
    assert_eq!(current.file_id, original.file_id);
    assert_eq!(current.size, BODY_BYTES as u64);
    let mut content = backend
        .content(&owner, &current.version_id)
        .unwrap()
        .fully_verified()
        .unwrap();
    let mut actual = Vec::new();
    content.file.read_to_end(&mut actual).unwrap();
    assert!(actual == body, "complete 64 MiB content differs");
    drop(content);
    drop(backend);
    let reopened = EngineBackend::new(Engine::open(&root).unwrap());
    assert_eq!(
        reopened
            .operation(&owner, published.operation_id)
            .unwrap()
            .result,
        published.result
    );
    assert_eq!(
        reopened
            .lookup_current(&owner, &repository.main_branch_id, &path)
            .unwrap()
            .unwrap(),
        current
    );
    let entries = reopened
        .engine
        .lock()
        .unwrap()
        .list_tree(&repository.main_branch_id)
        .unwrap();
    assert_eq!(entries.len(), files);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.path.as_str(), format!("file-{index:06}.txt"));
    }
    println!(
        "\nCONTENTION_EVIDENCE {}",
        json!({"files":files,"body_bytes":BODY_BYTES,
        "writer":writer_interval,"read_kinds":["directory","file_history","operation"],
        "reads":query_samples,"mutex_acquisition_elapsed":acquisitions,
        "probe_complete":probe_complete,"mutex_owner_hold_time_measured":false,
        "network_stack_measured":false,"independent_devices":false})
    );
}

#[test]
#[ignore = "explicit process-isolated release measurement"]
fn contention_500() {
    measure(500);
}
#[test]
#[ignore = "explicit process-isolated release measurement"]
fn contention_1500() {
    measure(1500);
}
#[test]
#[ignore = "explicit process-isolated release measurement"]
fn contention_3000() {
    measure(3000);
}
