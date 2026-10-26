//! VFS close policy exercised against the actual socket bridge and core backend.
use super::*;
use smb_protocol::client::{Client, RemoteError, close_descriptor};
use std::{
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixListener,
    },
    thread::JoinHandle,
};

#[derive(Clone, Copy)]
enum Failure {
    Upload,
    PendingOperation,
    UnknownOperation,
    Abort,
}

struct CloseFailure {
    inner: Arc<dyn Backend>,
    failure: Failure,
    enabled: AtomicBool,
    uploads: AtomicUsize,
    commits: AtomicUsize,
    aborts: AtomicUsize,
}

impl Backend for CloseFailure {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        if self.enabled.load(Ordering::SeqCst) {
            if matches!(self.failure, Failure::PendingOperation) {
                return Ok(storage_contract::OperationResult {
                    operation_id,
                    kind: "workspace.commit".into(),
                    commit_id: None,
                    history_tree_id: None,
                    branch_id: None,
                    commit_state: storage_contract::CommitState::Prepared,
                    protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                    current_health: storage_contract::CurrentHealth::Healthy,
                    request_digest: "0".repeat(64),
                    sequence: 0,
                    result: json!({}),
                });
            }
            if matches!(self.failure, Failure::UnknownOperation) {
                return Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::RecoveryRequired,
                    "original operation outcome is unavailable",
                ));
            }
        }
        self.inner.operation(identity, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        self.inner.commit_workspace(identity, command)
    }

    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn authorize_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.authorize_current(identity, branch)
    }

    fn read_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        self.inner.read_directory(identity, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        self.inner
            .authorize_workspace_selection(identity, workspace, path)
    }

    fn lookup_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_current(identity, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        if action == crate::BackendAction::AbortWorkspace {
            self.aborts.fetch_add(1, Ordering::SeqCst);
        }
        if self.enabled.load(Ordering::SeqCst)
            && action == crate::BackendAction::AbortWorkspace
            && matches!(self.failure, Failure::Abort)
        {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::WorkspaceBusy,
                "injected abort failure",
            ));
        }
        self.inner.call(action, identity, input)
    }

    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
        if self.enabled.load(Ordering::SeqCst) && matches!(self.failure, Failure::Upload) {
            return Err(ApiError::forbidden());
        }
        self.inner.upload(identity, workspace, item, path, source)
    }

    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.inner.content(identity, version)
    }
}

fn connect(shared: Arc<Shared>) -> (Client, JoinHandle<()>) {
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
    let socket = shared.spool.join(format!("{}.sock", new_id("rpc")));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        run_connection(FrameStream::new(stream).unwrap(), shared).unwrap();
    });
    let client = Client::connect(&socket, "alice", &repository).unwrap();
    // The connected sockets stay live; the listener is used only once.
    fs::remove_file(socket).unwrap();
    (client, worker)
}

fn open(client: &Client, path: &str, create: bool) -> (OpenResult, OwnedFd) {
    client
        .open(Request::Open {
            path: path.into(),
            writable: true,
            create,
            truncate: false,
            exclusive: false,
            write_through: false,
        })
        .unwrap()
}

fn source(shared: &Shared, opened: &OpenResult) -> PathBuf {
    fs::read_dir(&shared.spool)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            if !path.to_string_lossy().ends_with(".work.json") {
                return None;
            }
            let record: Value = json_codec::from_slice(&fs::read(path).unwrap()).unwrap();
            (record["workspace_id"].as_str() == opened.workspace_id.as_deref())
                .then(|| PathBuf::from(record["source_path"].as_str().unwrap()))
        })
        .next()
        .expect("writer recovery record missing")
}

fn record(source: &Path) -> Value {
    json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap()
}

fn remote(error: &io::Error) -> &RemoteError {
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<RemoteError>()
        .unwrap()
}

