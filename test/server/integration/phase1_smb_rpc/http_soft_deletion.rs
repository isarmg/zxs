//! Router/Engine acceptance for ordinary Web deletion. All files are temporary;
//! native SMB networking and a browser are separate acceptance targets.
use super::soft_deletion::request;
use super::*;
use json_codec::{Value, json};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn seed() -> (tempfile::TempDir, Engine, Repository, Entry, Entry) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("state"),
        Config::development([root.path().join("p")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    for path in ["A.txt", "B.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"same retained bytes")
            .unwrap();
    }
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            operation_id: OperationId::generate().unwrap(),
            message: "HTTP soft delete seed".into(),
            request_digest: None,
        })
        .unwrap();
    let a = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("A.txt")
        .unwrap()
        .clone();
    let b = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("B.txt")
        .unwrap()
        .clone();
    engine
        .create_file_branch(
            &a.file_id,
            &a.version_id,
            &storage_contract::BranchName::parse("review").unwrap(),
            "alice",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    (root, engine, repo, a, b)
}
fn command(repo: &Repository, entry: &Entry, operation: OperationId) -> storage_server::DeleteFile {
    storage_server::DeleteFile {
        repository_id: HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
        file_id: HistoryFileId::parse(&entry.file_id).unwrap(),
        path: name_policy::LookupPath::parse(entry.path.as_str()).unwrap(),
        operation_id: operation,
    }
}

#[test]
fn http_whole_file_deletion_retains_all_branches_replays_after_undo_and_cold_opens() {
    let (root, engine, repo, a, b) = seed();
    let operation = OperationId::generate().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let backend=Arc::new(EngineBackend::new(engine));
        let auth=backend.protect_auth(AuthStore::initialize(&root.path().join("state"),"alice","phase-one-smb-password").unwrap()).unwrap();
        let identity=auth.sole_identity().unwrap();
        let app=storage_server::router(backend.clone(),auth,storage_server::ServerConfig::new(&root.path().join("state"))).unwrap();
        let endpoint=format!("/api/files/{}/deletion",a.file_id);
        let body=json!({"repository_id":repo.repository_id,"path":"A.txt","operation_id":operation});
        assert_eq!(request(&app,None,"POST",&endpoint,body.clone()).await.0,http::StatusCode::UNAUTHORIZED);
        let (status,login)=request(&app,None,"POST","/api/auth/login",json!({"username":"alice","password":"phase-one-smb-password"})).await;
        assert_eq!(status,http::StatusCode::OK,"{login}");
        let token=login["token"].as_str().unwrap();
        let mut subset=body.clone();subset["branch_ids"]=json!([]);
        assert_eq!(request(&app,Some(token),"POST",&endpoint,subset).await.0,http::StatusCode::BAD_REQUEST);
        assert_eq!(request(&app,Some(token),"POST",&format!("{endpoint}?force=true"),body.clone()).await.0,http::StatusCode::BAD_REQUEST);
        let other=format!("/api/files/{}/deletion",b.file_id);
        assert_eq!(request(&app,Some(token),"POST",&other,body.clone()).await.0,http::StatusCode::NOT_FOUND);
        assert!(backend.current_file_deletion(&identity,&HistoryFileId::parse(&a.file_id).unwrap()).unwrap().is_none());
        let first=request(&app,Some(token),"POST",&endpoint,body.clone()).await;
        assert_eq!(first.0,http::StatusCode::OK,"{}",first.1);
        assert_eq!(first.1["operation_kind"],"file.delete");
        let node=&first.1["result"]["deletion_node"];
        assert_eq!(node["file_id"],a.file_id);
        assert_eq!(node["branch_heads"].as_object().unwrap().len(),2);
        assert!(node.get("version_id").is_none());
        let visible=backend.tree_snapshot(&identity,ReadTree{branch:&repo.main_branch_id,commit:None}).unwrap();
        assert!(!visible.entries.iter().any(|entry|entry.file_id==a.file_id));
        assert!(visible.entries.iter().any(|entry|entry.file_id==b.file_id));
        assert_eq!(request(&app,Some(token),"POST",&endpoint,body.clone()).await,first);
        let mut equivalent=body.clone();equivalent["path"]="a.TXT".into();
        assert_eq!(request(&app,Some(token),"POST",&endpoint,equivalent).await,first);
        let mut changed=body.clone();changed["path"]="B.txt".into();
        let rejection=request(&app,Some(token),"POST",&endpoint,changed).await;
        assert_eq!(rejection.0,http::StatusCode::CONFLICT,"{}",rejection.1);
        assert_eq!(rejection.1["error"]["code"],"IDEMPOTENCY_KEY_REUSED");
        let undo=request(&app,Some(token),"POST",&format!("{endpoint}/undo"),json!({"deletion_node_id":node["node_id"],"operation_id":OperationId::generate().unwrap()})).await;
        assert_eq!(undo.0,http::StatusCode::OK,"{}",undo.1);
        assert_eq!(request(&app,Some(token),"POST",&endpoint,body).await,first);
        assert_eq!(request(&app,Some(token),"GET",&endpoint,json!({})).await.1,Value::Null);
        let visible=backend.tree_snapshot(&identity,ReadTree{branch:&repo.main_branch_id,commit:None}).unwrap();
        assert!(visible.entries.iter().any(|entry|entry.file_id==a.file_id&&entry.entry_id==a.entry_id));
        // Repeating the SMB adapter request observes the same permanent receipt,
        // even though today's visibility changed. It cannot delete again.
        let replay=backend.delete_current_file(&identity,storage_server::DeleteCurrentFile {
            branch_id:repo.main_branch_id.clone(),file_id:HistoryFileId::parse(&a.file_id).unwrap(),
            expected_entry_id:a.entry_id.clone(),path:name_policy::LookupPath::parse("A.txt").unwrap(),operation_id:operation,
        }).unwrap();
        assert_eq!(replay.unwrap().result["deletion_node"],*node);
        assert!(backend.current_file_deletion(&identity,&HistoryFileId::parse(&a.file_id).unwrap()).unwrap().is_none());
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    let engine = Engine::open(&root.path().join("state")).unwrap();
    assert!(engine.file_deletion(&a.file_id).is_none());
    assert_eq!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("A.txt")
            .unwrap()
            .file_id,
        a.file_id
    );
    assert_eq!(
        exported(&engine, root.path(), &a.version_id),
        b"same retained bytes"
    );
    assert!(engine.state().operations.contains_key(&operation));
}

