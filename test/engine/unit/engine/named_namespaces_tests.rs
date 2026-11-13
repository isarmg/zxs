//! Runtime name projections retain immutable audit paths and bind their source.
use super::*;
use std::path::PathBuf;
use crate::{CommitRequest, Config, DirectoryCommitRequest, OperationId, Repository};

#[path = "../../support/directory_git_fixture.rs"]
mod directory_fixture;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repository: Repository,
    retained: String,
    file_branch: String,
    directory: String,
    original_head: String,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut engine = Engine::initialize(
            &root, Config::development([temporary.path().join("disk")]).unwrap(),
        ).unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "before.txt", b"historical body").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "name projection fixture".into(),
            request_digest: None,
        }).unwrap();
        directory_fixture::checkpoint(&mut engine, &repository, "");
        let original_head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id.clone();
        let entry = engine.namespace_tree(&repository.main_branch_id).unwrap()
            .get("before.txt").unwrap().clone();
        // Disk roots expose independent file branches, not whole-history forks.
        // Neither retained scope is selected by the current directory.
        let retained = engine.create_file_branch(
            &entry.file_id, &entry.version_id,
            &storage_contract::BranchName::parse("retained").unwrap(), "admin", op(),
        ).unwrap().branch_id.unwrap();
        let file_branch = engine.create_file_branch(
            &entry.file_id, &entry.version_id,
            &storage_contract::BranchName::parse("file retained").unwrap(), "admin", op(),
        ).unwrap().branch_id.unwrap();
        assert_ne!(retained, file_branch);
        for branch in [&retained, &file_branch] {
            assert_eq!(engine.state().branches[branch].file_id.as_deref(),
                Some(entry.file_id.as_str()));
            assert_ne!(engine.state().file_selections.get(&entry.file_id), Some(branch));
        }
        let directory = engine.create_directory_branches(
            &repository.repository_id, None,
            &storage_contract::BranchName::parse("directory retained").unwrap(), "admin", op(),
        ).unwrap().result["directory_branch_id"].as_str().unwrap().to_owned();
        Self { _temporary: temporary, root, engine, repository, retained,
            file_branch, directory, original_head }
    }

    fn rename(&mut self) {
        self.engine.file_namespace(
            &self.repository.repository_id,
            crate::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "before.txt", Some("after.md"),
            ).unwrap(), "admin", op(),
        ).unwrap();
    }

    fn assert_current(&self) {
        let view = self.engine.metadata_view();
        for branch in [&self.repository.main_branch_id, &self.retained, &self.file_branch] {
            let tree = self.engine.namespace_tree(branch).unwrap();
            assert!(tree.get("before.txt").is_none());
            assert!(tree.get("after.md").is_some());
            let tree = view.namespace_tree(branch).unwrap();
            assert!(tree.get("before.txt").is_none());
            assert!(tree.get("after.md").is_some());
            let snapshot = self.engine.namespace_snapshot(branch).unwrap();
            assert!(snapshot.tree().get("before.txt").is_none());
            assert!(snapshot.tree().get("after.md").is_some());
        }
        let directories = self.engine.directory_namespace_view().unwrap();
        let entries = directories.entries(&self.directory).unwrap();
        assert!(entries.iter().any(|entry| entry.path() == "after.md"));
        assert!(entries.iter().all(|entry| entry.path() != "before.txt"));
        assert!(self.engine.namespace_tree_at(&self.retained, &self.original_head).unwrap()
            .get("before.txt").is_some());
    }
}

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

#[test]
fn current_name_projects_unselected_scopes_and_cold_open_without_rewriting_audit_paths() {
    let mut fixture = Fixture::new();
    let frozen = fixture.engine.state().commits[&fixture.original_head].clone();
    let old = fixture.engine.namespace_snapshot(&fixture.retained).unwrap();
    let witness = old.witness();
    fixture.rename();
    assert!(!fixture.engine.namespace_snapshot_matches(&witness));
    assert!(old.tree().get("before.txt").is_some());
    assert_eq!(fixture.engine.state().commits[&fixture.original_head], frozen);
    fixture.assert_current();
    let Fixture { _temporary, root, engine, repository, retained, file_branch,
        directory, original_head } = fixture;
    drop(engine);
    let fixture = Fixture { engine: Engine::open(&root).unwrap(), _temporary, root,
        repository, retained, file_branch, directory, original_head };
    fixture.assert_current();
}