fn close_vfs(client: &Client, opened: &OpenResult, fd: OwnedFd) -> io::Result<()> {
    // This is the same descriptor/lock/request policy called by close_opened in
    // the Samba VFS module, without replacing the RPC or core backend with mocks.
    close_descriptor(opened.handle, fd, &Mutex::new(()), |request| {
        client.call(request).map(|_| ())
    })
}

#[test]
fn failed_vfs_close_detaches_only_its_writer_and_keeps_both_sessions_usable() {
    for failure in [
        Failure::Upload,
        Failure::PendingOperation,
        Failure::UnknownOperation,
        Failure::Abort,
    ] {
        let (_temp, mut shared, _backend, _branch) = fixture();
        let injected = Arc::new(CloseFailure {
            inner: shared.backend.clone(),
            failure,
            enabled: AtomicBool::new(false),
            uploads: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
            aborts: AtomicUsize::new(0),
        });
        Arc::get_mut(&mut shared).unwrap().backend = injected.clone();
        let (client, worker) = connect(shared.clone());
        let (other, other_worker) = connect(shared.clone());
        let (opened, fd) = open(&client, "report.txt", false);
        let (unrelated, unrelated_fd) = open(&client, "other.txt", true);
        let retained = source(&shared, &opened);
        let operation = record(&retained)["operation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        if !matches!(failure, Failure::Abort) {
            File::from(fd.try_clone().unwrap())
                .write_all_at(b"pending", 0)
                .unwrap();
            client
                .call(Request::Dirty {
                    handle: opened.handle,
                })
                .unwrap();
        }
        injected.enabled.store(true, Ordering::SeqCst);
        let before = (
            injected.uploads.load(Ordering::SeqCst),
            injected.commits.load(Ordering::SeqCst),
            injected.aborts.load(Ordering::SeqCst),
        );
        let raw_fd = fd.as_raw_fd();
        let error = close_vfs(&client, &opened, fd).unwrap_err();
        let expected_code = match failure {
            Failure::Upload => "FORBIDDEN",
            Failure::PendingOperation | Failure::UnknownOperation => "RECOVERY_REQUIRED",
            Failure::Abort => "WORKSPACE_BUSY",
        };
        assert_eq!(remote(&error).code.to_string(), expected_code);
        assert_eq!(
            remote(&error).operation_id.as_deref(),
            Some(operation.as_str())
        );
        assert!(
            !fs::read_link(format!("/proc/self/fd/{raw_fd}"))
                .is_ok_and(|target| target == retained),
            "VFS descriptor survived its terminal close"
        );
        let metadata = record(&retained);
        assert_eq!(metadata["operation_id"], operation);
        assert_eq!(metadata["handle_state"], "detached");
        assert_eq!(metadata["detach_error"]["code"], expected_code);
        assert_eq!(metadata["retained_bytes_synced"], true);
        assert_eq!(
            fs::read(&retained).unwrap(),
            if matches!(failure, Failure::Abort) {
                b"old".as_slice()
            } else {
                b"pending".as_slice()
            }
        );
        assert_eq!(injected.commits.load(Ordering::SeqCst), before.1);
        assert_eq!(
            injected.aborts.load(Ordering::SeqCst),
            before.2 + usize::from(matches!(failure, Failure::Abort)),
            "a failed save must not be replaced with an abort"
        );
        if matches!(
            failure,
            Failure::PendingOperation | Failure::UnknownOperation
        ) {
            assert_eq!(injected.uploads.load(Ordering::SeqCst), before.0);
        }
        let repeated = client
            .call(Request::Close {
                handle: opened.handle,
            })
            .unwrap_err();
        assert_eq!(
            remote(&repeated).code,
            smb_protocol::client::RemoteErrorCode::Server(storage_contract::ErrorCode::NotFound)
        );
        assert_eq!(injected.commits.load(Ordering::SeqCst), before.1);
        injected.enabled.store(false, Ordering::SeqCst);
        assert!(
            shared
                .file_writers
                .lock()
                .unwrap()
                .iter()
                .all(|(_, path)| path != "report.txt")
        );
        assert!(
            shared
                .file_writers
                .lock()
                .unwrap()
                .iter()
                .any(|(_, path)| path == "other.txt")
        );

        // Traffic keeps the original connection active. Reopen succeeds now,
        // rather than waiting for the 120-second idle timeout to release locks.
        client
            .call(Request::Stat {
                path: "report.txt".into(),
            })
            .unwrap();
        let (reopened, reopened_fd) = open(&client, "report.txt", false);
        close_vfs(&client, &reopened, reopened_fd).unwrap();
        let (reopened, reopened_fd) = open(&other, "report.txt", false);
        close_vfs(&other, &reopened, reopened_fd).unwrap();
        File::from(unrelated_fd.try_clone().unwrap())
            .write_all_at(b"healthy", 0)
            .unwrap();
        client
            .call(Request::Dirty {
                handle: unrelated.handle,
            })
            .unwrap();
        close_vfs(&client, &unrelated, unrelated_fd).unwrap();
        assert!(retained.is_file());
        assert_eq!(record(&retained)["operation_id"], operation);
        drop(client);
        drop(other);
        worker.join().unwrap();
        other_worker.join().unwrap();
    }
}

