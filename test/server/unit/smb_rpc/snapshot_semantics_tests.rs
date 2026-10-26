use super::*;
use smb_protocol::protocol::NamespaceOperation;

#[derive(Clone, Copy, Debug)]
enum ConcurrentNamespace {
    FileToDirectory,
    RemoveChild,
    CreateMissing,
}

fn publish_namespace(
    backend: &dyn Backend,
    identity: &Identity,
    branch: &str,
    changes: &[(&str, &str)],
) -> Result<Value, ApiError> {
    let workspace: Workspace = json_codec::from_value(backend.call(
        crate::BackendAction::CreateWorkspace,
        identity,
        json!({"branch_id":branch}),
    )?)
    .map_err(ApiError::invalid_source)?;
    for (kind, path) in changes {
        backend.call(
            crate::BackendAction::Namespace,
            identity,
            json!({"workspace_id":workspace.workspace_id,"kind":kind,"path":path}),
        )?;
    }
    backend.call(
        crate::BackendAction::CommitWorkspaceInternal,
        identity,
        json!({
            "workspace_id":workspace.workspace_id,"operation_id":new_id("op"),
            "expected_head":workspace.base_head,"fencing_token":workspace.fencing_token.to_string(),
            "message":"concurrent namespace fixture"
        }),
    )
}

struct NamespaceAfterWorkspace {
    inner: Arc<dyn Backend>,
    change: ConcurrentNamespace,
    enabled: AtomicBool,
    captured: Mutex<Option<Workspace>>,
}

impl Backend for NamespaceAfterWorkspace {
    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<Entry>, ApiError> {
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        let result = self.inner.call(action, identity, input)?;
        if action == crate::BackendAction::CreateWorkspace
            && self.enabled.swap(false, Ordering::SeqCst)
        {
            let workspace: Workspace =
                json_codec::from_value(result.clone()).map_err(ApiError::invalid_source)?;
            let changes: &[(&str, &str)] = match self.change {
                ConcurrentNamespace::FileToDirectory => {
                    &[("delete", "report.txt"), ("mkdir", "report.txt")]
                }
                ConcurrentNamespace::RemoveChild => &[("delete", "target/child")],
                ConcurrentNamespace::CreateMissing => &[("mkdir", "missing")],
            };
            publish_namespace(self.inner.as_ref(), identity, &workspace.branch_id, changes)?;
            *self.captured.lock().unwrap() = Some(workspace);
        }
        Ok(result)
    }

    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.inner.upload(identity, workspace, item, path, source)
    }

    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.inner.content(identity, version)
    }
}