#[test]
fn soft_delete_adapter_checks_current_auth_and_preserves_cross_interface_receipt_identity() {
    let (root, engine, repo, a, _) = seed();
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
    let mut smb = identity.clone();
    smb.smb_binding = Some(storage_server::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation,
    });
    let operation = OperationId::generate().unwrap();
    assert_eq!(
        backend
            .soft_delete_file(&smb, command(&repo, &a, operation))
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let first = backend
        .delete_current_file(
            &smb,
            storage_server::DeleteCurrentFile {
                branch_id: repo.main_branch_id.clone(),
                path: name_policy::LookupPath::parse("A.txt").unwrap(),
                expected_entry_id: a.entry_id.clone(),
                file_id: HistoryFileId::parse(&a.file_id).unwrap(),
                operation_id: operation,
            },
        )
        .unwrap()
        .expect("native file deletion");
    assert_eq!(
        backend
            .soft_delete_file(&identity, command(&repo, &a, operation))
            .unwrap(),
        first
    );
    let file = HistoryFileId::parse(&a.file_id).unwrap();
    let node = backend
        .current_file_deletion(&identity, &file)
        .unwrap()
        .unwrap();
    backend
        .restore_deleted_file(
            &identity,
            storage_server::RestoreDeletedFile {
                file_id: file.clone(),
                deletion_node_id: node.node_id.clone(),
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    let second = backend
        .soft_delete_file(
            &identity,
            command(&repo, &a, OperationId::generate().unwrap()),
        )
        .unwrap();
    assert_ne!(second.result["deletion_node"]["node_id"], node.node_id);
    assert_eq!(
        backend
            .soft_delete_file(&identity, command(&repo, &a, operation))
            .unwrap(),
        first
    );
    assert_eq!(
        backend
            .current_file_deletion(&identity, &file)
            .unwrap()
            .unwrap()
            .node_id,
        second.result["deletion_node"]["node_id"].as_str().unwrap()
    );
}

#[path = "deletion_review.rs"]
mod deletion_review;
