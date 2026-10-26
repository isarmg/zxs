//! The authorization decorator must preserve backend overrides, not trait defaults.
use super::*;
use std::{
    os::fd::AsRawFd,
    sync::atomic::{AtomicBool, Ordering},
};

struct Recorded {
    method: &'static str,
    identity: Option<Identity>,
    arguments: Value,
}

struct RecordingBackend {
    calls: Mutex<Vec<Recorded>>,
    fail: AtomicBool,
    file: std::fs::File,
}

fn sentinel_error(method: &str) -> ApiError {
    ApiError {
        status: ::http::StatusCode::CONFLICT,
        source: None,
        code: storage_contract::ErrorCode::InjectedFailure,
        message: method.into(),
        retryable: true,
        request_id: format!("req_{method}"),
    }
}

impl RecordingBackend {
    fn record(
        &self,
        method: &'static str,
        identity: Option<&Identity>,
        arguments: Value,
    ) -> Result<Value, ApiError> {
        self.calls.lock().unwrap().push(Recorded {
            method,
            identity: identity.cloned(),
            arguments,
        });
        if self.fail.load(Ordering::SeqCst) {
            Err(sentinel_error(method))
        } else {
            Ok(json!({"returned_by": method}))
        }
    }

    fn read(
        &self,
        method: &'static str,
        identity: &Identity,
        arguments: Value,
    ) -> Result<Content, ApiError> {
        self.record(method, Some(identity), arguments)?;
        Ok(Content {
            file: self.file.try_clone().unwrap(),
            fallback_files: vec![],
            checksum_manifest: None,
            size: self.file.metadata().unwrap().len(),
            hash: method.into(),
            lease: Box::new(()),
            download_name: Some("sentinel.bin".into()),
        })
    }
}

fn upload_arguments(workspace: &str, item: &str, path: &str, source: &UploadSource) -> Value {
    json!({"workspace": workspace, "item": item, "path": path,
        "fd": source.file.as_raw_fd(), "size": source.size, "hash": source.hash})
}

