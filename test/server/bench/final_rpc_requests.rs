//! Verification-only local Unix RPC screen; not network SMB or physical-disk evidence.
use super::*;
#[path = "final_rpc_read.rs"]
mod read_adapter;
use read_adapter::{directory_paths, read_cycle};

use smb_protocol::client::{Client, Mutation, WriteSequence, close_descriptor};
use std::{os::unix::net::UnixListener, time::Instant};

fn path_for(index: usize) -> String {
    format!("dir-{:03}/file-{index:06}.txt", index % 97)
}

fn status() -> Value {
    let text = fs::read_to_string("/proc/self/status").unwrap();
    let field = |name: &str| -> u64 {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    json!({"vmhwm_kib":field("VmHWM:"),"vmrss_kib":field("VmRSS:")})
}

fn open_request(path: String, writable: bool) -> Request<storage_contract::OperationId> {
    Request::Open {
        path,
        writable,
        create: false,
        truncate: false,
        exclusive: false,
        write_through: false,
    }
}

fn expected_body(index: usize, commits: usize) -> Vec<u8> {
    let mut body = format!("initial body {index}");
    for round in 0..commits - 2 {
        if (round * 37) % 500 == index {
            body = format!("edited body {index} round {round}");
        }
    }
    body.into_bytes()
}

fn list_cycle(client: &Client, directory: usize) -> (u128, Vec<Value>) {
    let request = open_request(format!("dir-{directory:03}"), false);
    let started = Instant::now();
    let (opened, fd) = client.open(request).unwrap();
    let mut pages = Vec::new();
    loop {
        let page = client
            .call(Request::ReadDirectory {
                handle: opened.handle,
                restart: false,
            })
            .unwrap();
        let more = page["more"].as_bool().unwrap();
        pages.push(page);
        if !more {
            break;
        }
    }
    close_descriptor(opened.handle, fd, &Mutex::new(()), |request| {
        client.call(request).map(|_| ())
    })
    .unwrap();
    (started.elapsed().as_nanos(), pages)
}

#[test]
#[ignore = "bounded explicit local-RPC measurement only"]
fn bounded_original_requests() {
    let root = PathBuf::from(std::env::var_os("ZXS_RPC_ROOT").unwrap());
    let commits: usize = std::env::var("ZXS_RPC_COMMITS").unwrap().parse().unwrap();
    assert!(matches!(commits, 22 | 220));
    let started = Instant::now();
    let mut engine = Engine::open(&root.join("state")).unwrap();
    let open_ns = started.elapsed().as_nanos();
    let open_memory = status();
    assert_eq!(engine.state().sequence, commits as u64);
    let warnings = engine.take_recovery_warnings();
    let warnings = json!({
        "checkpoint_load":warnings.checkpoint_load.map(|error| format!("{error:?}")),
        "checkpoint_save":warnings.checkpoint_save.map(|error| format!("{error:?}")),
        "seal_failures":warnings.seals.len(),
        "checkpoint_selected_sequence":"not exposed by existing public API"
    });
    let auth = Arc::new(Mutex::new(
        AuthStore::initialize(&root.join("auth"), "bench", "long-test-password").unwrap(),
    ));
    let backend = Arc::new(EngineBackend::new(engine));
    backend
        .set_management_auth(crate::lock(&auth).reader())
        .unwrap();
    let identity = crate::lock(&auth).reader().sole_identity().unwrap();
    let repository = backend.repositories_snapshot(&identity).unwrap()[0]
        .repository_id
        .clone();
    let spool = root.join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let budget = budget::Budget::recover(
        &spool,
        crate::SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024,
            recovery_reserve_bytes: 1,
        },
    )
    .unwrap();
    let shared = Arc::new(Shared {
        budget,
        max_file_bytes: 4096,
        authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
        backend: backend.clone(),
        username: "bench".into(),
        auth: crate::lock(&auth).reader(),
        spool,
        file_writers: Mutex::new(BTreeSet::new()),
        namespace_operations: Mutex::new(BTreeSet::new()),
    });
    let socket = root.join("rpc.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let serving = shared.clone();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        run_connection(FrameStream::new(stream).unwrap(), serving).unwrap();
    });
    let start = Instant::now();
    let client = Client::connect(&socket, "bench", &repository).unwrap();
    let hello_ns = start.elapsed().as_nanos();
    fs::remove_file(socket).unwrap();
    let first = read_cycle(&client, 0);
    assert_eq!(first.2, expected_body(0, commits));
    let mut read_ns = Vec::with_capacity(1000);
    let mut read_open_ns = Vec::with_capacity(1000);
    let mut list_ns = Vec::with_capacity(1000);
    for round in 0..1000 {
        let index = (round * 101) % 500;
        let (elapsed, opened, body) = read_cycle(&client, index);
        read_ns.push(elapsed);
        read_open_ns.push(opened);
        assert_eq!(body, expected_body(index, commits));
        let directory = round % 97;
        let (elapsed, pages) = list_cycle(&client, directory);
        list_ns.push(elapsed);
        let paths = directory_paths(pages);
        let expected: Vec<_> = (0..500)
            .filter(|index| index % 97 == directory)
            .map(path_for)
            .collect();
        assert_eq!(paths, expected);
    }
    let read_memory = status();
    let mut save_ns = Vec::with_capacity(20);
    let mut responses = Vec::with_capacity(20);
    let batch = Instant::now();
    for round in 0..20 {
        let index = (round * 37) % 500;
        let body = format!("RPC saved body {index} round {round}").into_bytes();
        let request = open_request(path_for(index), true);
        let start = Instant::now();
        let (opened, fd) = client.open(request).unwrap();
        let file = File::from(fd);
        let mut sequence = WriteSequence::new(false);
        sequence
            .mutate(
                opened.handle,
                body.len() as u64,
                body.len() as u64,
                |request| client.call(request),
                || {
                    file.write_all_at(&body, 0)?;
                    file.set_len(body.len() as u64)?;
                    Ok(((), Mutation::Changed))
                },
            )
            .unwrap();
        let mut response = None;
        close_descriptor(opened.handle, file.into(), &Mutex::new(()), |request| {
            response = Some(client.call(request)?);
            Ok(())
        })
        .unwrap();
        save_ns.push(start.elapsed().as_nanos());
        responses.push(response.unwrap());
    }
    let save_batch_ns = batch.elapsed().as_nanos();
    let save_memory = status();
    // Assertions and receipt materialization are outside all request timers.
    let mut published = backend.test_with_engine_locked(|engine| {
        engine
            .state()
            .commits
            .values()
            .filter(|commit| commit.sequence > commits as u64)
            .map(|commit| (commit.sequence, commit.operation_id))
            .collect::<Vec<_>>()
    });
    published.sort_by_key(|(sequence, _)| *sequence);
    assert_eq!(published.len(), 20);
    let mut operations = BTreeSet::new();
    let mut receipts = Vec::with_capacity(20);
    for (round, ((_, operation), response)) in published.into_iter().zip(&responses).enumerate() {
        assert_eq!(response["closed"], true);
        assert_eq!(response["aborted"], false);
        assert!(operations.insert(operation));
        backend.test_with_engine_locked(|engine| {
            let record = engine.operation_record(operation).unwrap();
            assert_eq!(record.owner(), Some("bench"));
            let receipt = record.result();
            assert_eq!(
                receipt.commit_state,
                storage_contract::CommitState::Published
            );
            assert_eq!(receipt.sequence, (commits + round + 1) as u64);
            assert!(receipt.commit_id.is_some());
        });
        receipts.push(json!({"operation_id":operation.to_string()}));
    }
    fs::write(
        root.join("rpc-receipts.json"),
        json_codec::to_vec(&receipts).unwrap(),
    )
    .unwrap();
    drop(client);
    worker.join().unwrap();
    assert!(shared.file_writers.lock().unwrap().is_empty());
    println!(
        "RPC_RESULT {}",
        json!({"schema":"final-local-rpc-v1", "commits_before":commits,
        "open_ns":open_ns,"open_memory":open_memory,"recovery_warnings":warnings,
        "hello_ns":hello_ns,"first_read_ns":first.0,"first_open_ns":first.1,
        "read_ns":read_ns,"read_open_ns":read_open_ns,"list_ns":list_ns,
        "save_ns":save_ns,"save_batch_ns":save_batch_ns,"read_memory":read_memory,
        "save_memory":save_memory,"verified_reads":1001,"verified_lists":1000,"verified_receipts":20,
        "transport":"production local Unix RPC; not network SMB; no peer-accept-loop measurement"})
    );
}

