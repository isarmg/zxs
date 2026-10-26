//! Directory selection admits the complete composed tree, not just its subtree.
//! All namespace, publication, and restart checks use disposable storage pools.
use std::path::PathBuf;
use storage_engine::{CommitRequest, Config, Engine, NamespaceOperation, Repository, new_id};

struct Pool {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repo: Repository,
}

impl Pool {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        config.max_entries = 4;
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository("Directory quota", "P01", 3, "admin", &new_id("op"))
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            engine,
            repo,
        }
    }

    fn namespace(&mut self, kind: storage_contract::NamespaceOperationKind, path: &str) {
        self.engine
            .file_namespace(
                &self.repo.repository_id,
                NamespaceOperation {
                    kind,
                    path: path.into(),
                    destination: None,
                },
                "admin",
                &new_id("op"),
            )
            .unwrap();
    }

    fn save(&mut self, path: &str) {
        let workspace = self
            .engine
            .create_workspace(&self.repo.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, path.as_bytes())
            .unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Directory quota fixture".into(),
                request_digest: None,
            })
            .unwrap();
    }

    fn register(&mut self, path: &str, name: &str) -> String {
        self.engine
            .create_directory_branches(&self.repo.repository_id, path, name, "admin", &new_id("op"))
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn paths(&self) -> Vec<String> {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect()
    }

    fn state_bytes(&self) -> Vec<u8> {
        json_codec::to_vec(self.engine.state()).unwrap()
    }

    fn journals(&self) -> Vec<(String, Vec<u8>)> {
        self.engine
            .config()
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .map(|disk| {
                (
                    disk.disk_id.clone(),
                    std::fs::read(disk.root.join("metadata/application-log/journal.bin")).unwrap(),
                )
            })
            .collect()
    }
}

fn sibling_compositions() -> (Pool, String, String) {
    let mut pool = Pool::new();
    pool.namespace(storage_contract::NamespaceOperationKind::Mkdir, "left");
    pool.namespace(storage_contract::NamespaceOperationKind::Mkdir, "right");
    pool.save("left/one.txt");
    pool.save("left/two.txt");
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
    let left = pool.register("left", "full");
    let left_root = pool.register("", "left-full");
    pool.namespace(
        storage_contract::NamespaceOperationKind::Delete,
        "left/one.txt",
    );
    pool.namespace(
        storage_contract::NamespaceOperationKind::Delete,
        "left/two.txt",
    );
    pool.save("right/one.txt");
    pool.save("right/two.txt");
    assert_eq!(
        pool.paths(),
        ["left", "right", "right/one.txt", "right/two.txt"]
    );
    let right = pool.register("right", "full");
    let right_root = pool.register("", "right-full");

    // Both roots were created legally at the four-entry limit. Combining their
    // two-file sibling subtrees would instead expose six entries in one tree.
    let branches = &pool.engine.state().directory_branches;
    assert_eq!(branches[&left_root].entries.len(), 4);
    assert_eq!(branches[&right_root].entries.len(), 4);
    assert_eq!(
        branches[&left].entries.len() + branches[&right].entries.len() + 2,
        6
    );
    (pool, left_root, right_root)
}

#[test]
fn oversized_sibling_selection_preserves_state_journals_and_operation_id() {
    let (mut pool, _, _) = sibling_compositions();
    let operation = new_id("op");
    let before = pool.state_bytes();
    let journals = pool.journals();
    assert_eq!(journals.len(), 3);

    for _ in 0..2 {
        let error = pool
            .engine
            .select_directory_branches(
                &pool.repo.repository_id,
                "left",
                "full",
                "admin",
                &operation,
            )
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
        // Includes commits, branch heads, file contexts, directory selections,
        // generations, events, operation receipts, and request bindings.
        assert_eq!(pool.state_bytes(), before);
        assert_eq!(pool.journals(), journals);
        assert_eq!(
            pool.engine.operation(&operation).unwrap_err().code(),
            storage_contract::ErrorCode::OperationNotFound
        );
        assert_eq!(pool.engine.operation_owner(&operation).unwrap(), None);
    }

    // The rejected operation did not reserve its key and can succeed once the
    // siblings leave enough room, without recreating its frozen composition.
    pool.namespace(
        storage_contract::NamespaceOperationKind::Delete,
        "right/one.txt",
    );
    pool.namespace(
        storage_contract::NamespaceOperationKind::Delete,
        "right/two.txt",
    );
    let result = pool
        .engine
        .select_directory_branches(
            &pool.repo.repository_id,
            "left",
            "full",
            "admin",
            &operation,
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
    assert_eq!(
        pool.engine.operation(&operation).unwrap().result,
        result.result
    );
    let after = pool.state_bytes();
    let journals = pool.journals();
    assert_eq!(
        pool.engine
            .select_directory_branches(
                &pool.repo.repository_id,
                "left",
                "full",
                "admin",
                &operation
            )
            .unwrap()
            .result,
        result.result
    );
    assert_eq!(pool.state_bytes(), after);
    assert_eq!(pool.journals(), journals);
}

#[test]
fn root_selection_replaces_the_tree_at_the_entry_limit_and_survives_restart() {
    let (mut pool, left_root, right_root) = sibling_compositions();
    pool.engine
        .select_default_directory_branch(
            &pool.repo.repository_id,
            &left_root,
            "admin",
            &new_id("op"),
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
    pool.engine
        .select_directory_branches(
            &pool.repo.repository_id,
            "",
            "right-full",
            "admin",
            &new_id("op"),
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "right", "right/one.txt", "right/two.txt"]
    );
    assert_eq!(
        pool.engine.state().directory_selections[&pool.repo.repository_id],
        right_root
    );
    let before = pool.state_bytes();
    let journals = pool.journals();
    let Pool {
        _temporary,
        root,
        engine,
        repo,
    } = pool;
    drop(engine);
    let recovered = Pool {
        engine: Engine::open(&root).unwrap(),
        _temporary,
        root,
        repo,
    };
    assert_eq!(recovered.state_bytes(), before);
    assert_eq!(recovered.journals(), journals);
}