#[test]
fn auth_only_publication_keeps_unchanged_named_scope_witness() {
    let mut fixture = Fixture::new();
    fixture.rename();
    let snapshot = fixture.engine.namespace_snapshot(&fixture.retained).unwrap();
    let witness = snapshot.witness();
    fixture.engine.persist_auth_state(json_codec::json!({"revision":"one"}), "admin", op())
        .unwrap();
    assert!(fixture.engine.namespace_snapshot_matches(&witness));
    fixture.assert_current();
}

#[test]
fn bypassing_state_install_fails_closed_and_preserves_original_evidence() {
    let mut fixture = Fixture::new();
    let original = Arc::clone(&fixture.engine.state);
    let bytes = crate::canonical_json(original.as_ref()).unwrap();
    fixture.engine.namespace_tree(&fixture.retained).unwrap();
    let mut replacement = original.as_ref().clone();
    replacement.control_auth = Some(json_codec::json!({"uninstalled":true}));
    fixture.engine.state = Arc::new(replacement);
    assert!(fixture.engine.namespace_tree(&fixture.retained).is_err());
    assert!(fixture.engine.namespace_snapshot(&fixture.retained).is_err());
    assert!(fixture.engine.directory_namespace_view().is_err());
    assert_eq!(crate::canonical_json(original.as_ref()).unwrap(), bytes);
    fixture.engine.named_namespaces.install_state(&mut fixture.engine.state, original);
    assert!(fixture.engine.namespace_tree(&fixture.retained).unwrap().get("before.txt").is_some());
}

#[test]
fn workspace_on_retained_branch_uses_current_name_without_creating_another_file() {
    let mut fixture = Fixture::new();
    let original = fixture.engine.namespace_tree(&fixture.retained).unwrap()
        .get("before.txt").unwrap().clone();
    fixture.rename();
    let workspace = fixture.engine.create_workspace(&fixture.retained, "admin").unwrap();
    let source = fixture.engine.workspace_namespace_entries(&workspace).unwrap();
    assert!(source.entries().iter().any(|entry| {
        entry.path() == "after.md" && entry.file_id == original.file_id
    }));
    assert!(source.entries().iter().all(|entry| entry.path() != "before.txt"));
    fixture.engine.stage_bytes(&workspace.workspace_id, "after.md", b"new branch content")
        .unwrap();
    fixture.engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: op(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "save using current name".into(),
        request_digest: None,
    }).unwrap();
    let tree = fixture.engine.namespace_tree(&fixture.retained).unwrap();
    assert_eq!(tree.get("after.md").unwrap().file_id, original.file_id);
    assert!(tree.get("before.txt").is_none());
    assert!(fixture.engine.namespace_tree_at(&fixture.retained, &fixture.original_head).unwrap()
        .get("before.txt").is_some());
}

#[test]
fn workspace_staged_under_previous_name_cannot_publish_after_container_rename() {
    let mut fixture = Fixture::new();
    let workspace = fixture.engine.create_workspace(&fixture.retained, "admin").unwrap();
    fixture.engine.stage_bytes(&workspace.workspace_id, "before.txt", b"unpublished bytes")
        .unwrap();
    fixture.rename();
    let state_before = crate::canonical_json(fixture.engine.state()).unwrap();
    let retained = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    let manifest_before = crate::canonical_json(&retained).unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: op(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "stale name must not publish".into(),
        request_digest: None,
    };
    assert!(fixture.engine.commit_workspace(request).is_err());
    assert_eq!(crate::canonical_json(fixture.engine.state()).unwrap(), state_before);
    assert_eq!(
        crate::canonical_json(&fixture.engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        manifest_before,
    );
}