fn create_request(path: String, write_through: bool) -> Request<storage_contract::OperationId> {
    Request::Open {
        path,
        writable: true,
        create: true,
        truncate: false,
        exclusive: true,
        write_through,
    }
}

fn write_tail(client: &Client, opened: &OpenResult, file: &File, body: &[u8], write_through: bool) {
    let mut sequence = WriteSequence::new(write_through);
    if write_through {
        sequence
            .mutate(
                opened.handle,
                body.len() as u64,
                body.len() as u64,
                |request| client.call(request),
                || {
                    file.write_all_at(body, 0)?;
                    Ok(((), Mutation::Changed))
                },
            )
            .unwrap();
    } else {
        for (index, chunk) in body.chunks(64 * 1024).enumerate() {
            let offset = (index * 64 * 1024) as u64;
            sequence
                .mutate(
                    opened.handle,
                    offset + chunk.len() as u64,
                    chunk.len() as u64,
                    |request| client.call(request),
                    || {
                        file.write_all_at(chunk, offset)?;
                        Ok(((), Mutation::Changed))
                    },
                )
                .unwrap();
        }
    }
}

#[test]
#[ignore = "bounded explicit local-RPC measurement only"]
fn bounded_write_tails() {
    let root = PathBuf::from(std::env::var_os("ZXS_RPC_ROOT").unwrap());
    let commits: usize = std::env::var("ZXS_RPC_COMMITS").unwrap().parse().unwrap();
    assert!(matches!(commits, 22 | 220));
    let engine = Engine::open(&root.join("state")).unwrap();
    assert_eq!(engine.state().sequence, (commits + 20) as u64);
    let auth = AuthStore::open(&root.join("auth")).unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    backend.set_management_auth(auth.reader()).unwrap();
    let identity = auth.reader().sole_identity().unwrap();
    let repository = backend.repositories_snapshot(&identity).unwrap()[0]
        .repository_id
        .clone();
    let spool = root.join("spool");
    assert!(spool.is_dir());
    let shared = Arc::new(Shared {
        budget: budget::Budget::recover(
            &spool,
            crate::SpoolLimits {
                bytes_per_account: 16 * 1024 * 1024,
                bytes_total: 64 * 1024 * 1024,
                recovery_reserve_bytes: 1,
            },
        )
        .unwrap(),
        max_file_bytes: 9 * 1024 * 1024,
        authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
        backend: backend.clone(),
        username: "bench".into(),
        auth: auth.reader(),
        spool,
        file_writers: Mutex::new(BTreeSet::new()),
        namespace_operations: Mutex::new(BTreeSet::new()),
    });
    let socket = root.join("rpc.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let serving = shared.clone();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        run_connection(FrameStream::new(stream).unwrap(), serving).unwrap();
    });
    let client = Client::connect(&socket, "bench", &repository).unwrap();
    fs::remove_file(socket).unwrap();
    let mut flush_ns = Vec::with_capacity(40);
    let mut close_ns = Vec::with_capacity(40);
    let mut write_through_ns = Vec::with_capacity(40);
    for round in 0..40 {
        let body = vec![round as u8; 8 * 1024 * 1024];
        let (opened, fd) = client
            .open(create_request(format!("tail-flush-{round:03}.bin"), false))
            .unwrap();
        let file = File::from(fd);
        write_tail(&client, &opened, &file, &body, false);
        let mut sequence = WriteSequence::new(false);
        let started = Instant::now();
        sequence
            .flush(opened.handle, |request| client.call(request))
            .unwrap();
        flush_ns.push(started.elapsed().as_nanos());
        let suffix = vec![255 - round as u8; 64 * 1024];
        sequence
            .mutate(
                opened.handle,
                (body.len() + suffix.len()) as u64,
                suffix.len() as u64,
                |request| client.call(request),
                || {
                    file.write_all_at(&suffix, body.len() as u64)?;
                    Ok(((), Mutation::Changed))
                },
            )
            .unwrap();
        let started = Instant::now();
        close_descriptor(opened.handle, file.into(), &Mutex::new(()), |request| {
            client.call(request).map(|_| ())
        })
        .unwrap();
        close_ns.push(started.elapsed().as_nanos());
        let (opened, fd) = client
            .open(create_request(format!("tail-through-{round:03}.bin"), true))
            .unwrap();
        let file = File::from(fd);
        let started = Instant::now();
        write_tail(&client, &opened, &file, &body, true);
        write_through_ns.push(started.elapsed().as_nanos());
        close_descriptor(opened.handle, file.into(), &Mutex::new(()), |request| {
            client.call(request).map(|_| ())
        })
        .unwrap();
    }
    // Capture original operation identities outside every timer. An independent
    // process reopens this cohort and verifies all 120 receipts and full bodies.
    let mut operations = backend.test_with_engine_locked(|engine| {
        engine
            .state()
            .commits
            .values()
            .filter(|commit| commit.sequence > (commits + 20) as u64)
            .map(|commit| (commit.sequence, commit.operation_id))
            .collect::<Vec<_>>()
    });
    operations.sort_by_key(|(sequence, _)| *sequence);
    assert_eq!(operations.len(), 120);
    fs::write(
        root.join("tails-receipts.json"),
        json_codec::to_vec(&operations).unwrap(),
    )
    .unwrap();
    drop(client);
    worker.join().unwrap();
    assert!(shared.file_writers.lock().unwrap().is_empty());
    println!(
        "RPC_RESULT {}",
        json!({"schema":"final-local-rpc-tails-v1", "commits_before":commits+20,
        "flush_ns":flush_ns,"close_ns":close_ns,"write_through_ns":write_through_ns,
        "body_bytes":8*1024*1024,"append_bytes":64*1024,"published_generations":120,
        "samples_per_metric":40,"p99_caveat":"40 samples: nearest-rank P99 is maximum; no population SLO claim"})
    );
}
