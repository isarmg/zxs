use super::*;
use std::sync::Mutex;

#[test]
fn serialized_engine_mutations_preserve_one_verified_hash_chain() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let engine = Mutex::new(Engine::initialize(&root, config).unwrap());
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    std::thread::scope(|scope| {
        for index in 0..8 {
            let engine = &engine;
            scope.spawn(move || {
                engine
                    .lock()
                    .unwrap()
                    .create_repository(
                        &storage_contract::RepositoryName::parse(&format!("repository-{index}"))
                            .unwrap(),
                        "P01",
                        1,
                        "admin",
                        storage_contract::OperationId::generate(),
                    )
                    .unwrap();
            });
        }
    });
    let scanned = scan(&disk).unwrap();
    assert!(!scanned.torn_tail);
    assert_eq!(
        scanned
            .records
            .iter()
            .filter(|(r, _)| r.record_type == RecordType::Publication)
            .count(),
        8
    );
    drop(engine);
    assert_eq!(Engine::open(&root).unwrap().state().repositories.len(), 8);
}

#[test]
fn detached_preparation_retains_writer_ownership_without_appending() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("repo").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"payload")
        .unwrap();
    let preparation = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let before = std::fs::read(log_path(&disk)).unwrap();
    drop(engine);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    let prepared = preparation.execute().unwrap();
    assert_eq!(std::fs::read(log_path(&disk)).unwrap(), before);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    drop(prepared);
    let reopened = Engine::open(&root).unwrap();
    assert_eq!(reopened.state().repositories.len(), 1);
}
