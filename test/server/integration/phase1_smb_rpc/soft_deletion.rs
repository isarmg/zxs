use super::*;
use json_codec::{Value, json};
use storage_contract::history_window::HistoryFileId;

fn unlink(client: &Client, path: &str, operation_id: OperationId) -> Value {
    client
        .call(Request::Namespace {
            kind: NamespaceOperation::Unlink,
            path: path.into(),
            destination: None,
            replace: false,
            operation_id,
        })
        .unwrap()
}

fn branch(
    fixture: &Fixture,
    entry: &Entry,
    name: &str,
) -> Result<storage_contract::OperationResult, storage_server::ApiError> {
    fixture.backend.create_file_branch(
        &fixture.identity(),
        storage_server::CreateFileBranch {
            file_id: entry.file_id.clone(),
            source_version_id: entry.version_id.clone(),
            source_node_id: None,
            name: storage_contract::BranchName::parse(name).unwrap(),
            operation_id: OperationId::generate().unwrap(),
        },
    )
}

#[test]
fn public_smb_soft_delete_revokes_cached_reads_and_pages_and_restores_exact_file() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    save(&client, "p/A.txt", b"retained same body", true);
    save(&client, "p/B.txt", b"retained same body", true);
    let repository = fixture.repository();
    let a = fixture.entry(&repository.main_branch_id, "A.txt");
    let b = fixture.entry(&repository.main_branch_id, "B.txt");
    branch(&fixture, &a, "review").unwrap();
    let tagged = fixture.annotate(&repository.repository_id, &a);
    let (reader, _placeholder) = open(&client, "p/A.txt", false, false, false);
    assert_eq!(client.read(reader.handle, 0, &mut [0; 4]).unwrap(), 4);
    let (buffered, _placeholder) = open(&client, "p", false, false, false);
    let operation = OperationId::generate().unwrap();
    unlink(&client, "p/A.txt", operation);
    let file = HistoryFileId::parse(&a.file_id).unwrap();
    let deleted = fixture
        .backend
        .current_file_deletion(&fixture.identity(), &file)
        .unwrap()
        .unwrap();
    assert_eq!(deleted.branch_heads.len(), 2);
    assert!(deleted.parent_node_ids.contains(&tagged));
    assert_eq!(deleted.operation_id, operation);
    assert!(
        client.read(reader.handle, 0, &mut [0; 4]).is_err(),
        "cached content must reauthorize"
    );
    assert!(
        client
            .call(Request::ReadDirectory {
                handle: buffered.handle,
                restart: false
            })
            .is_err(),
        "buffered first page must reauthorize"
    );
    let page: DirectoryPage = json_codec::from_value(
        client
            .call(Request::ReadDirectory {
                handle: buffered.handle,
                restart: true,
            })
            .unwrap(),
    )
    .unwrap();
    assert!(
        !page
            .entries
            .iter()
            .any(|entry| entry.entry.file_id == a.file_id)
    );
    assert!(!directory(&client, "p").contains(&"A.txt".to_owned()));
    assert_eq!(
        branch(&fixture, &a, "frozen").unwrap_err().code,
        ErrorCode::BranchNotActive
    );
    save(&client, "p/B.txt", b"B still edits", false);
    branch(&fixture, &b, "B still branches").unwrap();
    save(&client, "p/C.txt", b"retained same body", true);
    let c = fixture.entry(&repository.main_branch_id, "C.txt");
    assert_ne!(c.file_id, a.file_id);
    branch(&fixture, &c, "C still branches").unwrap();
    let restore_operation = OperationId::generate().unwrap();
    let restore = || storage_server::RestoreDeletedFile {
        file_id: file.clone(),
        deletion_node_id: deleted.node_id.clone(),
        operation_id: restore_operation,
    };
    let first = fixture
        .backend
        .restore_deleted_file(&fixture.identity(), restore())
        .unwrap();
    assert_eq!(
        fixture
            .backend
            .restore_deleted_file(&fixture.identity(), restore())
            .unwrap(),
        first
    );
    assert_eq!(read(&client, "p/A.txt").1, b"retained same body");
    assert_eq!(client.read(reader.handle, 0, &mut [0; 4]).unwrap(), 4);
    assert_eq!(
        fixture.entry(&repository.main_branch_id, "A.txt").file_id,
        a.file_id
    );
    close(&client, reader.handle);
    close(&client, buffered.handle);
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert!(engine.file_deletion(&a.file_id).is_none());
    assert_eq!(
        exported(&engine, root.path(), &a.version_id),
        b"retained same body"
    );
}

