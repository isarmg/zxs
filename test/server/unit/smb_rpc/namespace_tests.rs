//! Namespace kind admission and uncertain publication over the real RPC bridge.
use super::*;
use std::{os::unix::net::UnixStream, thread::JoinHandle};

fn connect(shared: Arc<Shared>) -> (FrameStream, JoinHandle<()>) {
    let repository = shared
        .backend
        .call(
            crate::BackendAction::Repositories,
            &identity(&shared),
            json!({}),
        )
        .unwrap()["repositories"][0]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // No listening filesystem socket or external service is needed. This is
    // the same framed protocol on two real, connected Unix descriptors.
    let (client, server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        run_connection(FrameStream::new(server).unwrap(), shared).unwrap();
    });
    let mut client = FrameStream::new(client).unwrap();
    call(
        &mut client,
        Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: "alice".into(),
            repository_id: repository,
        },
    )
    .unwrap();
    (client, worker)
}

fn call(client: &mut FrameStream, request: Request) -> Result<Value, Response> {
    client.send(&request, None).unwrap();
    let (response, descriptor) = client.receive::<Response>().unwrap();
    assert!(descriptor.is_none());
    match response {
        Response::Ok { result } => Ok(result),
        response => Err(response),
    }
}

#[test]
fn unlink_and_rmdir_preserve_targets_of_the_wrong_kind() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    session
        .dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: "mkdir".into(),
            path: "empty".into(),
            destination: None,
            replace: false,
        })
        .unwrap();
    for (kind, path, expected) in [
        (
            "rmdir",
            "report.txt",
            storage_contract::ErrorCode::NotDirectory,
        ),
        ("unlink", "empty", storage_contract::ErrorCode::IsDirectory),
    ] {
        let error = session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind: kind.into(),
                path: path.into(),
                destination: None,
                replace: false,
            })
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert!(session.find(path).unwrap().is_some());
        assert!(session.writer_locks.is_empty());
    }
}

struct NamespaceFailure {
    inner: Arc<dyn Backend>,
    publish: bool,
    spool: PathBuf,
    commits: Mutex<Vec<Value>>,
    aborts: AtomicUsize,
}
impl Backend for NamespaceFailure {
    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        if action == crate::BackendAction::AbortWorkspace {
            self.aborts.fetch_add(1, Ordering::SeqCst);
        }
        if action == crate::BackendAction::CommitWorkspaceInternal {
            let path = self.spool.join(format!(
                "{}.namespace.json",
                input["operation_id"].as_str().unwrap()
            ));
            let record: Value = json_codec::from_slice(
                &fs::read(&path).expect("recovery record must precede submission"),
            )
            .unwrap();
            assert_eq!(record["commit_request"], input);
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
            self.commits.lock().unwrap().push(input.clone());
            if self.publish {
                self.inner.call(action, identity, input)?;
            }
            return Err(ApiError::new(
                503,
                storage_contract::ErrorCode::InjectedFailure,
                "injected uncertain publication outcome",
            ));
        }
        self.inner.call(action, identity, input)
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &str,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.inner.content(identity, version)
    }
}