#[test]
fn namespace_preconditions_use_workspace_base_after_concurrent_head_changes() {
    for (change, kind, path, destination, expected) in [
        (
            ConcurrentNamespace::FileToDirectory,
            NamespaceOperation::Rmdir,
            "report.txt",
            None,
            storage_contract::ErrorCode::NotDirectory,
        ),
        (
            ConcurrentNamespace::RemoveChild,
            NamespaceOperation::Rmdir,
            "target",
            None,
            storage_contract::ErrorCode::DirectoryNotEmpty,
        ),
        (
            ConcurrentNamespace::RemoveChild,
            NamespaceOperation::Rename,
            "source",
            Some("target"),
            storage_contract::ErrorCode::DirectoryNotEmpty,
        ),
        (
            ConcurrentNamespace::CreateMissing,
            NamespaceOperation::Rmdir,
            "missing",
            None,
            storage_contract::ErrorCode::NotFound,
        ),
    ] {
        let (_temp, original, _backend, branch) = fixture();
        publish_namespace(
            original.backend.as_ref(),
            &identity(&original),
            &branch,
            &[
                ("mkdir", "source"),
                ("mkdir", "target"),
                ("mkdir", "target/child"),
            ],
        )
        .unwrap();
        let backend = Arc::new(NamespaceAfterWorkspace {
            inner: Arc::clone(&original.backend),
            change,
            enabled: AtomicBool::new(true),
            captured: Mutex::new(None),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: Arc::clone(&original.auth),
            spool: original.spool.clone(),
            budget: Arc::clone(&original.budget),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: Mutex::new(0),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let mut session = session(shared, &branch);
        let operation_id = new_id("op");
        let error = session
            .dispatch(Request::Namespace {
                operation_id: operation_id.clone(),
                kind,
                path: path.into(),
                destination: destination.map(str::to_owned),
                replace: destination.is_some(),
            })
            .unwrap_err();
        assert_eq!(error.code, expected, "{change:?} {kind:?}");
        assert_eq!(error.operation_id.as_deref(), Some(operation_id.as_str()));
        let captured = backend.captured.lock().unwrap().take().unwrap();
        let current = session.entries().unwrap();
        match change {
            ConcurrentNamespace::FileToDirectory => assert_eq!(
                current
                    .iter()
                    .find(|entry| entry.path() == "report.txt")
                    .unwrap()
                    .kind,
                storage_contract::NamespaceEntryKind::Directory
            ),
            ConcurrentNamespace::RemoveChild => {
                assert!(current.iter().any(|entry| entry.path() == "target"));
                assert!(!current.iter().any(|entry| entry.path() == "target/child"));
            }
            ConcurrentNamespace::CreateMissing => {
                assert!(current.iter().any(|entry| entry.path() == "missing"));
            }
        }
        let aborted: Workspace = json_codec::from_value(
            backend
                .inner
                .call(
                    crate::BackendAction::Workspace,
                    &session.identity,
                    json!({"workspace_id":captured.workspace_id}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(aborted.state, storage_contract::WorkspaceState::Aborted);
        assert!(session.writer_locks.is_empty());
        assert!(session.shared.file_writers.lock().unwrap().is_empty());
        assert!(
            session
                .shared
                .namespace_operations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(
            !session
                .shared
                .spool
                .join(format!("{operation_id}.namespace.json"))
                .exists()
        );
        assert!(
            backend
                .inner
                .call(
                    crate::BackendAction::Operation,
                    &session.identity,
                    json!({"operation_id":operation_id}),
                )
                .is_err()
        );
    }
}

#[test]
fn publication_comparison_uses_entry_identity_in_renamed_commit() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let (opened, file) = open_file(&mut session, true);
    file.write_all_at(b"new", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        session.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    session.flush(opened.handle).unwrap();
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);

    let workspace: Workspace = json_codec::from_value(
        backend
            .call(
                crate::BackendAction::CreateWorkspace,
                &session.identity,
                json!({"branch_id":branch}),
            )
            .unwrap(),
    )
    .unwrap();
    backend
        .call(
            crate::BackendAction::Namespace,
            &session.identity,
            json!({
                "workspace_id":workspace.workspace_id,"kind":"rename",
                "path":"report.txt","destination":"renamed.txt"
            }),
        )
        .unwrap();
    let renamed_publication = backend.call(
        crate::BackendAction::CommitWorkspaceInternal,
        &session.identity,
        json!({
            "workspace_id":workspace.workspace_id,"operation_id":new_id("op"),
            "expected_head":workspace.base_head,"fencing_token":workspace.fencing_token.to_string(),
            "message":"rename identity fixture"
        }),
    ).unwrap();
    assert!(session.find("report.txt").unwrap().is_none());
    let renamed = session.find("renamed.txt").unwrap().unwrap();
    assert_eq!(renamed.entry_id, opened.entry_id);

    // Compare the actual renamed snapshot while the open descriptor keeps its old path.
    let Handle::Write(writer) = session.handles.get_mut(&opened.handle).unwrap() else {
        panic!("expected write handle");
    };
    super::super::publication::finish_publication(
        &session.shared,
        &session.identity,
        &branch,
        writer,
        renamed_publication,
    )
    .unwrap();
    assert!(writer.committed);
    assert!(!writer.dirty);
    assert_eq!(writer.path.as_str(), "report.txt");
    assert_eq!(writer.item_id, renamed.entry_id);
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    let content = backend
        .content(&session.identity, &renamed.version_id)
        .unwrap();
    let mut bytes = [0; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new");
    session.close(opened.handle, false).unwrap();
    assert!(session.writer_locks.is_empty());
    assert!(session.shared.file_writers.lock().unwrap().is_empty());
}