impl Backend for RecordingBackend {
    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.record(
            "snapshot_at",
            Some(identity),
            json!({"branch":branch,"commit":commit}),
        )?;
        Ok(Arc::new(storage_contract::Commit {
            commit_id: commit.into(),
            history_tree_id: "history".into(),
            kind: "snapshot".into(),
            parent_commit_ids: vec![],
            tree_id: "tree".into(),
            sequence: 1,
            operation_id: "operation".into(),
            author: "author".into(),
            message: "snapshot fixture".into(),
            entries: vec![],
            namespace_deletions: None,
            pending_file_versions: Default::default(),
            provenance: None,
        }))
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.record(
            "lookup_at",
            Some(identity),
            json!({"branch":branch,"commit":commit,"path":path.as_str(),"key":path.key()}),
        )?;
        Ok(None)
    }
    fn setup_operation(
        &self,
        action: &'static str,
        input: Value,
        bootstrap: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        self.record(
            "setup_operation",
            identity,
            json!({"action": action, "input": input, "bootstrap": bootstrap}),
        )
    }
    fn start_management(&self, _: Arc<Mutex<AuthStore>>) {
        self.record("start_management", None, Value::Null).unwrap();
    }
    fn stop_management(&self) {
        self.record("stop_management", None, Value::Null).unwrap();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.record("drain_management", None, Value::Null)
            .map(|_| ())
    }
    fn management_status(&self) -> Value {
        self.record("management_status", None, Value::Null).unwrap()
    }
    fn retry_storage(&self, identity: &Identity) -> Result<Value, ApiError> {
        self.record("retry_storage", Some(identity), Value::Null)
    }
    fn call(
        &self,
        action: BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.record(
            "call",
            Some(identity),
            json!({"action": action.as_str(), "input": input}),
        )
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &UploadSource,
    ) -> Result<Value, ApiError> {
        self.record(
            "upload",
            Some(identity),
            upload_arguments(workspace, item, path, source),
        )
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &UploadSource,
        generation: u64,
    ) -> Result<Value, ApiError> {
        let mut arguments = upload_arguments(workspace, item, path, source);
        arguments["generation"] = generation.into();
        self.record("upload_generation", Some(identity), arguments)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.read("content", identity, json!({"version": version}))
    }
    fn range_content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.read("range_content", identity, json!({"version": version}))
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.read(
            "workspace_content",
            identity,
            json!({"workspace": workspace, "item": item}),
        )
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.record("checksums", Some(identity), json!({"version": version}))?;
        Ok(vec![19, 23])
    }
    fn observe_disks(&self, _: &InternalDiskObservation) -> Result<Value, ApiError> {
        self.record("observe_disks", None, Value::Null)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    backend: AuthorizedBackend,
    probe: Arc<RecordingBackend>,
    identity: Identity,
    token: String,
    source: UploadSource,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::tests::private_tempdir();
        let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let token = auth.login("admin", "a-long-test-password").unwrap().token;
        let mut identity = auth.authenticate(&token).unwrap();
        identity.smb_binding = Some(auth::SmbBinding {
            repository_id: "repo-bound".into(),
            branch_id: "branch-bound".into(),
            generation: 29,
        });
        let path = root.path().join("body");
        std::fs::write(&path, b"forwarding probe").unwrap();
        let source = UploadSource::from_file(std::fs::File::open(path).unwrap(), 1024).unwrap();
        let probe = Arc::new(RecordingBackend {
            calls: Mutex::new(vec![]),
            fail: AtomicBool::new(false),
            file: source.file.try_clone().unwrap(),
        });
        let backend = AuthorizedBackend {
            backend: probe.clone(),
            auth: Arc::new(Mutex::new(auth)),
        };
        Self {
            _root: root,
            backend,
            probe,
            identity,
            token,
            source,
        }
    }

    fn protected_calls(&self) -> Vec<(&'static str, Result<Value, ApiError>)> {
        let (backend, identity, source) = (&self.backend, &self.identity, &self.source);
        let content = |value: Content| {
            assert_eq!(value.size, source.size);
            assert_eq!(value.download_name.as_deref(), Some("sentinel.bin"));
            json!({"returned_by": value.hash})
        };
        vec![
            (
                "setup_operation",
                backend.setup_operation(
                    "status",
                    json!({"probe": 3}),
                    Some("bootstrap-probe"),
                    Some(identity),
                ),
            ),
            ("retry_storage", backend.retry_storage(identity)),
            (
                "snapshot_at",
                backend
                    .snapshot_at(identity, "branch", "commit")
                    .map(|snapshot| {
                        assert_eq!(snapshot.commit_id, "commit");
                        json!({"returned_by":"snapshot_at"})
                    }),
            ),
            (
                "lookup_at",
                backend
                    .lookup_at(
                        identity,
                        "branch",
                        "commit",
                        &name_policy::LookupPath::parse("File.txt").unwrap(),
                    )
                    .map(|entry| {
                        assert!(entry.is_none());
                        json!({"returned_by":"lookup_at"})
                    }),
            ),
            (
                "call",
                backend.call(BackendAction::Repositories, identity, json!({"probe": 7})),
            ),
            (
                "upload",
                backend.upload(
                    identity,
                    "workspace",
                    "item",
                    &name_policy::PortablePath::parse("path").unwrap(),
                    source,
                ),
            ),
            (
                "upload_generation",
                backend.upload_generation(
                    identity,
                    "workspace",
                    "item",
                    &name_policy::PortablePath::parse("path").unwrap(),
                    source,
                    37,
                ),
            ),
            (
                "content",
                backend.content(identity, "version-content").map(content),
            ),
            (
                "range_content",
                backend
                    .range_content(identity, "version-range")
                    .map(content),
            ),
            (
                "workspace_content",
                backend
                    .workspace_content(identity, "workspace-read", "item-read")
                    .map(content),
            ),
            (
                "checksums",
                backend
                    .checksums(identity, "version-checksums")
                    .map(|bytes| json!(bytes)),
            ),
        ]
    }
}

fn assert_identity(actual: Option<&Identity>, expected: &Identity) {
    let actual = actual.expect("the request identity must be forwarded");
    assert_eq!(actual.principal, expected.principal);
    assert_eq!(actual.admission_session, expected.admission_session);
    assert_eq!(actual.smb_binding, expected.smb_binding);
}

fn assert_error(actual: ApiError, method: &str) {
    let expected = sentinel_error(method);
    assert_eq!(actual.status, expected.status);
    assert_eq!(actual.code, expected.code);
    assert_eq!(actual.message, expected.message);
    assert_eq!(actual.retryable, expected.retryable);
    assert_eq!(actual.request_id, expected.request_id);
}