#[test]
fn namespace_submission_errors_retain_original_operation_without_abort() {
    for publish in [false, true] {
        let (_temp, original, _backend, branch) = fixture();
        let backend = Arc::new(NamespaceFailure {
            inner: original.backend.clone(),
            publish,
            spool: original.spool.clone(),
            commits: Mutex::new(Vec::new()),
            aborts: AtomicUsize::new(0),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: original.auth.clone(),
            spool: original.spool.clone(),
            budget: original.budget.clone(),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: Mutex::new(0),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let (mut client, worker) = connect(shared.clone());
        let error = call(
            &mut client,
            Request::Namespace {
                operation_id: new_id("op"),
                kind: "rename".into(),
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            },
        )
        .unwrap_err();
        let Response::Error {
            code,
            operation_id,
            retryable,
            ..
        } = error
        else {
            panic!("expected an error response");
        };
        assert_eq!(code, "INJECTED_FAILURE");
        assert!(
            !retryable,
            "namespace must not suggest replaying an unknown outcome"
        );
        let commit = backend.commits.lock().unwrap()[0].clone();
        assert_eq!(operation_id.as_deref(), commit["operation_id"].as_str());
        assert_eq!(
            backend.aborts.load(Ordering::SeqCst),
            0,
            "uncertain submission must never abort"
        );
        let records = fs::read_dir(&shared.spool)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.to_string_lossy()
                    .ends_with(".namespace.json")
                    .then_some(path)
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        let record: Value = json_codec::from_slice(&fs::read(&records[0]).unwrap()).unwrap();
        assert_eq!(record["commit_request"], commit);
        assert_eq!(record["principal"], "alice");
        assert_eq!(record["branch_id"], branch);
        assert_eq!(record["path"], "report.txt");
        assert_eq!(record["destination"], "renamed.txt");
        assert!(shared.file_writers.lock().unwrap().is_empty());
        let original_exists = call(
            &mut client,
            Request::Stat {
                path: "report.txt".into(),
            },
        )
        .is_ok();
        assert_eq!(original_exists, !publish);
        if publish {
            let operation = backend
                .inner
                .call(
                    crate::BackendAction::Operation,
                    &identity(&shared),
                    json!({"operation_id":operation_id}),
                )
                .unwrap();
            assert_eq!(operation["commit_state"], "PUBLISHED");
        }
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn uncertain_namespace_dispatch_never_aborts_its_workspace() {
    for publish in [false, true] {
        let (_temp, original, _backend, branch) = fixture();
        let backend = Arc::new(NamespaceFailure {
            inner: original.backend.clone(),
            publish,
            spool: original.spool.clone(),
            commits: Mutex::new(Vec::new()),
            aborts: AtomicUsize::new(0),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: original.auth.clone(),
            spool: original.spool.clone(),
            budget: original.budget.clone(),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: Mutex::new(0),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let mut session = session(shared, &branch);
        let error = session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind: "rename".into(),
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            })
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
        let commit = backend.commits.lock().unwrap()[0].clone();
        assert_eq!(
            error.operation_id.as_deref(),
            commit["operation_id"].as_str()
        );
        let record_path = session.shared.spool.join(format!(
            "{}.namespace.json",
            error.operation_id.as_deref().unwrap()
        ));
        let record: Value = json_codec::from_slice(&fs::read(record_path).unwrap()).unwrap();
        assert_eq!(record["commit_request"], commit);
        assert_eq!(record["path"], "report.txt");
        assert_eq!(record["destination"], "renamed.txt");
        assert_eq!(record["principal"], "alice");
        assert_eq!(record["branch_id"], branch);
        assert!(session.writer_locks.is_empty());
        assert_eq!(session.find("renamed.txt").unwrap().is_some(), publish);
        if publish {
            let operation = backend
                .inner
                .call(
                    crate::BackendAction::Operation,
                    &session.identity,
                    json!({"operation_id":error.operation_id}),
                )
                .unwrap();
            assert_eq!(operation["commit_state"], "PUBLISHED");
        }
        assert_eq!(
            backend.aborts.load(Ordering::SeqCst),
            0,
            "an attempted commit is not proven unpublished by an error response"
        );
    }
}

#[test]
fn legacy_untyped_delete_cannot_remove_files_or_directories() {
    // The old unlinkat callback sent exactly this request for both flags=0 and
    // AT_REMOVEDIR. Reject that ambiguous wire action rather than retaining an
    // old compatibility path which silently deletes either target kind.
    for path in ["report.txt", "empty"] {
        let (_temp, shared, _backend, branch) = fixture();
        let mut session = session(shared, &branch);
        if path == "empty" {
            session
                .dispatch(Request::Namespace {
                    operation_id: new_id("op"),
                    kind: "mkdir".into(),
                    path: path.into(),
                    destination: None,
                    replace: false,
                })
                .unwrap();
        }
        let result = session.dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: "delete".into(),
            path: path.into(),
            destination: None,
            replace: false,
        });
        assert!(
            session.find(path).unwrap().is_some(),
            "ambiguous legacy delete actually removed {path}: {result:?}"
        );
        assert!(result.is_err());
    }
}

#[test]
fn typed_removal_succeeds_and_successful_namespace_records_are_cleaned() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    for (kind, path) in [
        ("mkdir", "empty"),
        ("rmdir", "empty"),
        ("unlink", "report.txt"),
    ] {
        session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind: kind.into(),
                path: path.into(),
                destination: None,
                replace: false,
            })
            .unwrap();
    }
    assert!(session.find("empty").unwrap().is_none());
    assert!(session.find("report.txt").unwrap().is_none());
    assert!(!fs::read_dir(&session.shared.spool).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .to_string_lossy()
            .ends_with(".namespace.json")
    }));
}

