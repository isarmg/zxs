//! Finite wire-level generations across publication, lost replies and reopen.
use super::*;
use smb_protocol::client::{Mutation, WriteSequence};
use std::{net::Shutdown, os::unix::fs::MetadataExt};
use storage_contract::{CommitState, CurrentHealth, OperationId, ProtectionAtPublish};

struct Published {
    operation: OperationId,
    path: String,
    body: Vec<u8>,
}

fn value(wire: &mut FrameStream, request: Request<OperationId>) -> io::Result<Value> {
    wire.send(&request, None)?;
    let (response, descriptor) = wire.receive::<Response>()?;
    assert!(descriptor.is_none());
    match response {
        Response::Ok { result } => Ok(result),
        response => panic!("bounded lifecycle request failed: {response:?}"),
    }
}

fn open(wire: &mut FrameStream, path: &str, write_through: bool) -> (OpenResult, File) {
    let (response, fd) = binding_tests::exchange(
        wire,
        Request::Open {
            path: path.into(),
            writable: true,
            create: true,
            truncate: false,
            exclusive: false,
            write_through,
        },
    );
    let Response::Ok { result } = response else {
        panic!("bounded lifecycle open failed: {response:?}")
    };
    (
        json_codec::from_value(result).unwrap(),
        File::from(fd.unwrap()),
    )
}

fn write(
    wire: &mut FrameStream,
    sequence: &mut WriteSequence,
    opened: &OpenResult,
    file: &File,
    bytes: &[u8],
) {
    sequence
        .mutate(
            opened.handle,
            bytes.len() as u64,
            bytes.len() as u64,
            |request| value(wire, request),
            || {
                file.write_all_at(bytes, 0)?;
                file.set_len(bytes.len() as u64)?;
                Ok(((), Mutation::Changed))
            },
        )
        .unwrap();
}

// The published generation changes its workspace after Flush. Match the actual
// descriptor identity, rather than reusing Open's now-obsolete workspace ID.
fn original_operation(shared: &Shared, file: &File) -> OperationId {
    let identity = file.metadata().unwrap();
    let records: Vec<Value> = fs::read_dir(&shared.spool)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".work.json"))
        .map(|path| json_codec::from_slice(&fs::read(path).unwrap()).unwrap())
        .filter(|record: &Value| {
            let source = fs::metadata(record["source_path"].as_str().unwrap()).unwrap();
            source.dev() == identity.dev() && source.ino() == identity.ino()
        })
        .collect();
    assert_eq!(records.len(), 1);
    records[0]["operation_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn reopen(fixture: &Fixture) -> (Arc<Shared>, Arc<EngineBackend>) {
    let backend = Arc::new(EngineBackend::new(
        Engine::open(&fixture.path().join("state")).unwrap(),
    ));
    backend
        .set_management_auth(crate::lock(&fixture.auth).reader())
        .unwrap();
    let spool = fixture.path().join("spool");
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
        max_file_bytes: 4096,
        authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
        backend: backend.clone(),
        username: "alice".into(),
        auth: crate::lock(&fixture.auth).reader(),
        spool,
        file_writers: Mutex::new(BTreeSet::new()),
        namespace_operations: Mutex::new(BTreeSet::new()),
    });
    (shared, backend)
}

fn verify(backend: &EngineBackend, branch: &str, published: &[Published]) {
    backend.test_with_engine_locked(|engine| {
        assert_eq!(engine.state().sequence, 2 + published.len() as u64);
        for (index, saved) in published.iter().enumerate() {
            let record = engine.operation_record(saved.operation).unwrap();
            assert_eq!(record.owner(), Some("alice"));
            let receipt = record.result();
            assert_eq!(receipt.sequence, 3 + index as u64);
            assert_eq!(receipt.commit_state, CommitState::Published);
            assert_eq!(
                receipt.protection_at_publish,
                ProtectionAtPublish::TargetMet
            );
            let tree = engine
                .namespace_tree_at(branch, receipt.commit_id.as_ref().unwrap())
                .unwrap();
            let version = &tree.get(&saved.path).unwrap().version_id;
            assert_eq!(engine.state().placements[version].target_copies, 3);
            let mut bytes = Vec::new();
            engine
                .open_content(version)
                .unwrap()
                .file
                .take(4097)
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(bytes, saved.body);
            // observe() verifies every independent FULL payload and publication
            // proof. Receipt facts remain frozen; health is a current check.
            assert_eq!(
                engine.prepare_operation_health(record).observe().health(),
                Some(CurrentHealth::Healthy)
            );
        }
        for path in ["report.txt", "second.txt"] {
            if let Some(saved) = published.iter().rev().find(|saved| saved.path == path) {
                let tree = engine.namespace_tree(branch).unwrap();
                let mut bytes = Vec::new();
                engine
                    .open_content(&tree.get(path).unwrap().version_id)
                    .unwrap()
                    .file
                    .take(4097)
                    .read_to_end(&mut bytes)
                    .unwrap();
                assert_eq!(bytes, saved.body);
            }
        }
    });
}

