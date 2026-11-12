//! Actual published proofs on disposable disks, shared by publication boundary tests.
use crate::disk::version_dir;
use crate::journal::{ContentTombstone, decode_proof};
use crate::{CommitRequest, Config, Engine, FileVersion, OperationId, Repository};
use std::path::PathBuf;

pub(crate) struct Fixture {
    pub engine: Engine,
    pub version: FileVersion,
    pub directory: PathBuf,
    _temporary: tempfile::TempDir,
    repository: Repository,
}

impl Fixture {
    pub fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let config = Config::development([
            root.join("member-a"),
            root.join("member-b"),
            root.join("member-c"),
        ])
        .unwrap();
        let mut engine = Engine::initialize(&root.join("system"), config).unwrap();
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("publication source").unwrap(),
                "P01",
                3,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        save(&mut engine, &repository, b"original body");
        let entry = engine
            .list_tree(&repository.main_branch_id)
            .unwrap()
            .remove(0);
        let version = engine.state().versions[&entry.version_id].clone();
        let directory = version_dir(
            &engine.config().disks[0],
            &version.history_tree_id,
            &version.version_id,
        );
        Self {
            engine,
            version,
            directory,
            _temporary: temporary,
            repository,
        }
    }

    pub fn purge(&mut self) -> ContentTombstone {
        save(&mut self.engine, &self.repository, b"replacement body");
        let plan = self
            .engine
            .create_purge_plan(
                &self.version.history_tree_id,
                vec![self.version.version_id.clone()],
                "admin",
            )
            .unwrap();
        self.engine
            .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
            .unwrap();
        decode_proof(&std::fs::read(self.directory.join("purged.json")).unwrap()).unwrap()
    }
}

fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "publication source".into(),
            request_digest: None,
        })
        .unwrap();
}