#[test]
fn malformed_namespace_requests_are_rejected_before_creating_workspaces() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    let before = backend
        .inner
        .call(
            crate::BackendAction::Tree,
            &session.identity,
            json!({"branch_id":branch}),
        )
        .unwrap();
    for (kind, path, destination, replace) in [
        ("mkdir", "../bad", None, false),
        ("mkdir", "valid", Some("other"), false),
        ("rmdir", "valid", None, true),
        ("rename", "report.txt", None, false),
        ("rename", "report.txt", Some("../bad"), false),
    ] {
        assert!(
            session
                .dispatch(Request::Namespace {
                    operation_id: new_id("op"),
                    kind: kind.into(),
                    path: path.into(),
                    destination: destination.map(str::to_owned),
                    replace,
                })
                .is_err()
        );
        assert!(session.writer_locks.is_empty());
    }
    let after = backend
        .inner
        .call(
            crate::BackendAction::Tree,
            &session.identity,
            json!({"branch_id":branch}),
        )
        .unwrap();
    assert_eq!(after, before);
    assert!(
        backend.change_before_workspace.load(Ordering::SeqCst),
        "malformed input reached workspace creation"
    );
}

#[test]
fn write_through_dirty_error_identifies_the_new_submitted_generation() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let (opened, file) = session
        .open("report.txt".into(), true, false, false, false, true)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(file.unwrap());
    file.write_all_at(b"one", 0).unwrap();
    session
        .dispatch(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap();
    let first = match &session.handles[&opened.handle] {
        Handle::Write(writer) => writer.operation_id.clone(),
        _ => unreachable!(),
    };
    backend.fail.store(true, Ordering::SeqCst);
    file.write_all_at(b"two", 0).unwrap();
    let error = session
        .dispatch(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    let current = match &session.handles[&opened.handle] {
        Handle::Write(writer) => writer.operation_id.clone(),
        _ => unreachable!(),
    };
    assert_ne!(current, first);
    assert_eq!(error.operation_id.as_deref(), Some(current.as_str()));
    let operation = backend
        .inner
        .call(
            crate::BackendAction::Operation,
            &session.identity,
            json!({"operation_id":current}),
        )
        .unwrap();
    assert_eq!(operation["commit_state"], "PUBLISHED");
}

struct PreparationFailure {
    inner: Arc<dyn Backend>,
    failure: Option<crate::BackendAction>,
    abort_fails: bool,
    aborts: AtomicUsize,
    commits: AtomicUsize,
}
impl Backend for PreparationFailure {
    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        if action == crate::BackendAction::CommitWorkspaceInternal {
            self.commits.fetch_add(1, Ordering::SeqCst);
        }
        if action == crate::BackendAction::AbortWorkspace {
            self.aborts.fetch_add(1, Ordering::SeqCst);
            if self.abort_fails {
                return Err(ApiError::new(
                    503,
                    storage_contract::ErrorCode::InjectedFailure,
                    "injected abort failure",
                ));
            }
        }
        if Some(action) == self.failure {
            return Err(ApiError::new(
                503,
                storage_contract::ErrorCode::InvalidRequest,
                "injected staging failure",
            ));
        }
        self.inner.call(action, identity, input)
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &str,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.inner.content(identity, version)
    }
}

#[test]
fn preparation_and_record_sync_failures_keep_unresolved_recovery_evidence() {
    for (failure, directory_sync) in [
        (Some(crate::BackendAction::Tree), false),
        (Some(crate::BackendAction::Namespace), false),
        (None, false),
        (None, true),
    ] {
        for abort_fails in [false, true] {
            let (_temp, original, _, branch) = fixture();
            let backend = Arc::new(PreparationFailure {
                inner: original.backend.clone(),
                failure,
                abort_fails,
                aborts: AtomicUsize::new(0),
                commits: AtomicUsize::new(0),
            });
            let shared = Arc::new(Shared {
                backend: backend.clone(),
                username: original.username.clone(),
                auth: original.auth.clone(),
                spool: original.spool.clone(),
                budget: original.budget.clone(),
                max_file_bytes: original.max_file_bytes,
                authenticated_sessions: Mutex::new(0),
                file_writers: Mutex::new(BTreeSet::new()),
                namespace_operations: Mutex::new(BTreeSet::new()),
            });
            let mut session = session(shared, &branch);
            let operation_id = new_id("op");
            if failure.is_none() {
                super::super::namespace::fail_next_record_sync(directory_sync);
            }
            let error = session
                .dispatch(Request::Namespace {
                    operation_id: operation_id.clone(),
                    kind: "mkdir".into(),
                    path: "new".into(),
                    destination: None,
                    replace: false,
                })
                .unwrap_err();
            assert_eq!(
                error.code,
                if failure.is_none() {
                    storage_contract::ErrorCode::IoError
                } else {
                    storage_contract::ErrorCode::InvalidRequest
                }
            );
            assert_eq!(error.operation_id.as_deref(), Some(operation_id.as_str()));
            assert_eq!(backend.commits.load(Ordering::SeqCst), 0);
            assert_eq!(backend.aborts.load(Ordering::SeqCst), 1);
            let record_path = session
                .shared
                .spool
                .join(format!("{operation_id}.namespace.json"));
            assert_eq!(
                record_path.exists(),
                abort_fails || failure.is_none(),
                "an unresolved abort or failed record sync must retain evidence"
            );
            if record_path.exists() {
                let record: Value =
                    json_codec::from_slice(&fs::read(record_path).unwrap()).unwrap();
                assert_eq!(record["operation_id"], operation_id);
                assert_eq!(record["path"], "new");
                assert!(
                    record["commit_request"]["workspace_id"]
                        .as_str()
                        .unwrap()
                        .starts_with("ws_")
                );
            }
            assert!(session.writer_locks.is_empty());
            assert!(
                backend
                    .inner
                    .call(
                        crate::BackendAction::Operation,
                        &session.identity,
                        json!({"operation_id":operation_id})
                    )
                    .is_err(),
                "preparation failure must not be presented as publication"
            );
        }
    }
}

