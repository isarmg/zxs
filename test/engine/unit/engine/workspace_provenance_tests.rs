//! A real single-owner fresh pool, not an inferred history/receipt fixture.
use crate::{CommitRequest,Config,Engine,OperationResult};
use storage_contract::{BranchName,Mode,OperationId,Principal};

fn put(engine:&mut Engine,branch:&str,body:&[u8]) -> (CommitRequest,OperationResult) {
    let workspace=engine.create_workspace(branch,"original author").unwrap();
    engine.stage_bytes(&workspace.workspace_id,"document.txt",body).unwrap();
    let input=CommitRequest {workspace_id:workspace.workspace_id,principal:"original author".into(),
        operation_id:OperationId::generate().unwrap(),expected_head:workspace.base_head,
        fencing_token:workspace.fencing_token,message:"explicit original".into(),request_digest:None};
    let result=engine.commit_workspace(input.clone()).unwrap();
    (input,result)
}

#[test]
fn original_workspace_decision_is_manual_only_and_survives_exact_cold_retry() {
    let tmp=tempfile::tempdir().unwrap();
    let system=tmp.path().join("system");
    let config=Config::independent_roots(Mode::Development,
        [(name_policy::Name::parse("primary").unwrap(),tmp.path().join("disk"))]).unwrap();
    let slot=config.disks[0].slot_id.clone();
    let mut engine=Engine::initialize(&system,config).unwrap();
    let repo=engine.create_disk_root(&slot,"original author",OperationId::generate().unwrap()).unwrap();
    let (_,birth)=put(&mut engine,&repo.main_branch_id,b"first");
    assert_eq!(birth.result["workspace_file_origin"]["kind"],"no_such_creation");
    let entry=engine.list_tree(&repo.main_branch_id).unwrap().into_iter().find(|v|v.path()=="document.txt").unwrap();
    let branch=engine.create_file_branch(&entry.file_id,&entry.version_id,&BranchName::parse("edited").unwrap(),
        "original author",OperationId::generate().unwrap()).unwrap().branch_id.unwrap();
    engine.select_file_branch(&entry.file_id,&branch,"original author",OperationId::generate().unwrap()).unwrap();
    // The first save on an inherited branch consumes its pending, unowned
    // base. It does not prove E. The subsequent independent change does.
    let (_,inherited)=put(&mut engine,&branch,b"now independently owned");
    assert_eq!(inherited.result["workspace_file_origin"]["kind"],"no_such_creation");
    let (input,issued)=put(&mut engine,&branch,b"focused explicit change");
    let actor:Principal="original author".parse().unwrap();
    let fact=super::original(engine.state(),&issued,&actor).unwrap().unwrap();
    assert_eq!(fact.reason,crate::business::file_draft::origin::Reason::ManualFile{});
    assert_eq!(fact.operation,input.operation_id);
    assert_eq!(fact.target.file.to_string(),entry.file_id);
    assert_eq!(fact.node.to_string(),engine.file_branch_head(&branch).unwrap());
    // Structural admission rejects missing/unknown or borrowed stable IDs; it
    // cannot turn the visible operation into a new automatic or native cause.
    for mutation in ["missing","other-file","unknown","wrong-actor"] {
        let mut state=engine.state().clone();
        let receipt=state.operations.get_mut(&input.operation_id).unwrap();
        match mutation {
            "missing"=>{receipt.result.as_object_mut().unwrap().remove("workspace_file_origin");},
            "other-file"=>receipt.result["workspace_file_origin"]["binding"]["target"]["file"]=json_codec::json!(storage_contract::FileId::generate().unwrap()),
            "unknown"=>receipt.result["workspace_file_origin"]["kind"]=json_codec::json!("automatic_save"),
            _=>{state.operation_principals.insert(input.operation_id,"other author".into());}
        }
        assert!(super::retained(&state,&state.operations[&input.operation_id]).is_err(),"{mutation}");
    }
    let scope=storage_contract::automatic_observation::Scope {repository:repo.repository_id.parse().unwrap(),
        directory:storage_contract::snapshot_worktree::NamespaceDirectoryId::Repository(repo.repository_id.parse().unwrap()),
        start:storage_contract::Timestamp::from_unix_seconds(0),end:storage_contract::Timestamp::from_unix_seconds(u64::MAX)};
    let req=storage_contract::automatic_observation::Request {scope,position:storage_contract::automatic_observation::Position::First};
    assert!(engine.automatic_node_page(&req).unwrap().0.rows.is_empty());
    drop(engine);
    let mut cold=Engine::open(&system).unwrap();
    let replay=cold.commit_workspace(input).unwrap();
    assert_eq!(replay,issued);
    assert_eq!(super::original(cold.state(),&replay,&actor).unwrap(),Some(fact));
    assert!(cold.automatic_node_page(&req).unwrap().0.rows.is_empty());
}
