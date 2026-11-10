//! Directory selection admits the complete composed tree, not just its subtree.
//! All namespace, publication, and restart checks use disposable storage pools.
use std::{io::Read, path::PathBuf};
use storage_engine::{
    CommitRequest, Config, DirectoryCommitRequest, Engine, NamespaceOperation, Repository,
};

#[path = "../support/directory_git_fixture.rs"]
mod directory_fixture;

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
        ])
        .unwrap();
        config.max_entries = 4;
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Directory quota").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            engine,
            repo,
        }
    }

    fn namespace(
        &mut self,
        kind: storage_contract::NamespaceOperationKind,
        path: &str,
        target: Option<&str>,
    ) {
        self.engine
            .file_namespace(
                &self.repo.repository_id,
                NamespaceOperation::parse(kind, path, target).unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
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
                operation_id: storage_contract::OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Directory quota fixture".into(),
                request_digest: None,
            })
            .unwrap();
    }

    fn register(&mut self, path: &str, name: &str) -> String {
        self.checkpoint(path);
        self.engine
            .create_directory_branches(
                &self.repo.repository_id,
                (if (path).is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                &storage_contract::BranchName::parse(name).unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn select(&mut self, path: &str, name: &str) {
        self.engine
            .select_directory_branches(
                &self.repo.repository_id,
                Some(&name_policy::LookupPath::parse(path).unwrap()),
                &storage_contract::BranchName::parse(name).unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
    }

    fn checkpoint(&mut self, path: &str) {
        directory_fixture::checkpoint(&mut self.engine, &self.repo, path);
    }

    fn paths(&self) -> Vec<String> {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path.into_string())
            .collect()
    }

    fn assert_file(&self, path: &str, expected: &[u8]) {
        let entry = self
            .engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path() == path)
            .unwrap();
        let mut body = Vec::new();
        self.engine
            .open_content(&entry.version_id)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, expected);
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

fn empty_composition() -> Pool {
    let mut pool = Pool::new();
    pool.namespace(
        storage_contract::NamespaceOperationKind::Mkdir,
        "left",
        None,
    );
    pool.namespace(
        storage_contract::NamespaceOperationKind::Mkdir,
        "right",
        None,
    );
    pool
}

fn fill_left(pool: &mut Pool) {
    pool.save("left/one.txt");
    pool.save("left/two.txt");
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
}

fn rename_left_to_right(pool: &mut Pool) {
    pool.namespace(
        storage_contract::NamespaceOperationKind::Rename,
        "left/one.txt",
        Some("right/one.txt"),
    );
    pool.namespace(
        storage_contract::NamespaceOperationKind::Rename,
        "left/two.txt",
        Some("right/two.txt"),
    );
    assert_eq!(
        pool.paths(),
        ["left", "right", "right/one.txt", "right/two.txt"]
    );
}

fn sibling_compositions() -> Pool {
    let mut pool = empty_composition();
    pool.register("left", "empty");
    pool.register("right", "empty");
    fill_left(&mut pool);
    let left = pool.register("left", "full");

    // Checkout a committed empty branch instead of deleting owned files.
    pool.select("left", "empty");
    assert_eq!(pool.paths(), ["left", "right"]);
    pool.save("right/one.txt");
    pool.save("right/two.txt");
    let right = pool.register("right", "full");

    // Each sibling was committed legally at the four-entry limit. Combining
    // their two-file subtrees would instead expose six entries in one tree.
    let branches = &pool.engine.state().directory_branches;
    assert_eq!(
        branches[&left].entries.len() + branches[&right].entries.len() + 2,
        6
    );
    pool
}

fn root_compositions() -> (Pool, String, String) {
    let mut pool = empty_composition();
    fill_left(&mut pool);
    // The root owns ordinary descendants. Independently initialized children
    // would be preserved by root checkout rather than replaced with its tree.
    let left_root = pool.register("", "left-full");
    rename_left_to_right(&mut pool);
    let right_root = pool.register("", "right-full");

    let branches = &pool.engine.state().directory_branches;
    assert_eq!(branches[&left_root].entries.len(), 4);
    assert_eq!(branches[&right_root].entries.len(), 4);
    (pool, left_root, right_root)
}

#[test]
fn oversized_sibling_selection_preserves_state_journals_and_operation_id() {
    let mut pool = sibling_compositions();
    let operation = storage_contract::OperationId::generate().unwrap();
    let before = pool.state_bytes();
    let journals = pool.journals();
    assert_eq!(journals.len(), 3);

    for _ in 0..2 {
        let error = pool
            .engine
            .select_directory_branches(
                &pool.repo.repository_id,
                Some(&name_policy::LookupPath::parse("left").unwrap()),
                &storage_contract::BranchName::parse("full").unwrap(),
                "admin",
                operation,
            )
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
        // Includes commits, branch heads, file contexts, directory selections,
        // generations, events, operation receipts, and request bindings.
        assert_eq!(pool.state_bytes(), before);
        assert_eq!(pool.journals(), journals);
        assert_eq!(
            pool.engine.operation(operation).unwrap_err().code(),
            storage_contract::ErrorCode::OperationNotFound
        );
        assert_eq!(pool.engine.operation_owner(operation).unwrap(), None);
    }

    // The rejected operation did not reserve its key and can succeed once the
    // siblings leave enough room, without recreating its frozen composition.
    pool.select("right", "empty");
    assert_eq!(pool.paths(), ["left", "right"]);
    let result = pool
        .engine
        .select_directory_branches(
            &pool.repo.repository_id,
            Some(&name_policy::LookupPath::parse("left").unwrap()),
            &storage_contract::BranchName::parse("full").unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
    pool.assert_file("left/one.txt", b"left/one.txt");
    pool.assert_file("left/two.txt", b"left/two.txt");
    assert_eq!(
        pool.engine.operation(operation).unwrap().result,
        result.result
    );
    let after = pool.state_bytes();
    let journals = pool.journals();
    assert_eq!(
        pool.engine
            .select_directory_branches(
                &pool.repo.repository_id,
                Some(&name_policy::LookupPath::parse("left").unwrap()),
                &storage_contract::BranchName::parse("full").unwrap(),
                "admin",
                operation
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
    let (mut pool, left_root, right_root) = root_compositions();
    pool.engine
        .select_default_directory_branch(
            &pool.repo.repository_id,
            &left_root,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "left/one.txt", "left/two.txt", "right"]
    );
    pool.engine
        .select_directory_branches(
            &pool.repo.repository_id,
            None,
            &storage_contract::BranchName::parse("right-full").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        pool.paths(),
        ["left", "right", "right/one.txt", "right/two.txt"]
    );
    pool.assert_file("right/one.txt", b"left/one.txt");
    pool.assert_file("right/two.txt", b"left/two.txt");
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
    recovered.assert_file("right/one.txt", b"left/one.txt");
    recovered.assert_file("right/two.txt", b"left/two.txt");
}
