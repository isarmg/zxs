use crate::{
    CommitRequest, Config, Engine, Entry, NamespaceOperation, NamespaceOperationKind,
    OperationId, Repository,
};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use storage_contract::history_window::HistoryWindowCancellation;
use storage_contract::ErrorCode;

struct Fixture {
    temporary: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
    first: Entry,
    second: Entry,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("disk")]).unwrap(),
        )
        .unwrap();
        let repository = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        for (path, body) in [("first.txt", b"first body"), ("second.txt", b"other body")] {
            engine.stage_bytes(&workspace.workspace_id, path, body).unwrap();
        }
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "path audit source".into(),
            request_digest: None,
        }).unwrap();
        let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
        let first = tree.get("first.txt").unwrap().clone();
        let second = tree.get("second.txt").unwrap().clone();
        Self { temporary, engine, repository, first, second }
    }

    fn rename(&mut self, from: &str, to: &str) -> crate::OperationResult {
        self.engine.file_namespace(
            &self.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        ).unwrap()
    }
}

#[test]
fn path_history_preserves_permanent_identity_and_body_across_rename_and_cold_open() {
    let mut fixture = Fixture::new();
    let versions = fixture.engine.state().versions.clone();
    let original = fixture.engine.open_content(&fixture.first.version_id).unwrap();
    let original_identity = original.file.metadata().unwrap();
    let receipt = fixture.rename("first.txt", "renamed.txt");
    let expected = crate::persist::path_changes::read(&receipt).unwrap().remove(0);
    let mut observed = Vec::new();
    fixture.engine.visit_file_path_changes(&HistoryWindowCancellation::new(), |fact| {
        observed.push(fact);
    }).unwrap();
    assert_eq!(observed.len(), 1);
    let fact = &observed[0];
    assert_eq!(fact.event_id, expected.event_id);
    assert_eq!(fact.operation_id, receipt.operation_id);
    assert_eq!(fact.file_id.to_string(), fixture.first.file_id);
    assert_eq!(fact.entry_id.to_string(), fixture.first.entry_id);
    assert_eq!(fact.history_tree_id.to_string(), fixture.repository.history_tree_id);
    assert_eq!(fact.branch_id, expected.branch_id);
    assert_eq!(fact.before, expected.before);
    assert_eq!(fact.after, expected.after);
    assert_eq!(fact.before.path.as_str(), "first.txt");
    assert_eq!(fact.after.path.as_str(), "renamed.txt");
    assert_eq!(fact.before_version_id.to_string(), fixture.first.version_id);
    assert_eq!(fact.after_version_id, fact.before_version_id);
    assert_eq!(fact.sequence.get(), receipt.sequence);
    assert_eq!(fact.occurred_at, expected.occurred_at);
    assert_eq!(fact.author.as_str(), "admin");
    assert_eq!(fixture.engine.state().versions, versions);
    assert!(fixture.engine.state().file_path_nodes.is_empty());
    let mut renamed = fixture.engine.open_content(&fixture.first.version_id).unwrap();
    let renamed_identity = renamed.file.metadata().unwrap();
    assert_eq!(renamed_identity.dev(), original_identity.dev());
    assert_eq!(renamed_identity.ino(), original_identity.ino());
    let mut body = Vec::new();
    renamed.file.read_to_end(&mut body).unwrap();
    assert_eq!(body, b"first body");
    drop(original);
    drop(renamed);
    let system = fixture.temporary.path().join("system");
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    let mut reopened = Vec::new();
    engine.visit_file_path_changes(&HistoryWindowCancellation::new(), |fact| {
        reopened.push(fact);
    }).unwrap();
    assert_eq!(reopened, observed);
    assert_eq!(engine.operation(receipt.operation_id).unwrap(), receipt);
}

#[test]
fn path_history_canceled_before_scan_never_calls_the_visitor() {
    let mut fixture = Fixture::new();
    fixture.rename("first.txt", "renamed.txt");
    let cancellation = HistoryWindowCancellation::new();
    cancellation.cancel();
    let mut callbacks = 0;
    let error = fixture.engine.visit_file_path_changes(&cancellation, |_| callbacks += 1)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert_eq!(callbacks, 0);
}

#[test]
fn path_history_rejects_a_corrupt_other_file_receipt_instead_of_hiding_it() {
    let mut fixture = Fixture::new();
    fixture.rename("first.txt", "renamed.txt");
    let other = fixture.rename("second.txt", "other.txt");
    let facts = crate::persist::path_changes::read(&other).unwrap();
    assert_eq!(facts[0].file_id.to_string(), fixture.second.file_id);
    let state = std::sync::Arc::make_mut(&mut fixture.engine.state);
    state.operations.get_mut(&other.operation_id).unwrap().result.as_object_mut().unwrap()
        .remove("path_changes");
    let before = storage_contract::canonical_json(state).unwrap();
    let mut observed = Vec::new();
    let error = fixture.engine.visit_file_path_changes(&HistoryWindowCancellation::new(), |fact| {
        if fact.file_id.to_string() == fixture.first.file_id {
            observed.push(fact);
        }
    }).unwrap_err();
    // Valid callbacks may precede the bad receipt; none make this scan complete.
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert_eq!(storage_contract::canonical_json(fixture.engine.state()).unwrap(), before);
}