#[test]
fn sixty_generations_lost_wire_replies_and_six_restarts_keep_original_receipts() {
    let (fixture, shared, backend, branch) = fixture();
    drop(shared);
    drop(backend);
    let mut published = Vec::new();
    for epoch in 0..6 {
        let (shared, backend) = reopen(&fixture);
        verify(&backend, &branch, &published);
        for step in 0..10 {
            let generation = epoch * 10 + step;
            let path = if generation % 2 == 0 {
                "report.txt"
            } else {
                "second.txt"
            };
            let mode = generation % 4;
            let (mut wire, worker, hello) = binding_tests::wire(shared.clone());
            assert!(matches!(hello, Response::Ok { .. }));
            let (opened, file) = open(&mut wire, path, mode == 1);
            let mut sequence = WriteSequence::new(mode == 1);
            let mut body = vec![generation as u8; 512 + generation];
            let operation = original_operation(&shared, &file);
            write(&mut wire, &mut sequence, &opened, &file, &body);
            if mode == 0 {
                sequence
                    .flush(opened.handle, |request| value(&mut wire, request))
                    .unwrap();
                published.push(Published {
                    operation,
                    path: path.into(),
                    body: body.clone(),
                });
                body.extend_from_slice(b"after-flush");
                write(&mut wire, &mut sequence, &opened, &file, &body);
                let operation = original_operation(&shared, &file);
                drop(file);
                value(
                    &mut wire,
                    Request::Close {
                        handle: opened.handle,
                    },
                )
                .unwrap();
                published.push(Published {
                    operation,
                    path: path.into(),
                    body,
                });
            } else {
                drop(file); // Same descriptor lifetime as VFS Close.
                if mode == 2 || mode == 3 {
                    let request = if mode == 2 {
                        Request::<OperationId>::Close {
                            handle: opened.handle,
                        }
                    } else {
                        Request::<OperationId>::Flush {
                            handle: opened.handle,
                        }
                    };
                    // Refuse replies before sending, while keeping the request
                    // direction live. Completion is established by join, not time.
                    wire.0.shutdown(Shutdown::Read).unwrap();
                    wire.send(&request, None).unwrap();
                    wire.0.shutdown(Shutdown::Write).unwrap();
                } else {
                    value(
                        &mut wire,
                        Request::Close {
                            handle: opened.handle,
                        },
                    )
                    .unwrap();
                }
                published.push(Published {
                    operation,
                    path: path.into(),
                    body,
                });
            }
            drop(wire);
            let result = worker.join().unwrap();
            if mode == 2 || mode == 3 {
                let error = result.unwrap_err();
                assert!(
                    matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                    ),
                    "{error:?}"
                );
            } else {
                result.unwrap();
            }
            assert!(shared.file_writers.lock().unwrap().is_empty());
            assert!(shared.namespace_operations.lock().unwrap().is_empty());
            // Reconciliation queries the original identity. No re-publication
            // or replacement operation is issued after an unknown wire reply.
            let saved = published.last().unwrap();
            let observed = backend
                .operation_outcome(&identity(&shared), saved.operation)
                .unwrap();
            assert_eq!(observed.operation_id, saved.operation);
            assert_eq!(observed.commit_state, CommitState::Published);
        }
        verify(&backend, &branch, &published);
        drop(shared);
        drop(backend);
    }
    let (_shared, backend) = reopen(&fixture);
    verify(&backend, &branch, &published);
    assert_eq!(published.len(), 75);
}
