//! Real publication and cold-read cases, without creating shared identities.
#[path = "content_matches/rename.rs"]
mod rename;
#[path = "content_matches/shared_history.rs"]
mod shared_history;
use storage_contract::content_matches::{ContentMatchPage, ContentMatchRequest};
use storage_contract::history_window::HistoryWindowCancellation;
use storage_engine::{
    BranchName, CommitRequest, Config, Engine, Entry, ErrorCode, FileNodeTagsUpdateRequest,
    FileTagTarget, OperationId, Workspace,
};
fn commit(workspace: Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "content match regression".into(),
        request_digest: None,
    }
}
struct Pool {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Engine,
    branch: String,
    history: String,
    repository: String,
}
impl Pool {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine = Engine::initialize(
            &root,
            Config::development([temp.path().join("disk")]).unwrap(),
        )
        .unwrap();
        let repo = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        Self {
            _temp: temp,
            root,
            engine,
            branch: repo.main_branch_id,
            history: repo.history_tree_id,
            repository: repo.repository_id,
        }
    }
    fn save(&mut self, files: &[(&str, &[u8])]) {
        let ws = self.engine.create_workspace(&self.branch, "admin").unwrap();
        for (path, bytes) in files {
            self.engine
                .stage_bytes(&ws.workspace_id, path, bytes)
                .unwrap();
        }
        self.engine.commit_workspace(commit(ws)).unwrap();
    }
    fn entry(&self, path: &str) -> Entry {
        self.engine
            .namespace_tree(&self.branch)
            .unwrap()
            .get(path)
            .unwrap()
            .clone()
    }
    fn request(
        &self,
        entry: &Entry,
        node: &str,
        after: Option<&str>,
        limit: usize,
    ) -> ContentMatchRequest {
        let view = self.engine.metadata_view();
        ContentMatchRequest::parse(
            &entry.file_id,
            node,
            view.history_root(&self.history).unwrap(),
            after,
            limit,
        )
        .unwrap()
    }
    fn page(&self, request: ContentMatchRequest) -> ContentMatchPage {
        let read = self
            .engine
            .capture_content_matches(request.clone(), HistoryWindowCancellation::new())
            .unwrap();
        let page = read.page().unwrap();
        self.engine.recheck_content_matches(&read).unwrap();
        page.validate(&request).unwrap();
        page
    }
}
#[test]
fn content_matches_are_bounded_nonempty_and_stable_after_cold_reopen() {
    let mut pool = Pool::new();
    pool.save(&[
        ("a", b"shared"),
        ("b", b"shared"),
        ("c", b"shared"),
        ("different", b"others"),
        ("empty-a", b""),
        ("empty-b", b""),
    ]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    let c = pool.entry("c");
    let foreign = pool
        .engine
        .create_repository(
            &storage_contract::RepositoryName::parse("other history").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let ws = pool
        .engine
        .create_workspace(&foreign.main_branch_id, "admin")
        .unwrap();
    pool.engine
        .stage_bytes(&ws.workspace_id, "foreign", b"shared")
        .unwrap();
    pool.engine.commit_workspace(commit(ws)).unwrap();
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let first = pool.page(pool.request(&a, &a.version_id, None, 1));
    assert_eq!(first.matches.len(), 1);
    let after = first.next_after_version_id.as_ref().unwrap();
    let second = pool.page(pool.request(&a, &a.version_id, Some(after.as_str()), 1));
    assert_eq!(second.matches.len(), 1);
    assert!(second.next_after_version_id.is_none());
    let mut actual = vec![
        first.matches[0].version_id.to_string(),
        second.matches[0].version_id.to_string(),
    ];
    actual.sort();
    let mut expected = vec![b.version_id, c.version_id];
    expected.sort();
    assert_eq!(actual, expected);
    let empty = pool.entry("empty-a");
    let empty_page = pool.page(pool.request(&empty, &empty.version_id, None, 64));
    assert_eq!(empty_page.logical_size, 0);
    assert!(empty_page.matches.is_empty());
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(pool.page(pool.request(&a, &a.version_id, None, 1)), first);
}
#[test]
fn content_matches_accept_metadata_nodes_but_reject_wrong_file_and_cursor() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared"), ("c", b"other")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    let c = pool.entry("c");
    let branch = pool
        .engine
        .create_file_branch(
            &a.file_id,
            &a.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let node = pool
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &a.file_id,
            targets: &[FileTagTarget {
                branch_id: branch,
                source_node_id: a.version_id.clone(),
                expected_node_id: a.version_id.clone(),
            }],
            add: &["label".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = pool.page(pool.request(&a, &node, None, 64));
    assert_eq!(page.source_version_id.as_str(), a.version_id);
    assert_eq!(page.matches.len(), 1);
    let request = pool.request(&a, &b.version_id, None, 64);
    let read = pool
        .engine
        .capture_content_matches(request, HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::FileNodeNotFound);
    let request = pool.request(&a, &node, Some(&c.version_id), 64);
    let read = pool
        .engine
        .capture_content_matches(request, HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::InvalidRequest);
}
#[test]
fn content_matches_cancel_and_recheck_exact_history_after_concurrent_publication() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared")]);
    let a = pool.entry("a");
    let token = HistoryWindowCancellation::new();
    let request = pool.request(&a, &a.version_id, None, 64);
    let read = pool
        .engine
        .capture_content_matches(request.clone(), token.clone())
        .unwrap();
    token.cancel();
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::OperationAborted);
    assert_eq!(
        pool.engine
            .recheck_content_matches(&read)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    let read = pool
        .engine
        .capture_content_matches(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    assert_eq!(page.matches.len(), 1);
    pool.save(&[("b", b"changed")]);
    assert_eq!(
        pool.engine
            .recheck_content_matches(&read)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    assert_eq!(
        pool.engine
            .capture_content_matches(request, HistoryWindowCancellation::new())
            .err()
            .unwrap()
            .code(),
        ErrorCode::HistoryChanged
    );
    // The old immutable B version still matches, independently of its new HEAD.
    assert_eq!(
        pool.page(pool.request(&a, &a.version_id, None, 64))
            .matches
            .len(),
        1
    );
}

#[test]
fn content_matches_preserve_purged_version_facts_without_reading_bodies() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"old shared"), ("b", b"old shared")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    pool.save(&[("a", b"new a"), ("b", b"new b")]);
    let plan = pool
        .engine
        .create_purge_plan(
            &pool.history,
            vec![a.version_id.clone(), b.version_id.clone()],
            "admin",
        )
        .unwrap();
    pool.engine
        .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
        .unwrap();
    let page = pool.page(pool.request(&a, &a.version_id, None, 64));
    assert_eq!(
        page.source_content_state,
        Some(storage_engine::ContentState::Purged)
    );
    assert_eq!(page.matches.len(), 1);
    assert_eq!(page.matches[0].version_id.as_str(), b.version_id);
    assert_eq!(
        page.matches[0].content_state,
        Some(storage_engine::ContentState::Purged)
    );
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(pool.page(pool.request(&a, &a.version_id, None, 64)), page);
}