#[test]
fn lost_close_reply_retains_original_published_operation_without_resubmitting() {
    let (_temp, shared, backend, _branch) = fixture();
    let (client, worker) = connect(shared.clone());
    let (opened, fd) = open(&client, "report.txt", false);
    File::from(fd.try_clone().unwrap())
        .write_all_at(b"new", 0)
        .unwrap();
    client
        .call(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap();
    let retained = source(&shared, &opened);
    let operation = record(&retained)["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    backend.fail.store(true, Ordering::SeqCst);
    let error = close_vfs(&client, &opened, fd).unwrap_err();
    assert_eq!(
        remote(&error).code,
        smb_protocol::client::RemoteErrorCode::Server(storage_contract::ErrorCode::InjectedFailure)
    );
    assert_eq!(
        remote(&error).operation_id.as_deref(),
        Some(operation.as_str())
    );
    let uploads = backend.uploads.load(Ordering::SeqCst);
    let result = backend
        .call(
            crate::BackendAction::Operation,
            &identity(&shared),
            json!({"operation_id": operation}),
        )
        .unwrap();
    assert_eq!(result["commit_state"], "PUBLISHED");
    assert_eq!(fs::read(&retained).unwrap(), b"new");
    assert_eq!(record(&retained)["handle_state"], "detached");
    let (next, next_fd) = open(&client, "report.txt", false);
    close_vfs(&client, &next, next_fd).unwrap();
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    assert!(retained.is_file());
    drop(client);
    worker.join().unwrap();
}

#[test]
fn poisoned_vfs_lock_detaches_without_publishing_or_discarding_unannounced_bytes() {
    let (_temp, shared, backend, _branch) = fixture();
    let (client, worker) = connect(shared.clone());
    let (opened, fd) = open(&client, "report.txt", false);
    let retained = source(&shared, &opened);
    File::from(fd.try_clone().unwrap())
        .write_all_at(b"unsent", 0)
        .unwrap();
    let lock = Mutex::new(());
    let _ = std::panic::catch_unwind(|| {
        let _guard = lock.lock().unwrap();
        panic!("injected VFS worker failure");
    });
    let uploads = backend.uploads.load(Ordering::SeqCst);
    assert!(
        close_descriptor(opened.handle, fd, &lock, |request| {
            assert!(matches!(request, Request::Detach { .. }));
            client.call(request).map(|_| ())
        })
        .is_err()
    );
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    assert_eq!(fs::read(&retained).unwrap(), b"unsent");
    let metadata = record(&retained);
    assert_eq!(metadata["handle_state"], "detached");
    assert_eq!(metadata["dirty"], false);
    assert_eq!(metadata["descriptor_changes_unconfirmed"], true);
    let (next, next_fd) = open(&client, "report.txt", false);
    close_vfs(&client, &next, next_fd).unwrap();
    assert!(retained.is_file());
    drop(client);
    worker.join().unwrap();
}