#[test]
fn protected_overrides_preserve_identity_arguments_results_and_errors() {
    let fixture = Fixture::new();
    let results = fixture.protected_calls();
    for (method, result) in &results {
        let expected = if *method == "checksums" {
            json!([19, 23])
        } else {
            json!({"returned_by": method})
        };
        assert_eq!(result.as_ref().unwrap(), &expected, "{method}");
    }
    let upload = upload_arguments("workspace", "item", "path", &fixture.source);
    let mut generation = upload.clone();
    generation["generation"] = 37.into();
    let arguments = [
        json!({"action": "status", "input": {"probe": 3}, "bootstrap": "bootstrap-probe"}),
        Value::Null,
        json!({"branch":"branch","commit":"commit"}),
        json!({"branch":"branch","commit":"commit","path":"File.txt","key":"file.txt"}),
        json!({"action": BackendAction::Repositories.as_str(), "input": {"probe": 7}}),
        upload,
        generation,
        json!({"version": "version-content"}),
        json!({"version": "version-range"}),
        json!({"workspace": "workspace-read", "item": "item-read"}),
        json!({"version": "version-checksums"}),
    ];
    let calls = std::mem::take(&mut *fixture.probe.calls.lock().unwrap());
    assert_eq!(calls.len(), results.len());
    for ((call, (method, _)), arguments) in calls.iter().zip(&results).zip(arguments) {
        assert_eq!(call.method, *method);
        assert_identity(call.identity.as_ref(), &fixture.identity);
        assert_eq!(call.arguments, arguments, "{method}");
    }
    fixture.probe.fail.store(true, Ordering::SeqCst);
    for (method, result) in fixture.protected_calls() {
        assert_error(result.unwrap_err(), method);
    }
    assert_eq!(fixture.probe.calls.lock().unwrap().len(), results.len());
}

#[test]
fn revoked_sessions_never_reach_protected_backend_methods() {
    let fixture = Fixture::new();
    fixture
        .backend
        .auth
        .lock()
        .unwrap()
        .logout(&fixture.token)
        .unwrap();
    for (method, result) in fixture.protected_calls() {
        let error = result.unwrap_err();
        assert_eq!(error.status, 401, "{method}");
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::Unauthorized,
            "{method}"
        );
    }
    assert!(fixture.probe.calls.lock().unwrap().is_empty());
}

#[test]
fn public_probes_and_bootstrap_setup_keep_their_explicit_exceptions() {
    let fixture = Fixture::new();
    fixture
        .backend
        .auth
        .lock()
        .unwrap()
        .logout(&fixture.token)
        .unwrap();
    for action in [BackendAction::Capabilities, BackendAction::Ready] {
        assert_eq!(
            fixture
                .backend
                .call(action, &fixture.identity, json!({"probe": 11}))
                .unwrap(),
            json!({"returned_by": "call"})
        );
    }
    assert_eq!(
        fixture
            .backend
            .setup_operation("status", json!({"probe": 13}), Some("bootstrap-only"), None)
            .unwrap(),
        json!({"returned_by": "setup_operation"})
    );
    let calls = fixture.probe.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for (call, action) in calls
        .iter()
        .zip([BackendAction::Capabilities, BackendAction::Ready])
    {
        assert_eq!(call.method, "call");
        assert_identity(call.identity.as_ref(), &fixture.identity);
        assert_eq!(
            call.arguments,
            json!({"action": action.as_str(), "input": {"probe": 11}})
        );
    }
    assert_eq!(calls[2].method, "setup_operation");
    assert!(calls[2].identity.is_none());
    assert_eq!(
        calls[2].arguments,
        json!({"action": "status", "input": {"probe": 13}, "bootstrap": "bootstrap-only"})
    );
}

#[test]
fn lifecycle_delegation_preserves_errors_and_private_hook_boundaries() {
    let fixture = Fixture::new();
    fixture
        .backend
        .start_management(fixture.backend.auth.clone());
    assert_eq!(
        fixture
            .backend
            .observe_disks(&InternalDiskObservation(()))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::UnsupportedOperation
    );
    assert!(fixture.probe.calls.lock().unwrap().is_empty());
    fixture.backend.stop_management();
    fixture.backend.drain_management().unwrap();
    assert_eq!(
        fixture.backend.management_status(),
        json!({"returned_by": "management_status"})
    );
    fixture.probe.fail.store(true, Ordering::SeqCst);
    assert_error(
        fixture.backend.drain_management().unwrap_err(),
        "drain_management",
    );
    let calls = fixture.probe.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.method).collect::<Vec<_>>(),
        [
            "stop_management",
            "drain_management",
            "management_status",
            "drain_management"
        ]
    );
    assert!(calls.iter().all(|call| call.identity.is_none()));
}