pub(super) async fn request(
    app: &http_routing::Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
) -> (http::StatusCode, Value) {
    use service_stack::ServiceExt;
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(http_routing::body::Body::from(
                    json_codec::to_vec(&body).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[test]
fn public_http_deletion_is_discoverable_and_undo_is_exact_authenticated_and_durable() {
    // Independent HTTP acceptance: no socket capability is needed to exercise
    // the real authenticated Router and durable Engine publication.
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let mut engine = Engine::initialize(
        &system,
        Config::development([root.path().join("p")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "deleted.txt", b"kept on disk")
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            operation_id: OperationId::generate().unwrap(),
            message: "HTTP deletion seed".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("deleted.txt")
        .unwrap()
        .clone();
    engine
        .soft_delete_file(storage_engine::DeleteFileRequest {
            target: storage_engine::FileDeletionTarget {
                repository_id: &storage_contract::history_window::HistoryRepositoryId::parse(
                    &repo.repository_id,
                )
                .unwrap(),
                file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
                path: &name_policy::LookupPath::parse("deleted.txt").unwrap(),
            },
            principal: "alice",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    let deleted = engine.file_deletion(&entry.file_id).unwrap().clone();
    assert_eq!(
        exported(&engine, root.path(), &entry.version_id),
        b"kept on disk"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend
            .protect_auth(
                AuthStore::initialize(
                    &root.path().join("state"),
                    "alice",
                    "phase-one-smb-password",
                )
                .unwrap(),
            )
            .unwrap();
        let identity = auth.sole_identity().unwrap();
        let app = storage_server::router(
            backend.clone(),
            auth,
            storage_server::ServerConfig::new(&root.path().join("state")),
        )
        .unwrap();
        let path = format!("/api/files/{}/deletion", entry.file_id);
        assert_eq!(
            request(&app, None, "GET", &path, json!({})).await.0,
            http::StatusCode::UNAUTHORIZED
        );
        let (status, login) = request(
            &app,
            None,
            "POST",
            "/api/auth/login",
            json!({"username":"alice","password":"phase-one-smb-password"}),
        )
        .await;
        assert_eq!(status, http::StatusCode::OK, "{login}");
        let token = login["token"].as_str().unwrap();
        let (status, node) = request(&app, Some(token), "GET", &path, json!({})).await;
        assert_eq!(status, http::StatusCode::OK, "{node}");
        assert_eq!(node["node_id"], deleted.node_id);
        assert!(node.get("version_id").is_none());
        let (status, page) =
            request(&app, Some(token), "GET", "/api/file-deletions", json!({})).await;
        assert_eq!(status, http::StatusCode::OK, "{page}");
        assert_eq!(page["entries"][0]["file_id"], entry.file_id);
        let undo = format!("{path}/undo");
        let operation = OperationId::generate().unwrap();
        let body = json!({"deletion_node_id":deleted.node_id,"operation_id":operation});
        let (status, error) = request(
            &app,
            Some(token),
            "POST",
            &undo,
            json!({"deletion_node_id":deleted.node_id,"operation_id":operation,"force":true}),
        )
        .await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST, "{error}");
        assert!(
            backend
                .current_file_deletion(&identity, &HistoryFileId::parse(&entry.file_id).unwrap())
                .unwrap()
                .is_some()
        );
        let first = request(&app, Some(token), "POST", &undo, body.clone()).await;
        assert_eq!(first.0, http::StatusCode::OK, "{}", first.1);
        assert_eq!(request(&app, Some(token), "POST", &undo, body.clone()).await, first);
        backend.delete_current_file(
            &identity,
            storage_server::DeleteCurrentFile {
                branch_id: repo.main_branch_id.clone(),
                path: name_policy::LookupPath::parse("deleted.txt").unwrap(),
                expected_entry_id: entry.entry_id.clone(),
                file_id: HistoryFileId::parse(&entry.file_id).unwrap(),
                operation_id: OperationId::generate().unwrap(),
            },
        ).unwrap().expect("native file deletion");
        let (_, second) = request(&app, Some(token), "GET", &path, json!({})).await;
        assert_ne!(second["node_id"], deleted.node_id);
        // Re-reading the original undo receipt must not undo a later deletion.
        assert_eq!(request(&app, Some(token), "POST", &undo, body).await, first);
        assert_eq!(request(&app, Some(token), "GET", &path, json!({})).await.1, second);
        let stale = request(
            &app, Some(token), "POST", &undo,
            json!({"deletion_node_id":deleted.node_id,"operation_id":OperationId::generate().unwrap()}),
        ).await;
        assert_eq!(stale.0, http::StatusCode::NOT_FOUND, "{}", stale.1);
        assert_eq!(request(&app, Some(token), "GET", &path, json!({})).await.1, second);
        let restored = request(
            &app, Some(token), "POST", &undo,
            json!({"deletion_node_id":second["node_id"],"operation_id":OperationId::generate().unwrap()}),
        ).await;
        assert_eq!(restored.0, http::StatusCode::OK, "{}", restored.1);
        assert_eq!(
            request(&app, Some(token), "GET", &path, json!({})).await.1,
            Value::Null
        );
        let cursor = format!(
            "/api/file-deletions?after={}&snapshot={}",
            entry.file_id,
            page["snapshot"].as_str().unwrap()
        );
        assert_eq!(
            request(&app, Some(token), "GET", &cursor, json!({}))
                .await
                .0,
            http::StatusCode::CONFLICT
        );
        assert!(
            backend
                .tree_snapshot(
                    &identity,
                    ReadTree {
                        branch: &repo.main_branch_id,
                        commit: None
                    }
                )
                .unwrap()
                .entries
                .iter()
                .any(|value| value.file_id == entry.file_id)
        );
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    let engine = Engine::open(&root.path().join("state")).unwrap();
    assert!(engine.file_deletion(&entry.file_id).is_none());
    assert!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("deleted.txt")
            .is_some()
    );
}

#[test]
fn public_backend_soft_delete_preserves_frontier_and_freezes_only_deleted_identity() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let mut engine = Engine::initialize(
        &system,
        Config::development([root.path().join("p")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "A.txt", b"retained A")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "B.txt", b"retained A")
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            operation_id: OperationId::generate().unwrap(),
            message: "adapter seed".into(),
            request_digest: None,
        })
        .unwrap();
    let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
    let a = tree.get("A.txt").unwrap().clone();
    let b = tree.get("B.txt").unwrap().clone();
    let review = engine
        .create_file_branch(
            &a.file_id,
            &a.version_id,
            &storage_contract::BranchName::parse("review").unwrap(),
            "alice",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = engine.file_branch_head(&review).unwrap();
    let tagged = engine
        .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
            file_id: &a.file_id,
            targets: &[storage_contract::FileTagTarget {
                branch_id: review.clone(),
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["retained-head".into()],
            remove: &[],
            location: None,
            principal: "alice",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let retained_note = storage_contract::Note::parse("before deletion".into()).unwrap();
    for version in [None, Some(a.version_id.as_str())] {
        engine
            .set_file_note(
                &a.file_id,
                version,
                &retained_note,
                "alice",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    engine
        .set_file_node_note(
            &a.file_id,
            &tagged,
            &retained_note,
            "alice",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "alice", "phase-one-smb-password").unwrap())
        .unwrap();
    let management = auth.sole_identity().unwrap();
    let mut smb = management.clone();
    smb.smb_binding = Some(storage_server::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation,
    });
    let file = HistoryFileId::parse(&a.file_id).unwrap();
    let command = |entry: String| storage_server::DeleteCurrentFile {
        branch_id: repo.main_branch_id.clone(),
        path: name_policy::LookupPath::parse("a.TXT").unwrap(),
        expected_entry_id: entry,
        file_id: file.clone(),
        operation_id: OperationId::generate().unwrap(),
    };
    assert!(
        backend
            .delete_current_file(&smb, command(b.entry_id.clone()))
            .is_err()
    );
    assert!(
        backend
            .current_file_deletion(&management, &file)
            .unwrap()
            .is_none()
    );
    let listed_before = backend
        .file_deletions(
            &management,
            storage_server::ReadFileDeletions::parse(Default::default()).unwrap(),
        )
        .unwrap()
        .snapshot;
    backend
        .authorize_listing_snapshot(&smb, &listed_before)
        .unwrap();
    backend
        .delete_current_file(&smb, command(a.entry_id.clone()))
        .unwrap()
        .expect("native file deletion");
    assert_eq!(
        backend
            .authorize_listing_snapshot(&smb, &listed_before)
            .unwrap_err()
            .code,
        ErrorCode::ListingChanged
    );
    let d = backend
        .current_file_deletion(&management, &file)
        .unwrap()
        .unwrap();
    assert_eq!(d.branch_heads.len(), 2);
    assert_eq!(d.parent_node_ids.len(), 2);
    assert_eq!(d.branch_heads[&review], tagged);
    assert!(d.parent_node_ids.contains(&a.version_id));
    assert!(backend.authorize_version(&smb, &a.version_id).is_err());
    assert!(
        backend
            .lookup_current(
                &smb,
                &repo.main_branch_id,
                &name_policy::LookupPath::parse("A.txt").unwrap(),
            )
            .unwrap()
            .is_none()
    );
    backend.authorize_version(&smb, &b.version_id).unwrap();
    let new_branch = |entry: &Entry, name: &str| {
        backend.create_file_branch(
            &management,
            storage_server::CreateFileBranch {
                file_id: entry.file_id.clone(),
                source_version_id: entry.version_id.clone(),
                source_node_id: None,
                name: storage_contract::BranchName::parse(name).unwrap(),
                operation_id: OperationId::generate().unwrap(),
            },
        )
    };
    assert_eq!(
        new_branch(&a, "must remain frozen").unwrap_err().code,
        ErrorCode::BranchNotActive
    );
    assert_eq!(
        backend
            .update_file_node_tags(
                &management,
                storage_server::UpdateFileNodeTags {
                    file_id: a.file_id.clone(),
                    targets: vec![storage_contract::FileTagTarget {
                        branch_id: review.clone(),
                        source_node_id: tagged.clone(),
                        expected_node_id: tagged.clone(),
                    }],
                    add: vec!["blocked".into()],
                    remove: vec![],
                    location: None,
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap_err()
            .code,
        ErrorCode::BranchNotActive
    );
    assert_eq!(
        backend_save(&backend, &management, &review, "A.txt", b"blocked")
            .unwrap_err()
            .code,
        ErrorCode::BranchNotActive
    );
    for note in ["blocked edit", ""] {
        for target in [
            storage_server::NoteTarget::File(a.file_id.clone()),
            storage_server::NoteTarget::Version(
                a.version_id
                    .parse::<storage_contract::VersionId>()
                    .unwrap()
                    .into(),
            ),
            storage_server::NoteTarget::Node {
                file: a.file_id.clone(),
                node: tagged.clone(),
            },
        ] {
            assert_eq!(
                backend
                    .update_note(
                        &management,
                        storage_server::UpdateNote {
                            target,
                            note: storage_contract::Note::parse(note.into()).unwrap(),
                            operation_id: OperationId::generate().unwrap(),
                        },
                    )
                    .unwrap_err()
                    .code,
                ErrorCode::BranchNotActive
            );
        }
    }
    backend
        .update_note(
            &management,
            storage_server::UpdateNote {
                target: storage_server::NoteTarget::File(b.file_id.clone()),
                note: storage_contract::Note::parse("B remains editable".into()).unwrap(),
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    new_branch(&b, "B review").unwrap();
    backend_save(
        &backend,
        &management,
        &repo.main_branch_id,
        "B.txt",
        b"B edits",
    )
    .unwrap();
    backend_save(
        &backend,
        &management,
        &repo.main_branch_id,
        "C.txt",
        b"retained A",
    )
    .unwrap();
    let c = backend
        .lookup_current(
            &smb,
            &repo.main_branch_id,
            &name_policy::LookupPath::parse("C.txt").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_ne!(c.file_id, a.file_id);
    new_branch(&c, "C review").unwrap();
    backend_save(
        &backend,
        &management,
        &repo.main_branch_id,
        "C.txt",
        b"C edits",
    )
    .unwrap();
    assert_eq!(
        backend
            .current_file_deletion(&management, &file)
            .unwrap()
            .unwrap(),
        d,
        "other files must not modify the frozen deletion frontier"
    );
    let joined = backend_shared_graph(&backend, &management, &b.file_id);
    assert_eq!(joined.files.len(), 3);
    assert!(joined.shared_contents.iter().any(|shared| {
        shared
            .version_ids
            .iter()
            .any(|id| id.as_str() == a.version_id)
            && shared
                .version_ids
                .iter()
                .any(|id| id.as_str() == b.version_id)
            && shared
                .version_ids
                .iter()
                .any(|id| id.as_str() == c.version_id)
    }));
    let deletion = joined
        .nodes
        .iter()
        .find(|node| node.node_id.as_str() == d.node_id)
        .unwrap();
    assert_eq!(
        deletion.kind,
        storage_contract::shared_history::SharedHistoryNodeKind::Deletion
    );
    assert!(deletion.version_id.is_none());
    assert_eq!(
        deletion
            .parent_node_ids
            .iter()
            .map(|node| node.as_str())
            .collect::<BTreeSet<_>>(),
        d.parent_node_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    backend
        .authorize_version(&management, &a.version_id)
        .unwrap();
    assert!(backend.current_file_deletion(&smb, &file).is_err());
    assert!(
        backend
            .restore_deleted_file(
                &smb,
                storage_server::RestoreDeletedFile {
                    file_id: file.clone(),
                    deletion_node_id: d.node_id.clone(),
                    operation_id: OperationId::generate().unwrap()
                }
            )
            .is_err()
    );
    backend
        .restore_deleted_file(
            &management,
            storage_server::RestoreDeletedFile {
                file_id: file,
                deletion_node_id: d.node_id,
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    backend.authorize_version(&smb, &a.version_id).unwrap();
    drop(auth);
    drop(backend);
    let engine = Engine::open(&system).unwrap();
    let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
    assert_eq!(tree.get("A.txt").unwrap().file_id, a.file_id);
    assert_eq!(engine.state().file_notes[&a.file_id], "before deletion");
    assert_eq!(
        engine.state().version_notes[&a.version_id],
        "before deletion"
    );
    assert_eq!(engine.state().file_path_notes[&tagged], "before deletion");
    assert_eq!(engine.state().file_notes[&b.file_id], "B remains editable");
    for (name, bytes) in [
        ("A.txt", b"retained A".as_slice()),
        ("B.txt", b"B edits".as_slice()),
        ("C.txt", b"C edits".as_slice()),
    ] {
        assert_eq!(
            exported(&engine, root.path(), &tree.get(name).unwrap().version_id),
            bytes
        );
    }
    let request = storage_contract::shared_history::SharedHistoryRequest::parse(
        &b.file_id,
        engine
            .metadata_view()
            .history_root(&repo.history_tree_id)
            .unwrap(),
    )
    .unwrap();
    let read = engine
        .capture_shared_history(
            request.clone(),
            storage_contract::history_window::HistoryWindowCancellation::new(),
        )
        .unwrap();
    let graph = read.graph().unwrap();
    engine.recheck_shared_history(&read).unwrap();
    graph.validate(&request).unwrap();
    assert_eq!(
        graph
            .files
            .iter()
            .map(|file| file.file_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([a.file_id.as_str(), b.file_id.as_str(), c.file_id.as_str()])
    );
}

fn backend_save(
    backend: &EngineBackend,
    identity: &Identity,
    branch: &str,
    path: &str,
    bytes: &[u8],
) -> Result<storage_contract::OperationResult, storage_server::ApiError> {
    use std::io::Write;
    let workspace = backend.create_workspace(
        identity,
        storage_server::CreateWorkspace {
            branch_id: branch.into(),
            source_commit_id: None,
        },
    )?;
    let workspace_id = workspace.workspace_id.clone();
    let mut source = tempfile::NamedTempFile::new().unwrap();
    source.write_all(bytes).unwrap();
    if let Err(error) = backend.upload(
        identity,
        &workspace_id,
        &storage_contract::new_id("entry").unwrap(),
        &name_policy::PortablePath::parse(path).unwrap(),
        &storage_server::UploadSource::from_file(source.reopen().unwrap(), 1024).unwrap(),
    ) {
        backend.abort_workspace(identity, &workspace_id).unwrap();
        return Err(error);
    }
    let result = backend.commit_workspace(
        identity,
        storage_server::CommitWorkspace {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "soft deletion adapter save".into(),
            request_digest: None,
        },
    );
    if result.is_err() {
        backend.abort_workspace(identity, &workspace_id).unwrap();
    }
    result
}

fn backend_shared_graph(
    backend: &EngineBackend,
    identity: &Identity,
    file: &str,
) -> storage_contract::shared_history::SharedHistoryGraph {
    use storage_contract::history_window::{
        HistoryCollection, HistoryPageLimit, HistoryWindowCancellation, HistoryWindowPage,
        HistoryWindowRequest,
    };
    let page = backend
        .history_window(
            identity,
            HistoryWindowRequest {
                file_id: HistoryFileId::parse(file).unwrap(),
                collection: HistoryCollection::Nodes,
                location: None,
                history_root: None,
                cursor: None,
                anchor_node_id: None,
                limit: HistoryPageLimit::new(1).unwrap(),
            },
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    let HistoryWindowPage::Nodes { identity: page, .. } = page else {
        panic!("expected nodes page");
    };
    let request = storage_contract::shared_history::SharedHistoryRequest {
        file_id: page.file_id,
        history_root: page.history_root,
    };
    let graph = backend
        .shared_history(identity, request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    graph.validate(&request).unwrap();
    graph
}