#[test]
fn dropped_successful_rpc_frame_keeps_the_client_allocated_operation_queryable() {
    let (_temp, shared, backend, _) = fixture();
    let (mut upstream, worker) = connect(shared.clone());
    let (client, proxy) = UnixStream::pair().unwrap();
    let proxy = std::thread::spawn(move || {
        let mut proxy = FrameStream::new(proxy).unwrap();
        let (request, descriptor) = proxy.receive::<Request>().unwrap();
        assert!(descriptor.is_none());
        upstream.send(&request, None).unwrap();
        let (response, descriptor) = upstream.receive::<Response>().unwrap();
        assert!(descriptor.is_none());
        assert!(
            matches!(response, Response::Ok { .. }),
            "core must have actually succeeded before dropping the frame"
        );
        // Deliberately discard the actual successful RPC frame and disconnect.
    });
    let mut client = FrameStream::new(client).unwrap();
    let operation_id = new_id("op");
    client
        .send(
            &Request::Namespace {
                operation_id: operation_id.clone(),
                kind: "rename".into(),
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        client.receive::<Response>().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    proxy.join().unwrap();
    worker.join().unwrap();
    let result = backend
        .inner
        .call(
            crate::BackendAction::Operation,
            &identity(&shared),
            json!({"operation_id":operation_id}),
        )
        .unwrap();
    assert_eq!(result["commit_state"], "PUBLISHED");
}

#[test]
fn namespace_identity_validation_and_replay_admission_precede_workspace_creation() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared.clone(), &branch);
    let request = |operation_id: String| Request::Namespace {
        operation_id,
        kind: "mkdir".into(),
        path: "new".into(),
        destination: None,
        replace: false,
    };
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    for operation_id in [
        "../../outside",
        "op_short",
        "ws_0123456789abcdef0123456789abcdef",
        "op_0123456789ABCDEF0123456789ABCDEF",
    ] {
        let error = session.dispatch(request(operation_id.into())).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(error.operation_id.is_none());
    }
    let retained_operation = new_id("op");
    let record = shared
        .spool
        .join(format!("{retained_operation}.namespace.json"));
    fs::write(&record, b"existing evidence must not be altered").unwrap();
    assert_eq!(
        session
            .dispatch(request(retained_operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        fs::read(record).unwrap(),
        b"existing evidence must not be altered"
    );
    let active_operation = new_id("op");
    shared
        .namespace_operations
        .lock()
        .unwrap()
        .insert(active_operation.clone());
    assert_eq!(
        session
            .dispatch(request(active_operation.clone()))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert!(
        shared
            .namespace_operations
            .lock()
            .unwrap()
            .remove(&active_operation)
    );
    assert!(
        backend
            .change_before_workspace
            .swap(false, Ordering::SeqCst),
        "rejected request created a workspace"
    );
    assert!(session.writer_locks.is_empty());
    assert!(shared.namespace_operations.lock().unwrap().is_empty());

    let published_operation = new_id("op");
    session
        .dispatch(request(published_operation.clone()))
        .unwrap();
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    assert_eq!(
        session
            .dispatch(request(published_operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::OperationIdConflict
    );
    assert!(backend.change_before_workspace.load(Ordering::SeqCst));
    assert!(shared.namespace_operations.lock().unwrap().is_empty());
}
