use super::*;
use crate::journal::RecordType;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};

struct Fixture {
    temp: tempfile::TempDir,
    root: std::path::PathBuf,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let config = Config::development([
            temp.path().join("primary"),
            temp.path().join("backup"),
            temp.path().join("tertiary"),
        ]);
        Self { temp, root, config }
    }
    fn initialize(&self) -> Engine {
        Engine::initialize(&self.root, self.config.clone()).unwrap()
    }
}
fn repository(e: &mut Engine, copies: u32) -> Repository {
    e.create_repository(
        &storage_contract::RepositoryName::parse("项目").unwrap(),
        "P01",
        copies,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap()
}
fn save(e: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> OperationResult {
    let w = e.create_workspace(branch, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, bytes).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "save".into(),
        request_digest: None,
    })
    .unwrap()
}
fn bytes(e: &Engine, id: &str) -> Vec<u8> {
    let mut f = e.open_content(id).unwrap();
    let mut b = Vec::new();
    f.file.read_to_end(&mut b).unwrap();
    b
}

#[test]
fn normalized_directory_rename_and_file_branch_keep_parent_closure_after_restart() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(
        &mut engine,
        &repo.main_branch_id,
        "Straße/one.txt",
        b"first",
    );
    save(
        &mut engine,
        &repo.main_branch_id,
        "STRASSE/two.txt",
        b"second",
    );
    let original = engine.list_tree(&repo.main_branch_id).unwrap();
    let file = crate::business::find_unordered(&original, "strasse/two.txt")
        .unwrap()
        .clone();
    let branch = engine
        .create_file_branch(
            &file.file_id,
            &file.version_id,
            "Draft",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch_id = branch.branch_id.unwrap();
    let snapshot = engine.list_tree(&branch_id).unwrap();
    assert_eq!(
        snapshot.len(),
        2,
        "file branches retain their normalized parent directory"
    );
    assert!(crate::business::find_unordered(&snapshot, "STRASSE").is_some());
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "strasse",
                Some("Moved"),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    drop(engine);
    let engine = Engine::open(&fixture.root).unwrap();
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    let moved = crate::business::find_unordered(&entries, "Moved/two.txt").unwrap();
    assert_eq!(moved.file_id, file.file_id);
    assert_eq!(bytes(&engine, &moved.version_id), b"second");
    assert!(crate::business::find_unordered(&entries, "Moved/one.txt").is_some());
    assert_eq!(engine.list_tree(&branch_id).unwrap().len(), 2);
    assert!(engine.check().unwrap().issues.is_empty());
}

#[test]
fn rejected_directory_upload_does_not_consume_an_immutable_generation() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    save(
        &mut engine,
        &repo.main_branch_id,
        "folder/child.txt",
        b"child",
    );
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let item = new_id("entry");
    let source = fixture.temp.path().join("upload.txt");
    fs::write(&source, b"retryable upload").unwrap();
    assert_eq!(
        engine
            .stage_upload(&workspace.workspace_id, &item, "folder", &source, 1)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::IsDirectory
    );
    let staging = engine.workspace_data_directory(&workspace).unwrap();
    assert!(fs::read_dir(staging).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|suffix| suffix != "full")
    }));
    let staged = engine
        .stage_upload(&workspace.workspace_id, &item, "valid.txt", &source, 1)
        .unwrap();
    assert_eq!(staged.dirty_generation, 1);
    assert_eq!(fs::read(staged.source_path).unwrap(), b"retryable upload");
}

#[test]
fn direct_namespace_cannot_write_an_inactive_directory_branch() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .state = storage_contract::BranchState::Archived;
    let before = canonical_json(engine.state()).unwrap();
    let error = engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Mkdir,
                "unpublished",
                None,
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::BranchNotActive);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}

#[test]
fn indexed_commit_comparison_preserves_normalized_identity_and_change_order() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    save(
        &mut engine,
        &repo.main_branch_id,
        "keep.txt",
        b"unchanged bytes",
    );
    let left = save(&mut engine, &repo.main_branch_id, "remove.txt", b"removed")
        .commit_id
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "remove.txt",
                None,
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "keep.txt",
                Some("KEEP.txt"),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let right = save(&mut engine, &repo.main_branch_id, "added.txt", b"added")
        .commit_id
        .unwrap();
    let comparison = engine.compare_commits(&left, &right).unwrap();
    let changes = comparison.changes().collect::<Vec<_>>();
    assert_eq!(changes.len(), 3);
    let CommitChange::Modified { before, after } = changes[0] else {
        panic!("expected modified")
    };
    assert_eq!(after.path(), "KEEP.txt");
    assert_eq!(before.version_id, after.version_id);
    let CommitChange::Removed { before } = changes[1] else {
        panic!("expected removed")
    };
    assert_eq!(before.path(), "remove.txt");
    let CommitChange::Added { after } = changes[2] else {
        panic!("expected added")
    };
    assert_eq!(after.path(), "added.txt");
}

#[test]
fn modified_local_maintenance_plan_cannot_execute_a_different_preview_after_restart() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"one");
    let first = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"two");
    let second = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"three");
    let mut plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![first.clone()], "admin")
        .unwrap();
    let plan_path = fixture
        .root
        .join("plans")
        .join(format!("{}.json", plan.plan_id));
    plan.input["version_ids"] = json_codec::json!([second]);
    crate::disk::write_json(&plan_path, &plan, true).unwrap();
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    let before = engine.state().sequence;
    assert_eq!(
        engine
            .commit_plan(
                &plan.plan_id,
                "admin",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PlanInvalid
    );
    assert_eq!(engine.state().sequence, before);
    assert_eq!(bytes(&engine, &first), b"one");
    assert_eq!(bytes(&engine, &second), b"two");
    plan.digest = object_hash("plan", &plan.input).unwrap();
    plan.plan_id = new_id("plan");
    crate::disk::write_json(&plan_path, &plan, true).unwrap();
    assert_eq!(
        engine
            .plan(plan_path.file_stem().unwrap().to_str().unwrap())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PlanInvalid
    );
}

#[test]
fn file_container_rename_replays_only_after_decision_and_keeps_all_version_bytes() {
    for (fault, committed) in [
        ("after_prepare_3", false),
        ("after_accept_1", true),
        ("after_accept_2", true),
        ("after_container_names", true),
    ] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        save(
            &mut engine,
            &repo.main_branch_id,
            "report.txt",
            b"version one",
        );
        save(
            &mut engine,
            &repo.main_branch_id,
            "report.txt",
            b"version two",
        );
        let entry = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
        let versions = engine.file_history(&entry.file_id).unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[0].storage_name.starts_with("v000001_"));
        assert!(versions[1].storage_name.starts_with("v000002_"));
        let operation = storage_contract::OperationId::generate();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Rename,
                        "report.txt",
                        Some("renamed.txt")
                    )
                    .unwrap(),
                    "admin",
                    operation
                )
                .unwrap_err()
                .code(),
            if committed {
                storage_contract::ErrorCode::InDoubt
            } else {
                storage_contract::ErrorCode::InjectedFailure
            }
        );
        if fault != "after_container_names" {
            for disk in &fixture.config.disks {
                assert!(
                    disk.root
                        .join("histories")
                        .join(&repo.history_tree_id)
                        .join("file-containers")
                        .join(&entry.file_id)
                        .join("namespace")
                        .join("report.txt")
                        .is_dir()
                );
            }
        }
        drop(engine);
        let mut engine = Engine::open(&fixture.root).unwrap();
        let expected = if committed {
            "renamed.txt"
        } else {
            "report.txt"
        };
        assert_eq!(
            engine.list_tree(&repo.main_branch_id).unwrap()[0].name(),
            expected
        );
        assert_eq!(engine.state().file_names[&entry.file_id], expected);
        for disk in &fixture.config.disks {
            let stable = disk
                .root
                .join("histories")
                .join(&repo.history_tree_id)
                .join("file-containers")
                .join(&entry.file_id);
            assert_eq!(fs::read_dir(&stable).unwrap().count(), 1);
            let named = stable.join("namespace").join(expected);
            assert!(named.is_dir());
            let files = fs::read_dir(named.join("versions"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(files.len(), 2);
            assert!(
                files
                    .iter()
                    .all(|path| fs::metadata(path).unwrap().nlink() == 1)
            );
        }
        assert_eq!(bytes(&engine, &versions[0].version_id), b"version one");
        assert_eq!(bytes(&engine, &versions[1].version_id), b"version two");
        if committed {
            let before = engine.state().sequence;
            engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Rename,
                        "report.txt",
                        Some("renamed.txt"),
                    )
                    .unwrap(),
                    "admin",
                    operation,
                )
                .unwrap();
            assert_eq!(
                engine.state().sequence,
                before,
                "replaying the rename cannot publish twice"
            );
        }
    }
}

#[test]
fn file_branches_register_then_write_and_switch_only_one_file_and_survive_reopen() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(
        &mut engine,
        &repo.main_branch_id,
        "docs/one.txt",
        b"one main",
    );
    save(
        &mut engine,
        &repo.main_branch_id,
        "docs/two.txt",
        b"two unchanged",
    );
    let before = engine.list_tree(&repo.main_branch_id).unwrap();
    let one = before
        .iter()
        .find(|e| e.path == "docs/one.txt")
        .unwrap()
        .clone();
    let two = before
        .iter()
        .find(|e| e.path == "docs/two.txt")
        .unwrap()
        .clone();
    let versions = engine.state().versions.len();
    let operation = storage_contract::OperationId::generate();
    let branch = engine
        .create_file_branch(&one.file_id, &one.version_id, "draft", "admin", operation)
        .unwrap();
    let branch_id = branch.result["branch_id"].as_str().unwrap().to_string();
    assert_eq!(
        engine
            .set_smb_binding(
                &repo.repository_id,
                &branch_id,
                true,
                engine.state().histories[&repo.history_tree_id].generation,
                "admin",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchScope
    );
    assert_eq!(
        engine.state().versions.len(),
        versions,
        "registration does not create or copy a content version"
    );
    let snapshot = engine.list_tree(&branch_id).unwrap();
    assert_eq!(
        snapshot
            .iter()
            .filter(|e| e.kind == NamespaceEntryKind::File)
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .iter()
            .find(|e| e.kind == NamespaceEntryKind::File)
            .unwrap()
            .file_id,
        one.file_id
    );
    let scope = engine.create_workspace(&branch_id, "admin").unwrap();
    assert_eq!(
        engine
            .stage_bytes(&scope.workspace_id, "docs/two.txt", b"bad")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchScope
    );
    assert_eq!(
        engine
            .namespace_operation(
                &scope.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Mkdir,
                    "other",
                    None
                )
                .unwrap()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchScope
    );
    engine
        .abort_workspace(&scope.workspace_id, "admin")
        .unwrap();
    save(&mut engine, &branch_id, "docs/one.txt", b"one draft");
    assert_eq!(
        bytes(
            &engine,
            &engine
                .list_tree(&repo.main_branch_id)
                .unwrap()
                .iter()
                .find(|e| e.file_id == one.file_id)
                .unwrap()
                .version_id
        ),
        b"one main",
        "editing an unselected branch cannot change the visible file"
    );
    engine
        .select_file_branch(
            &one.file_id,
            &branch_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save(
        &mut engine,
        &branch_id,
        "docs/one.txt",
        b"one selected draft",
    );
    let visible = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(
        visible
            .iter()
            .find(|e| e.file_id == two.file_id)
            .unwrap()
            .version_id,
        two.version_id
    );
    assert_eq!(
        bytes(
            &engine,
            &visible
                .iter()
                .find(|e| e.file_id == one.file_id)
                .unwrap()
                .version_id
        ),
        b"one selected draft"
    );
    let main = engine
        .state()
        .branches
        .values()
        .find(|b| b.file_id.as_deref() == Some(&one.file_id) && b.name == "main")
        .unwrap()
        .branch_id
        .clone();
    engine
        .select_file_branch(
            &one.file_id,
            &main,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        bytes(
            &engine,
            &engine
                .list_tree(&repo.main_branch_id)
                .unwrap()
                .iter()
                .find(|e| e.file_id == one.file_id)
                .unwrap()
                .version_id
        ),
        b"one main"
    );
    assert_eq!(
        engine
            .create_file_branch(
                &one.file_id,
                &two.version_id,
                "wrong file",
                "admin",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::VersionNotFound
    );
    assert_eq!(
        engine
            .select_file_branch(
                &one.file_id,
                &branch_id,
                "",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(engine.state().file_selections[&one.file_id], main);
    assert_eq!(
        bytes(
            &engine,
            &engine
                .list_tree(&branch_id)
                .unwrap()
                .iter()
                .find(|e| e.kind == NamespaceEntryKind::File)
                .unwrap()
                .version_id
        ),
        b"one selected draft"
    );
    assert_eq!(bytes(&engine, &two.version_id), b"two unchanged");
    assert!(engine.check().unwrap().issues.is_empty());
    let package = fixture.temp.path().join("file-branch-package");
    fs::set_permissions(
        fixture.temp.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    engine
        .set_file_note(
            &one.file_id,
            None,
            "file container note",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .set_file_note(
            &one.file_id,
            Some(&one.version_id),
            "original version note",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let target = Fixture::new();
    let mut recovered = target.initialize();
    recovered
        .restore_history_package(
            &package,
            "P01",
            3,
            "rescuer",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(recovered.state().file_selections[&one.file_id], main);
    assert_eq!(
        recovered.state().file_notes[&one.file_id],
        "file container note"
    );
    assert_eq!(
        recovered.state().version_notes[&one.version_id],
        "original version note"
    );
    assert_eq!(
        bytes(
            &recovered,
            &recovered
                .list_tree(&branch_id)
                .unwrap()
                .iter()
                .find(|e| e.kind == NamespaceEntryKind::File)
                .unwrap()
                .version_id
        ),
        b"one selected draft"
    );
}

#[test]
fn directory_writes_update_only_the_selected_file_branch() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"original");
    let file = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let result = engine
        .create_file_branch(
            &file.file_id,
            &file.version_id,
            "experiment",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = result.result["branch_id"].as_str().unwrap().to_string();
    save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"current main",
    );
    let pending = &engine.list_tree(&branch).unwrap()[0];
    assert_eq!(pending.version_id, file.version_id);
    assert_eq!(pending.size, file.size);
    assert_eq!(bytes(&engine, &pending.version_id), b"original");
    assert_eq!(
        engine.pending_file_source(&branch, &file.file_id),
        Some(file.version_id.as_str())
    );
    let main = engine.state().file_selections[&file.file_id].clone();
    assert_eq!(
        bytes(&engine, &engine.list_tree(&main).unwrap()[0].version_id),
        b"current main"
    );
    let change = NamespaceOperation::parse(
        storage_contract::NamespaceOperationKind::Rename,
        "file.txt",
        Some("renamed.txt"),
    )
    .unwrap();
    let operation = storage_contract::OperationId::generate();
    let result = engine
        .file_namespace(&repo.repository_id, change.clone(), "admin", operation)
        .unwrap();
    assert_eq!(
        engine
            .file_namespace(&repo.repository_id, change, "admin", operation)
            .unwrap()
            .operation_id,
        result.operation_id
    );
    engine
        .select_file_branch(
            &file.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let visible = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(visible[0].path, "renamed.txt");
    assert_eq!(visible[0].version_id, file.version_id);
    assert_eq!(visible[0].size, file.size);
    save(
        &mut engine,
        &repo.main_branch_id,
        "renamed.txt",
        b"first branch content",
    );
    let written = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(
        bytes(&engine, &written[0].version_id),
        b"first branch content"
    );
    assert!(engine.pending_file_source(&branch, &file.file_id).is_none());
}

#[test]
fn directory_file_branch_batch_is_recursive_atomic_and_idempotent() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "inside/a.txt", b"A main");
    save(
        &mut engine,
        &repo.main_branch_id,
        "inside/nested/b.txt",
        b"B main",
    );
    save(
        &mut engine,
        &repo.main_branch_id,
        "outside/c.txt",
        b"C unchanged",
    );
    let before = engine.list_tree(&repo.main_branch_id).unwrap();
    let outside = before
        .iter()
        .find(|e| e.path == "outside/c.txt")
        .unwrap()
        .clone();
    let versions = engine.state().versions.len();
    let operation = storage_contract::OperationId::generate();
    let created = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("inside").unwrap()),
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(created.result["count"], "2");
    assert_eq!(created.result["copied_files"], "0");
    assert_eq!(created.result["placeholder_files"], "0");
    assert!(created.result.get("branch_id").is_none());
    assert_eq!(engine.state().versions.len(), versions);
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("inside").unwrap()),
                &storage_contract::BranchName::parse("draft").unwrap(),
                "admin",
                operation
            )
            .unwrap()
            .result,
        created.result
    );
    assert_eq!(engine.state().sequence, sequence);
    for branch in created.result["branches"].as_array().unwrap() {
        let id = branch["branch_id"].as_str().unwrap();
        let files = engine.list_tree(id).unwrap();
        assert_eq!(
            files
                .iter()
                .filter(|e| e.kind == NamespaceEntryKind::File)
                .count(),
            1
        );
        let file = files
            .iter()
            .find(|e| e.kind == NamespaceEntryKind::File)
            .unwrap();
        save(
            &mut engine,
            id,
            &file.path,
            format!("draft {}", file.path).as_bytes(),
        );
    }
    let selection = storage_contract::OperationId::generate();
    let selected = engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("inside").unwrap()),
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            selection,
        )
        .unwrap();
    assert_eq!(selected.result["count"], "2");
    let tree = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(
        tree.iter()
            .find(|e| e.file_id == outside.file_id)
            .unwrap()
            .version_id,
        outside.version_id
    );
    assert_eq!(bytes(&engine, &outside.version_id), b"C unchanged");
    for file in tree
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File && e.path.starts_with("inside/"))
    {
        assert_eq!(
            bytes(&engine, &file.version_id),
            if file.path.ends_with("a.txt") {
                b"A main".as_slice()
            } else {
                b"B main".as_slice()
            }
        );
    }
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(
        engine
            .select_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("inside").unwrap()),
                &storage_contract::BranchName::parse("draft").unwrap(),
                "admin",
                selection
            )
            .unwrap()
            .result,
        selected.result
    );
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("inside").unwrap()),
                &storage_contract::BranchName::parse("draft").unwrap(),
                "admin",
                operation
            )
            .unwrap()
            .result,
        created.result
    );
    assert_eq!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .find(|e| e.file_id == outside.file_id)
            .unwrap()
            .version_id,
        outside.version_id
    );
}

#[test]
fn directory_batch_conflict_and_new_file_reject_without_partial_changes() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "folder/a.txt", b"A");
    save(&mut engine, &repo.main_branch_id, "folder/b.txt", b"B");
    let files = engine.list_tree(&repo.main_branch_id).unwrap();
    let b = files.iter().find(|e| e.path == "folder/b.txt").unwrap();
    engine
        .create_file_branch(
            &b.file_id,
            &b.version_id,
            "taken",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    // File and directory branch names are separate namespaces. Registering
    // this composition reuses the already selected shared file branches.
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("taken").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("folder").unwrap()),
                &storage_contract::BranchName::parse("taken").unwrap(),
                "admin",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::NameCollision
    );
    assert_eq!(
        canonical_json(engine.state()).unwrap(),
        before,
        "a late member conflict cannot create earlier branches or payloads"
    );
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("complete").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save(
        &mut engine,
        &repo.main_branch_id,
        "folder/new.txt",
        b"new file has no draft",
    );
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("complete").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        !engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|e| e.path == "folder/new.txt")
    );
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("folder").unwrap()),
                &storage_contract::BranchName::parse("missing actor").unwrap(),
                "",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let lost = engine.state().versions[&b.version_id].clone();
    for disk in &fixture.config.disks {
        let directory = crate::disk::version_dir(disk, &lost.history_tree_id, &lost.version_id);
        fs::remove_file(crate::container_storage::content_path(&directory, &lost).unwrap())
            .unwrap();
    }
    let version_count = engine.state().versions.len();
    let placeholders = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("unavailable").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(placeholders.result["placeholder_files"], "0");
    assert_eq!(placeholders.result["copied_files"], "0");
    assert_eq!(engine.state().versions.len(), version_count);
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("unavailable").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .all(|entry| !entry.version_id.is_empty()
                && engine.state().versions.contains_key(&entry.version_id))
    );
}

#[test]
fn full_versions_independent_rebuild_and_offline_export() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    save(&mut e, &r.main_branch_id, "资料/报告.txt", b"first");
    let first = e
        .list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == NamespaceEntryKind::File)
        .unwrap();
    save(
        &mut e,
        &r.main_branch_id,
        "资料/报告.txt",
        b"second complete",
    );
    let second = e
        .list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == NamespaceEntryKind::File)
        .unwrap();
    assert_ne!(first.version_id, second.version_id);
    assert_eq!(bytes(&e, &first.version_id), b"first");
    assert_eq!(bytes(&e, &second.version_id), b"second complete");
    let v = &e.state().versions[&second.version_id];
    let mut inodes = Vec::new();
    for d in &f.config.disks {
        let p = crate::container_storage::content_path(
            &crate::disk::version_dir(d, &v.history_tree_id, &v.version_id),
            v,
        )
        .unwrap();
        let m = fs::metadata(p).unwrap();
        assert_eq!(m.nlink(), 1);
        inodes.push(m.ino());
    }
    inodes.sort();
    inodes.dedup();
    assert_eq!(inodes.len(), 3);
    let seq = e.state().sequence;
    drop(e);
    fs::remove_file(f.root.join("index.sqlite")).unwrap();
    let e = Engine::open(&f.root).unwrap();
    assert_eq!(e.state().sequence, seq);
    assert_eq!(bytes(&e, &second.version_id), b"second complete");
    drop(e);
    let export = f.temp.path().join("export.txt");
    offline_export(&f.config.disks[2].root, &second.version_id, &export).unwrap();
    assert_eq!(fs::read(export).unwrap(), b"second complete");
}
#[test]
fn written_branch_survives_source_purge_and_merge_is_metadata_only() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    let first = save(&mut e, &r.main_branch_id, "a.txt", b"A");
    let old = e.list_tree(&r.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let b = e
        .create_branch(
            first.commit_id.as_deref().unwrap(),
            &storage_contract::BranchName::parse("独立分支").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(e.list_tree(&b.branch_id).unwrap()[0].version_id, old);
    save(&mut e, &b.branch_id, "a.txt", b"written independently");
    let fork = e.list_tree(&b.branch_id).unwrap()[0].version_id.clone();
    assert_ne!(old, fork);
    save(&mut e, &r.main_branch_id, "a.txt", b"B");
    let before = e.list_tree(&r.main_branch_id).unwrap();
    let target = e.state().branches[&r.main_branch_id].head_commit_id.clone();
    let count = e.state().versions.len();
    e.add_merge_link(
        &b.branch_id,
        &b.head_commit_id,
        &r.main_branch_id,
        &target,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(e.state().versions.len(), count);
    assert_eq!(
        e.list_tree(&r.main_branch_id).unwrap()[0].version_id,
        before[0].version_id
    );
    let plan = e
        .create_purge_plan(&r.history_tree_id, vec![old.clone()], "admin")
        .unwrap();
    e.commit_plan(
        &plan.plan_id,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(
        e.open_content(&old).err().unwrap().code(),
        storage_contract::ErrorCode::ContentPurged
    );
    assert_eq!(bytes(&e, &fork), b"written independently");
    drop(e);
    let e = Engine::open(&f.root).unwrap();
    assert_eq!(bytes(&e, &fork), b"written independently");
    assert_eq!(
        e.open_content(&old).err().unwrap().code(),
        storage_contract::ErrorCode::ContentPurged
    );
}
#[test]
fn faults_replay_same_operation_and_never_publish_early() {
    for point in [
        "after_accept_1",
        "after_accept_2",
        "after_accept_3",
        "after_finalize",
        "after_commit_proofs",
        "after_publication_1",
        "after_publication_3",
        "after_local_publications",
    ] {
        let f = Fixture::new();
        let mut e = f.initialize();
        let r = repository(&mut e, 3);
        let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
        e.stage_bytes(&w.workspace_id, "文件.txt", b"durable")
            .unwrap();
        let request = CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: point.into(),
            request_digest: None,
        };
        e.inject_fault_once(point);
        assert_eq!(
            e.commit_workspace(request.clone()).unwrap_err().code(),
            storage_contract::ErrorCode::InDoubt,
            "{point}"
        );
        assert!(e.is_read_only());
        assert_eq!(
            e.operation_owner(request.operation_id).unwrap().as_deref(),
            Some("admin")
        );
        drop(e);
        let mut e = Engine::open(&f.root).unwrap();
        let result = e.operation(request.operation_id).unwrap();
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::Published,
            "{point}"
        );
        assert_eq!(
            e.commit_workspace(request.clone()).unwrap().commit_id,
            result.commit_id
        );
        let files = e.list_tree(&r.main_branch_id).unwrap();
        assert_eq!(bytes(&e, &files[0].version_id), b"durable");
        assert_eq!(e.state().versions.len(), 1);
    }
}
#[test]
fn disjoint_workspaces_rebase_but_same_file_is_owned() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    let a = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    let b = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&a.workspace_id, "a", b"a").unwrap();
    assert_eq!(
        e.stage_bytes(&b.workspace_id, "a", b"b")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBusy
    );
    e.stage_bytes(&b.workspace_id, "b", b"b").unwrap();
    for w in [a, b] {
        e.commit_workspace(CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: "disjoint".into(),
            request_digest: None,
        })
        .unwrap();
    }
    assert_eq!(e.list_tree(&r.main_branch_id).unwrap().len(), 2);
}

#[test]
fn durable_workspace_and_plan_preparations_require_an_audit_actor() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let state = canonical_json(engine.state()).unwrap();
    let workspaces = engine.workspaces.len();
    let plans = engine.state.plans.len();
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    assert_eq!(
        engine
            .create_historical_workspace(&repo.main_branch_id, &head, "")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    assert_eq!(
        engine
            .create_purge_plan(&repo.history_tree_id, Vec::new(), "")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    assert_eq!(engine.workspaces.len(), workspaces);
    assert_eq!(engine.state.plans.len(), plans);
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
}

#[test]
fn workspace_actor_binding_and_idempotency_conflicts() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "a", b"private").unwrap();
    let request = CommitRequest {
        workspace_id: w.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head.clone(),
        fencing_token: w.fencing_token,
        message: String::new(),
        request_digest: None,
    };
    let state_before = canonical_json(e.state()).unwrap();
    let workspace_before = canonical_json(&e.workspace(&w.workspace_id).unwrap()).unwrap();
    // A correct fence does not permit changing the immutable operation actor.
    let mut changed_actor = request.clone();
    changed_actor.principal = "different-actor".into();
    assert_eq!(
        e.commit_workspace(changed_actor.clone())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PermissionDenied
    );
    assert_eq!(
        e.prepare_workspace_commit(changed_actor, None)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::PermissionDenied
    );
    // Conversely, the correct actor does not bypass a stale or forged fence.
    let mut changed_fence = request.clone();
    changed_fence.fencing_token = changed_fence.fencing_token.checked_add(1).unwrap();
    assert_eq!(
        e.commit_workspace(changed_fence.clone())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FencingTokenInvalid
    );
    assert_eq!(
        e.prepare_workspace_commit(changed_fence, None)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::FencingTokenInvalid
    );
    assert_eq!(canonical_json(e.state()).unwrap(), state_before);
    assert_eq!(
        canonical_json(&e.workspace(&w.workspace_id).unwrap()).unwrap(),
        workspace_before
    );
    e.commit_workspace(request).unwrap();
    let op = storage_contract::OperationId::generate();
    let b = e
        .create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            1,
            "admin",
            op,
        )
        .unwrap();
    assert_eq!(
        e.create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            1,
            "admin",
            op
        )
        .unwrap()
        .repository_id,
        b.repository_id
    );
    assert_eq!(
        e.create_repository(
            &storage_contract::RepositoryName::parse("other").unwrap(),
            "P01",
            1,
            "admin",
            op
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
}
#[test]
fn active_reads_protect_history_and_split_remaps_full_payloads() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    let first = save(&mut e, &r.main_branch_id, "a", b"v1");
    let old = e.list_tree(&r.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut e, &r.main_branch_id, "a", b"v2");
    let lease = e.open_content(&old).unwrap();
    assert_eq!(
        e.create_purge_plan(&r.history_tree_id, vec![old.clone()], "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    drop(lease);
    let plan = e
        .create_split_plan(
            &r.history_tree_id,
            first.commit_id.as_deref().unwrap(),
            "admin",
        )
        .unwrap();
    let result = e
        .commit_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let new_history = result.history_tree_id.unwrap();
    assert_ne!(new_history, r.history_tree_id);
    let graph = e.graph(&new_history).unwrap();
    assert!(
        graph
            .commits
            .iter()
            .find(|c| c.kind == "split_root")
            .unwrap()
            .parent_commit_ids
            .is_empty()
    );
    for c in &graph.commits {
        for entry in &c.entries {
            assert_eq!(
                e.state().versions[&entry.version_id].history_tree_id,
                new_history
            );
            assert!(bytes(&e, &entry.version_id).starts_with(b"v"));
        }
    }
    drop(e);
    Engine::open(&f.root).unwrap();
}
#[test]
fn fixed_route_capacity_does_not_fallback_and_second_writer_is_rejected() {
    let mut f = Fixture::new();
    f.config.disks[1].capacity_bytes = Some(1);
    let mut e = f.initialize();
    assert_eq!(
        Engine::open(&f.root).err().unwrap().code(),
        storage_contract::ErrorCode::WriterAlreadyActive
    );
    let r = repository(&mut e, 3);
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "a", b"too large for assigned backup")
        .unwrap();
    let err = e
        .commit_workspace(CommitRequest {
            workspace_id: w.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(err.code(), storage_contract::ErrorCode::NoSpace);
    assert!(e.list_tree(&r.main_branch_id).unwrap().is_empty());
    assert!(!e.workspace(&w.workspace_id).unwrap().files.is_empty());
}
#[test]
fn corrupt_log_is_not_skipped_and_torn_tail_is_bounded() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    save(&mut e, &r.main_branch_id, "a", b"a");
    drop(e);
    let log = f.config.disks[0]
        .root
        .join("metadata/application-log/journal.bin");
    fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(b"ZXS")
        .unwrap();
    let mut e = Engine::open(&f.root).unwrap();
    save(&mut e, &r.main_branch_id, "b", b"b");
    drop(e);
    let mut data = fs::read(&log).unwrap();
    data[20] ^= 1;
    fs::write(log, data).unwrap();
    assert!(Engine::open(&f.root).is_err());
}
#[test]
fn published_success_survives_later_content_damage() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    let op = save(&mut e, &r.main_branch_id, "a", b"verified");
    let entry = e.list_tree(&r.main_branch_id).unwrap()[0].clone();
    let version = e.state().versions[&entry.version_id].clone();
    drop(e);
    let content = crate::container_storage::content_path(
        &crate::disk::version_dir(
            &f.config.disks[0],
            &version.history_tree_id,
            &version.version_id,
        ),
        &version,
    )
    .unwrap();
    fs::remove_file(content).unwrap();
    let e = Engine::open(&f.root).unwrap();
    let result = e.operation(op.operation_id).unwrap();
    assert_eq!(
        result.protection_at_publish,
        storage_contract::ProtectionAtPublish::TargetMet
    );
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(result.current_health.as_str(), "UNAVAILABLE");
}

#[test]
fn explicit_upload_generations_and_sealed_wire_requests_are_stable() {
    use json_codec::json;
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    let input = f.temp.path().join("upload");
    fs::write(&input, b"generation one hundred").unwrap();
    let item = new_id("entry");
    let staged = e
        .stage_upload(&w.workspace_id, &item, "a", &input, 100)
        .unwrap();
    assert_eq!(staged.dirty_generation, 100);
    assert_eq!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 100)
            .unwrap()
            .source_path,
        staged.source_path
    );
    assert_eq!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 99)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::StaleUploadGeneration
    );
    fs::write(&input, b"different").unwrap();
    assert_eq!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 100)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::UploadGenerationConflict
    );
    let next = e
        .stage_upload(&w.workspace_id, &item, "a", &input, 200)
        .unwrap();
    assert_eq!(next.dirty_generation, 200);
    assert_eq!(
        fs::read(staged.source_path).unwrap(),
        b"generation one hundred"
    );
    let mut body = e
        .seal_workspace(&w.workspace_id, "admin")
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    body["file_placement"][&item]["target_copies"] = json!(1);
    body["request_digest"] = json!(commit_request_digest(&w.workspace_id, &body).unwrap());
    let request = CommitRequest {
        workspace_id: w.workspace_id.clone(),
        operation_id: body["operation_id"].as_str().unwrap().parse().unwrap(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: String::new(),
        request_digest: None,
    };
    let mut wrong = body.clone();
    wrong["request_digest"] = json!("0".repeat(64));
    assert_eq!(
        e.commit_workspace_wire(request.clone(), public_envelope(wrong))
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RequestDigestMismatch
    );
    assert!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 201)
            .is_err()
    );
    let result = e
        .commit_workspace_wire(request.clone(), public_envelope(body.clone()))
        .unwrap();
    let version = e.list_tree(&r.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    assert_eq!(e.state().placements[&version].route_slot_ids, ["P01"]);
    save(&mut e, &r.main_branch_id, "b", b"later head");
    assert_eq!(
        e.commit_workspace_wire(request, public_envelope(body))
            .unwrap()
            .commit_id,
        result.commit_id
    );
    drop(e);
    let mut e = Engine::open(&f.root).unwrap();
    assert_eq!(
        e.workspace(&w.workspace_id).unwrap().state.as_str(),
        "committed"
    );
    assert!(e.create_workspace(&r.main_branch_id, "admin").is_ok());
}

#[test]
fn system_disk_restore_recovers_auth_and_unsubmitted_full_bytes() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    e.persist_auth_state(
        json_codec::json!({"users":{"admin":{"password_hash":"test-hash-only"}}}),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "pending", b"not yet published")
        .unwrap();
    drop(e);
    fs::remove_dir_all(&f.root).unwrap();
    let recovered = f.temp.path().join("replacement-system");
    let mut e = Engine::restore_from_disks(
        &recovered,
        &f.config
            .disks
            .iter()
            .map(|d| d.root.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(
        e.auth_state().unwrap()["users"]["admin"]["password_hash"],
        "test-hash-only"
    );
    let pending = e.workspace(&w.workspace_id).unwrap();
    assert_eq!(pending.state.as_str(), "open");
    assert_eq!(
        fs::read(&pending.files.values().next().unwrap().source_path).unwrap(),
        b"not yet published"
    );
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: String::new(),
        request_digest: None,
    })
    .unwrap();
    assert_eq!(
        bytes(&e, &e.list_tree(&r.main_branch_id).unwrap()[0].version_id),
        b"not yet published"
    );
}

#[test]
fn mkdir_and_child_upload_publish_together_and_cancellation_never_undoes_execution() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.namespace_operation(
        &w.workspace_id,
        NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Mkdir,
            "文档",
            None,
        )
        .unwrap(),
    )
    .unwrap();
    e.stage_bytes(&w.workspace_id, "文档/说明.txt", b"first")
        .unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: String::new(),
        request_digest: None,
    })
    .unwrap();
    assert_eq!(e.list_tree(&r.main_branch_id).unwrap().len(), 2);
    let old = e
        .list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id;
    save(&mut e, &r.main_branch_id, "文档/说明.txt", b"second");
    let plan = e
        .create_purge_plan(&r.history_tree_id, vec![old.clone()], "admin")
        .unwrap();
    let cancel = storage_contract::OperationId::generate();
    let result = e.cancel_plan(&plan.plan_id, "admin", cancel).unwrap();
    assert_eq!(result.result["state"], "cancelled");
    assert_eq!(
        e.cancel_plan(&plan.plan_id, "admin", cancel)
            .unwrap()
            .sequence,
        result.sequence
    );
    assert_eq!(
        e.commit_plan(&plan.plan_id, "admin", cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    let plan = e
        .create_purge_plan(&r.history_tree_id, vec![old], "admin")
        .unwrap();
    e.commit_plan(
        &plan.plan_id,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(
        e.cancel_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::PlanClosed
    );
}

#[test]
fn managed_io_rejects_symlink_ancestors_and_preserves_export_directory_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    save(&mut e, &r.main_branch_id, "a", b"whole");
    let version = e.list_tree(&r.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let public = f.temp.path().join("exports");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    offline_export(&f.config.disks[0].root, &version, &public.join("a")).unwrap();
    assert_eq!(
        fs::metadata(&public).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let link = f.temp.path().join("linked");
    symlink(&public, &link).unwrap();
    assert_eq!(
        crate::disk::open_regular(&link.join("a"))
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert!(crate::disk::write_atomic(&link.join("attack"), b"no", true).is_err());
    assert!(!public.join("attack").exists());
    assert_eq!(
        fs::metadata(&public).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn conditional_directory_policies_change_only_future_placements() {
    let mut f = Fixture::new();
    f.config.disks.push(DiskConfig {
        slot_id: "P02".into(),
        disk_id: new_id("disk"),
        root: f.temp.path().join("second-primary"),
        level: 1,
        upstream_slot_id: None,
        metadata_member: false,
        binding_generation: 1,
        capacity_bytes: None,
    });
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    save(
        &mut e,
        &r.main_branch_id,
        "docs/a",
        b"three complete originals",
    );
    let old = e
        .list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|f| f.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id;
    let policy = new_id("policy");
    e.update_protection_policy(crate::ProtectionPolicyUpdate {
        policy_id: &policy,
        repository_id: &r.repository_id,
        path_prefix: "DOCS",
        target_copies: 1,
        expected_generation: 0,
        principal: "admin",
        operation_id: storage_contract::OperationId::generate(),
    })
    .unwrap();
    let rule = new_id("rule");
    e.update_placement_rule(crate::PlacementRuleUpdate {
        rule_id: &rule,
        repository_id: &r.repository_id,
        path_prefix: "Docs",
        primary_slot_id: "P02",
        expected_generation: 0,
        principal: "admin",
        operation_id: storage_contract::OperationId::generate(),
    })
    .unwrap();
    assert_eq!(
        e.update_protection_policy(crate::ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &r.repository_id,
            path_prefix: "DOCS",
            target_copies: 2,
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::GenerationMismatch
    );
    let other = e
        .create_repository(
            &storage_contract::RepositoryName::parse("other").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let before_rejected_updates = canonical_json(e.state()).unwrap();
    assert_eq!(
        e.update_protection_policy(crate::ProtectionPolicyUpdate {
            policy_id: &new_id("policy"),
            repository_id: &r.repository_id,
            path_prefix: "secret",
            target_copies: 1,
            expected_generation: 0,
            principal: "",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    // The sole administrator can manage both repositories, but an existing
    // policy/rule identity cannot be rebound to a different repository.
    assert_eq!(
        e.update_protection_policy(crate::ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &other.repository_id,
            path_prefix: "DOCS",
            target_copies: 1,
            expected_generation: 1,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::RequestBindingMismatch
    );
    assert_eq!(
        e.update_placement_rule(crate::PlacementRuleUpdate {
            rule_id: &rule,
            repository_id: &other.repository_id,
            path_prefix: "DOCS",
            primary_slot_id: "P02",
            expected_generation: 1,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::RequestBindingMismatch
    );
    assert_eq!(canonical_json(e.state()).unwrap(), before_rejected_updates);
    save(&mut e, &r.main_branch_id, "docs/a", b"new primary");
    let current = e
        .list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|f| f.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id;
    assert_eq!(
        e.state().placements[&old].route_slot_ids,
        ["P01", "B01", "C01"]
    );
    assert_eq!(e.state().placements[&current].route_slot_ids, ["P02"]);
    assert_eq!(
        e.new_file_placement(&r.repository_id, "docs-extra/a")
            .unwrap(),
        ("P01".into(), 3)
    );
    drop(e);
    let e = Engine::open(&f.root).unwrap();
    assert_eq!(
        e.new_file_placement(&r.repository_id, "DOCS/b").unwrap(),
        ("P02".into(), 1)
    );
    assert_eq!(bytes(&e, &old), b"three complete originals");
}

#[test]
fn overlapping_namespace_and_upload_never_reports_a_successful_disappearing_file() {
    for delete_first in [true, false] {
        let f = Fixture::new();
        let mut e = f.initialize();
        let r = repository(&mut e, 1);
        save(&mut e, &r.main_branch_id, "docs/original", b"original");
        let head = e.state().branches[&r.main_branch_id].head_commit_id.clone();
        let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
        let delete = NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::DeleteTree,
            "docs",
            None,
        )
        .unwrap();
        if delete_first {
            e.namespace_operation(&w.workspace_id, delete.clone())
                .unwrap();
        }
        let staged = e
            .stage_bytes(&w.workspace_id, "docs/new", b"must remain staged")
            .unwrap();
        if !delete_first {
            e.namespace_operation(&w.workspace_id, delete).unwrap();
        }
        let error = e
            .commit_workspace(CommitRequest {
                workspace_id: w.workspace_id.clone(),
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: w.base_head,
                fencing_token: w.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::WorkspaceDependencyBusy
        );
        assert_eq!(e.state().branches[&r.main_branch_id].head_commit_id, head);
        assert_eq!(fs::read(staged.source_path).unwrap(), b"must remain staged");
    }
}

#[test]
fn smb_binding_is_explicit_conditional_and_requires_clean_workspaces() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    let root = e.state().branches[&r.main_branch_id].head_commit_id.clone();
    let branch = e
        .create_branch(
            &root,
            &storage_contract::BranchName::parse("new-main").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    assert_eq!(
        e.set_smb_binding(
            &r.repository_id,
            &branch.branch_id,
            true,
            1,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::WorkspaceBusy
    );
    e.abort_workspace(&w.workspace_id, "admin").unwrap();
    e.set_smb_binding(
        &r.repository_id,
        &branch.branch_id,
        true,
        1,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(
        e.state().repositories[&r.repository_id].main_branch_id,
        branch.branch_id
    );
    assert_eq!(
        e.set_smb_binding(
            &r.repository_id,
            &r.main_branch_id,
            true,
            1,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::GenerationMismatch
    );
}

#[test]
fn retained_generations_and_sealed_workspaces_keep_their_resource_and_file_ownership() {
    let mut f = Fixture::new();
    f.config.max_file_bytes = 8;
    f.config.max_workspace_bytes = 10;
    f.config.max_workspaces = 2;
    let mut e = f.initialize();
    let r = repository(&mut e, 1);
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    let item = new_id("entry");
    let input = f.temp.path().join("input");
    fs::write(&input, b"1234").unwrap();
    let first = e
        .stage_upload(&w.workspace_id, &item, "a", &input, 1)
        .unwrap();
    fs::write(&input, b"5678").unwrap();
    e.stage_upload(&w.workspace_id, &item, "a", &input, 2)
        .unwrap();
    assert!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 2)
            .is_ok()
    );
    assert_eq!(
        e.stage_upload(&w.workspace_id, &item, "a", &input, 3)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert_eq!(fs::read(first.source_path).unwrap(), b"1234");
    e.seal_workspace(&w.workspace_id, "admin")
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    let second = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    assert_eq!(
        e.stage_bytes(&second.workspace_id, "a", b"no")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBusy
    );
    assert_eq!(
        e.create_workspace(&r.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let missing = f.temp.path().join("open-must-not-initialize");
    assert!(Engine::open(&missing).is_err());
    assert!(!missing.exists());
}

#[test]
fn merge_link_endpoints_must_belong_to_the_selected_distinct_branches() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    let first = save(&mut e, &repo.main_branch_id, "a", b"first")
        .commit_id
        .unwrap();
    let branch = e
        .create_branch(
            &first,
            &storage_contract::BranchName::parse("other").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let latest = save(&mut e, &repo.main_branch_id, "a", b"second")
        .commit_id
        .unwrap();
    assert_eq!(
        e.add_merge_link(
            &branch.branch_id,
            &latest,
            &repo.main_branch_id,
            &first,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::MergeEndpointMismatch
    );
    assert_eq!(
        e.add_merge_link(
            &branch.branch_id,
            &branch.head_commit_id,
            &repo.main_branch_id,
            &branch.head_commit_id,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::MergeEndpointMismatch
    );
    assert_eq!(
        e.add_merge_link(
            &repo.main_branch_id,
            &latest,
            &repo.main_branch_id,
            &first,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::MergeEndpointMismatch
    );
    e.archive_branch(
        &branch.branch_id,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    e.add_merge_link(
        &branch.branch_id,
        &branch.head_commit_id,
        &repo.main_branch_id,
        &first,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(
        e.state().branches[&repo.main_branch_id].head_commit_id,
        latest
    );
}

#[test]
fn auth_sessions_do_not_stale_plans_and_offline_tombstones_are_proven() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let r = repository(&mut e, 3);
    save(&mut e, &r.main_branch_id, "a", b"old");
    let old = e.list_tree(&r.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut e, &r.main_branch_id, "a", b"new");
    let p = e
        .create_purge_plan(&r.history_tree_id, vec![old.clone()], "admin")
        .unwrap();
    e.persist_auth_state(
        json_codec::json!({"session_hashes":["not-raw-session-token"]}),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    e.commit_plan(
        &p.plan_id,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    let report = offline_check(&f.config.disks[0].root).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(report.tombstoned_versions, 1);
    assert_eq!(report.healthy_replicas, 1);
    let dir = crate::disk::version_dir(&f.config.disks[0], &r.history_tree_id, &old);
    let mut proof: json_codec::Value =
        json_codec::from_slice(&fs::read(dir.join("purged.json")).unwrap()).unwrap();
    proof["content_state_page"][&old] = json_codec::json!("AVAILABLE");
    fs::write(dir.join("purged.json"), json_codec::to_vec(&proof).unwrap()).unwrap();
    assert_eq!(
        offline_check(&f.config.disks[0].root).unwrap().issues.len(),
        1
    );
}

#[test]
fn partial_publication_is_not_visible_with_two_members_and_receipt_tampering_is_rejected() {
    for complete in [false, true] {
        let f = Fixture::new();
        let mut e = f.initialize();
        let r = repository(&mut e, 3);
        let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
        e.stage_bytes(&w.workspace_id, "a", b"protected").unwrap();
        let request = CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: w.base_head.clone(),
            fencing_token: w.fencing_token,
            message: String::new(),
            request_digest: None,
        };
        if !complete {
            e.inject_fault_once("after_publication_1");
        }
        let result = e.commit_workspace(request.clone());
        assert_eq!(result.is_ok(), complete);
        drop(e);
        let missing = &f.config.disks[2].root;
        let hidden = f.temp.path().join("unavailable");
        fs::rename(missing, &hidden).unwrap();
        let e = Engine::open(&f.root).unwrap();
        assert!(e.is_read_only());
        assert_eq!(
            e.operation(request.operation_id).unwrap().commit_state,
            if complete {
                CommitState::Published
            } else {
                CommitState::InDoubt
            }
        );
        assert_eq!(
            e.list_tree(&r.main_branch_id).unwrap().is_empty(),
            !complete
        );
        drop(e);
        fs::rename(&hidden, missing).unwrap();
        let e = Engine::open(&f.root).unwrap();
        let version = e.list_tree(&r.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let dir = crate::disk::version_dir(&f.config.disks[0], &r.history_tree_id, &version);
        let mut proof: json_codec::Value =
            json_codec::from_slice(&fs::read(dir.join("publication-proof.json")).unwrap()).unwrap();
        proof["publication_receipts"][0]["record"]["record_type"] = json_codec::json!("PREPARE");
        fs::write(
            dir.join("publication-proof.json"),
            json_codec::to_vec(&proof).unwrap(),
        )
        .unwrap();
        assert_eq!(
            offline_check(&f.config.disks[0].root).unwrap().issues.len(),
            1
        );
        assert_eq!(
            offline_export(
                &f.config.disks[0].root,
                &version,
                &f.temp.path().join("must-not-export")
            )
            .unwrap_err()
            .code(),
            storage_contract::ErrorCode::PublicationInvalid
        );
    }
}
#[test]
fn production_does_not_silently_accept_directory_simulation() {
    let mut f = Fixture::new();
    f.config.mode = Mode::Production;
    assert_eq!(
        Engine::initialize(&f.root, f.config).err().unwrap().code(),
        storage_contract::ErrorCode::FilesystemRequired
    );
}

#[test]
fn sparse_namespace_branch_and_workspace_have_canonical_bytes() {
    let sparse_branch = json_codec::json!({
        "branch_id":new_id("br"),"history_tree_id":new_id("history"),"name":"namespace",
        "head_commit_id":new_id("commit"),"fork_commit_id":new_id("commit"),
        "branch_kind":"full","generation":"1","state":"active"
    });
    let decoded: Branch = json_codec::from_value(sparse_branch.clone()).unwrap();
    assert!(decoded.pending_file_versions.is_empty());
    assert_eq!(
        canonical_json(&decoded).unwrap(),
        canonical_json(&sparse_branch).unwrap()
    );
    let sparse_workspace = json_codec::json!({
        "workspace_id":new_id("ws"),"purpose":"normal","branch_id":decoded.branch_id,
        "principal":"admin","base_head":decoded.head_commit_id,"fencing_token":"1",
        "files":{},"namespace_operations":[],"created_at":"1","state":"open"
    });
    let decoded: Workspace = json_codec::from_value(sparse_workspace.clone()).unwrap();
    assert!(decoded.selected_file_branches.is_empty());
    assert_eq!(
        canonical_json(&decoded).unwrap(),
        canonical_json(&sparse_workspace).unwrap()
    );
}

#[test]
fn pending_first_write_keeps_selected_ancestry_and_creates_a_real_empty_version() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"old source");
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"newer main");
    let before = engine.state().versions.len();
    let branch = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "old source branch",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    assert_eq!(engine.state().versions.len(), before);
    assert_eq!(
        engine.pending_file_source(&branch, &source.file_id),
        Some(source.version_id.as_str())
    );
    engine
        .select_file_branch(
            &source.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let placeholder = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    assert_eq!(placeholder.version_id, source.version_id);
    assert_eq!(placeholder.size, source.size);
    assert_eq!(bytes(&engine, &placeholder.version_id), b"old source");
    save(&mut engine, &repo.main_branch_id, "file.txt", b"");
    let entry = engine.list_tree(&branch).unwrap().remove(0);
    let version = &engine.state().versions[&entry.version_id];
    assert_eq!(engine.state().versions.len(), before + 1);
    assert_eq!(version.source_version_ids, vec![source.version_id]);
    assert!(version.created_as_full_checkpoint);
    assert_eq!(entry.size, 0);
    assert!(
        engine
            .pending_file_source(&branch, &source.file_id)
            .is_none()
    );
    assert_eq!(bytes(&engine, &entry.version_id), b"");
    drop(engine);
    let engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id,
        entry.version_id
    );
    assert_eq!(bytes(&engine, &entry.version_id), b"");
}

#[test]
fn first_write_identical_to_source_is_not_an_unwritten_or_shared_version() {
    for source_bytes in [b"same bytes".as_slice(), b"".as_slice()] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        save(&mut engine, &repo.main_branch_id, "file.txt", source_bytes);
        let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let branch = engine
            .create_file_branch(
                &source.file_id,
                &source.version_id,
                "identical",
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        let before = engine.state().versions.len();
        save(&mut engine, &branch, "file.txt", source_bytes);
        let entry = engine.list_tree(&branch).unwrap().remove(0);
        assert_ne!(entry.version_id, source.version_id);
        assert_eq!(engine.state().versions.len(), before + 1);
        assert_eq!(bytes(&engine, &entry.version_id), source_bytes);
        assert!(
            engine.state().branches[&branch]
                .pending_file_versions
                .is_empty()
        );
    }
}

#[test]
fn pending_range_write_edits_parent_before_publishing_its_own_complete_version() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"parent body",
    );
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let branch = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "range",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry"),
            "file.txt",
            3,
            b"X",
            Some(4),
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "range".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree(&branch).unwrap().remove(0);
    assert_eq!(bytes(&engine, &entry.version_id), b"parX");
    assert_eq!(
        engine.state().versions[&entry.version_id].source_version_ids,
        vec![source.version_id]
    );
}

#[test]
fn switching_pending_destinations_fences_old_directory_writes_with_same_source() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"shared ancestor",
    );
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let first = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "first",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let second = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "second",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(
            &source.file_id,
            &first,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"belongs to first")
        .unwrap();
    engine
        .select_file_branch(
            &source.file_id,
            &second,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let before = engine.state().versions.len();
    let error = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "stale".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    assert!(staged.source_path.exists());
    assert_eq!(engine.state().versions.len(), before);
    assert_eq!(
        engine.list_tree(&first).unwrap()[0].version_id,
        source.version_id
    );
    assert!(
        engine
            .pending_file_source(&first, &source.file_id)
            .is_some()
    );
    assert_eq!(
        engine.list_tree(&second).unwrap()[0].version_id,
        source.version_id
    );
    assert!(
        engine
            .pending_file_source(&second, &source.file_id)
            .is_some()
    );
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"belongs to second",
    );
    assert_eq!(
        bytes(&engine, &engine.list_tree(&second).unwrap()[0].version_id),
        b"belongs to second"
    );
    assert_eq!(
        engine.list_tree(&first).unwrap()[0].version_id,
        source.version_id
    );
    assert!(
        engine
            .pending_file_source(&first, &source.file_id)
            .is_some()
    );
}

#[test]
fn directory_branch_inherits_frozen_parents_and_overrides_only_written_files_after_reopen() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    for (path, body) in [
        ("folder/a.txt", b"parent A".as_slice()),
        ("folder/b.txt", b"parent B".as_slice()),
        ("folder/c.txt", b"parent C".as_slice()),
    ] {
        save(&mut engine, &repo.main_branch_id, path, body);
    }
    let parents = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| (entry.path().to_owned(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    let before = engine.state().versions.len();
    let created = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(created.result["copied_files"], "0");
    assert_eq!(created.result["placeholder_files"], "0");
    assert_eq!(engine.state().versions.len(), before);
    // The inheritance is a frozen parent snapshot, not a moving parent HEAD.
    save(
        &mut engine,
        &repo.main_branch_id,
        "folder/b.txt",
        b"later parent B",
    );
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    for entry in engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
    {
        assert_eq!(entry.version_id, parents[entry.path()].version_id);
        assert_eq!(
            engine.pending_file_source(&repo.main_branch_id, &entry.file_id),
            None
        );
        assert_eq!(
            bytes(&engine, &entry.version_id),
            format!(
                "parent {}",
                entry.name().chars().next().unwrap().to_ascii_uppercase()
            )
            .as_bytes()
        );
    }
    save(
        &mut engine,
        &repo.main_branch_id,
        "folder/a.txt",
        b"draft A only",
    );
    assert_eq!(
        engine.state().versions.len(),
        before + 2,
        "one later parent edit and one actual draft edit, never branch copies"
    );
    let selected = engine.state().file_selections.clone();
    let inspect = |engine: &Engine| {
        for entry in engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if entry.path == "folder/a.txt" {
                assert_ne!(entry.version_id, parents[entry.path()].version_id);
                assert_eq!(bytes(engine, &entry.version_id), b"draft A only");
                assert!(
                    engine
                        .pending_file_source(&repo.main_branch_id, &entry.file_id)
                        .is_none()
                );
                assert!(
                    engine.state().branches[&selected[&entry.file_id]]
                        .pending_file_versions
                        .is_empty()
                );
            } else {
                assert_eq!(entry.version_id, parents[entry.path()].version_id);
                assert!(
                    engine
                        .pending_file_source(&repo.main_branch_id, &entry.file_id)
                        .is_none()
                );
                assert_eq!(
                    bytes(engine, &entry.version_id),
                    if entry.path == "folder/b.txt" {
                        b"parent B"
                    } else {
                        b"parent C"
                    }
                );
            }
        }
    };
    inspect(&engine);
    drop(engine);
    let engine = Engine::open(&fixture.root).unwrap();
    inspect(&engine);
    assert_eq!(engine.state().file_selections, selected);
}

#[test]
fn workspace_commit_rejects_bytes_changed_after_upload_sealing() {
    use std::os::unix::fs::PermissionsExt;

    for changed in [b"evil".as_slice(), b"changed length".as_slice()] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"good")
            .unwrap();
        fs::set_permissions(&staged.source_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&staged.source_path, changed).unwrap();
        let sequence = engine.state().sequence;
        let operation = storage_contract::OperationId::generate();
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: operation,
            principal: "admin".into(),
            expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        };
        let error = engine.commit_workspace(request.clone()).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::ChecksumMismatch);
        assert_eq!(engine.state().sequence, sequence);
        assert!(!engine.state().operations.contains_key(&operation));
        assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "open"
        );

        // Failure did not consume the operation or publish a different request.
        fs::write(&staged.source_path, b"good").unwrap();
        engine.commit_workspace(request).unwrap();
        let entries = engine.list_tree(&repo.main_branch_id).unwrap();
        assert_eq!(bytes(&engine, &entries[0].version_id), b"good");
        drop(engine);
        let reopened = Engine::open(&fixture.root).unwrap();
        assert_eq!(bytes(&reopened, &entries[0].version_id), b"good");
    }
}

#[test]
fn journal_append_rejects_dangling_symlink_without_creating_its_target() {
    let fixture = Fixture::new();
    let disk = &fixture.config.disks[0];
    let log = crate::journal::log_path(disk);
    crate::disk::private_dir(log.parent().unwrap()).unwrap();
    let outside = fixture.temp.path().join("outside-journal.bin");
    std::os::unix::fs::symlink(&outside, &log).unwrap();
    let error = crate::journal::append(
        disk,
        RecordType::Prepare,
        1,
        &"a".repeat(64),
        &"b".repeat(64),
    )
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::UnsafePath);
    assert!(!outside.exists());
    assert!(fs::symlink_metadata(&log).unwrap().file_type().is_symlink());
}

#[test]
fn sealed_batch_checksum_failure_rolls_back_all_files_and_retries_after_restart() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["first.txt", "second.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, path.as_bytes())
            .unwrap();
    }
    engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    // Corrupt the last item in execution order, after another file has already
    // been composed into the transaction's private candidate state.
    let staged = engine
        .workspace(&workspace.workspace_id)
        .unwrap()
        .files
        .values()
        .last()
        .unwrap()
        .clone();
    fs::set_permissions(&staged.source_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&staged.source_path, b"corrupted").unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "atomic sealed batch".into(),
        request_digest: None,
    };
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        storage_contract::ErrorCode::ChecksumMismatch
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(!engine.is_read_only());
    drop(engine);

    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "sealed"
    );
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        storage_contract::ErrorCode::ChecksumMismatch
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    fs::write(&staged.source_path, staged.path.as_bytes()).unwrap();
    let result = engine.commit_workspace(request.clone()).unwrap();
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap().commit_id,
        result.commit_id
    );
    assert_eq!(engine.state().versions.len(), 2);
    drop(engine);

    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(
        engine.commit_workspace(request).unwrap().commit_id,
        result.commit_id
    );
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(bytes(&engine, &entry.version_id), entry.path.as_bytes());
    }
    assert_eq!(engine.state().versions.len(), 2);
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "committed"
    );
}

#[test]
fn accepted_batch_recovers_without_staged_sources_and_replays_exactly_once() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let first = engine
        .stage_bytes(&workspace.workspace_id, "first.txt", b"first")
        .unwrap();
    let second = engine
        .stage_bytes(&workspace.workspace_id, "second.txt", b"second")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "recover full batch".into(),
        request_digest: None,
    };
    engine.inject_fault_once("after_accept_1");
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        storage_contract::ErrorCode::InDoubt
    );
    assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
    assert!(engine.state().versions.is_empty());
    // Once accepted, recovery must use prepared durable payloads, not require
    // uploads that the client may already have lost.
    fs::remove_file(first.source_path).unwrap();
    fs::remove_file(second.source_path).unwrap();
    drop(engine);

    let mut engine = Engine::open(&fixture.root).unwrap();
    let result = engine.operation(request.operation_id).unwrap();
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "committed"
    );
    let sequence = engine.state().sequence;
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap().commit_id,
        result.commit_id
    );
    assert_eq!(engine.state().sequence, sequence);
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(bytes(&engine, &entries[0].version_id), b"first");
    assert_eq!(bytes(&engine, &entries[1].version_id), b"second");
    assert_eq!(engine.state().versions.len(), 2);
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(
        engine.commit_workspace(request).unwrap().commit_id,
        result.commit_id
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.state().versions.len(), 2);
}

#[test]
fn unchanged_save_recovery_persists_operation_without_creating_content_history() {
    for (fault, accepted) in [("after_prepare_1", false), ("after_accept_1", true)] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let original = save(&mut engine, &repo.main_branch_id, "file.txt", b"unchanged");
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"unchanged")
            .unwrap();
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "same bytes".into(),
            request_digest: None,
        };
        let commits = engine.state().commits.len();
        let versions = engine.state().versions.len();
        let generation = engine.state().branches[&repo.main_branch_id].generation;
        engine.inject_fault_once(fault);
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap_err().code(),
            if accepted {
                storage_contract::ErrorCode::InDoubt
            } else {
                storage_contract::ErrorCode::InjectedFailure
            },
            "{fault}"
        );
        assert_eq!(engine.is_read_only(), accepted);
        drop(engine);

        let mut engine = Engine::open(&fixture.root).unwrap();
        assert_eq!(
            engine
                .state()
                .operations
                .contains_key(&request.operation_id),
            accepted,
            "{fault}"
        );
        let result = engine.commit_workspace(request.clone()).unwrap();
        assert_eq!(result.commit_id, original.commit_id);
        assert_eq!(result.result["no_change"], true);
        let sequence = engine.state().sequence;
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap().commit_id,
            result.commit_id
        );
        assert_eq!(engine.state().sequence, sequence);
        assert_eq!(engine.state().commits.len(), commits);
        assert_eq!(engine.state().versions.len(), versions);
        assert_eq!(
            engine.state().branches[&repo.main_branch_id].generation,
            generation
        );
        drop(engine);

        let mut engine = Engine::open(&fixture.root).unwrap();
        assert_eq!(
            engine.commit_workspace(request).unwrap().commit_id,
            original.commit_id
        );
        assert_eq!(engine.state().sequence, sequence);
        assert_eq!(engine.state().commits.len(), commits);
        assert_eq!(engine.state().versions.len(), versions);
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "committed"
        );
    }
}

#[test]
fn content_read_plan_pins_before_io_and_authenticates_manifest_outside_engine() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 2);
    save(&mut engine, &repo.main_branch_id, "range", &[7; 65536]);
    let version_id = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, "range", b"new version");
    let plan = engine.prepare_content(&version_id).unwrap();
    assert_eq!(
        engine
            .create_purge_plan(&repo.history_tree_id, vec![version_id.clone()], "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    // The plan owns its snapshot, so unrelated metadata work can run before any
    // disk verification; no borrow or Engine mutex is retained.
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let version = engine.state.versions[&version_id].clone();
    let placement = engine.state.placements[&version_id].clone();
    let first = engine.disk(&placement.route_slot_ids[0]).unwrap();
    let dir = disk::version_dir(first, &version.history_tree_id, &version_id);
    fs::write(dir.join("checksums.bin"), b"bad manifest").unwrap();
    let opened = plan.open().unwrap(); // authenticated second replica
    assert_eq!(opened.manifest.logical_size, 65536);
    assert_eq!(opened.file.metadata().unwrap().len(), 65536);
    drop(opened);
    assert!(
        engine
            .create_purge_plan(&repo.history_tree_id, vec![version_id.clone()], "admin")
            .is_ok()
    );
    // All disks gone: plan construction still succeeds, proving it performs no
    // payload, manifest, publication or disk identity reads under the lock.
    for slot in &placement.route_slot_ids {
        let disk = engine.disk(slot).unwrap();
        fs::remove_dir_all(disk::version_dir(
            disk,
            &version.history_tree_id,
            &version_id,
        ))
        .unwrap();
    }
    let plan = engine.prepare_content(&version_id).unwrap();
    assert!(plan.open().is_err());
    assert!(!engine.reads.lock().unwrap().contains_key(&version_id));
}

#[test]
fn shared_client_prepares_real_seal_without_losing_frozen_head() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    for expected_override in [None, Some("")] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "prepared.txt",
                b"prepared from a real seal",
            )
            .unwrap();
        let seal = engine
            .seal_workspace(&workspace.workspace_id, "admin")
            .map(|seal| json_codec::to_value(seal).unwrap())
            .unwrap();
        let operation_id = storage_contract::OperationId::generate();
        let body = storage_contract::client_protocol::prepare_commit(
            seal,
            &workspace.workspace_id,
            &operation_id.to_string(),
            "shared client",
            expected_override,
        )
        .unwrap();
        assert_eq!(body["expected_head"], workspace.base_head);
        assert_eq!(
            body["request_digest"],
            commit_request_digest(&workspace.workspace_id, &body).unwrap()
        );
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id,
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "shared client".into(),
            request_digest: Some(body["request_digest"].as_str().unwrap().into()),
        };
        let committed = engine
            .commit_workspace_wire(request.clone(), public_envelope(body.clone()))
            .unwrap();
        assert_eq!(
            engine
                .commit_workspace_wire(request, public_envelope(body))
                .unwrap()
                .commit_id,
            committed.commit_id
        );
    }
    drop(engine);
    let engine = Engine::open(&fixture.root).unwrap();
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(
        bytes(&engine, &entries[0].version_id),
        b"prepared from a real seal"
    );
}

#[test]
fn workspace_read_plan_pins_exact_inode_before_unlocked_verification() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "staged", b"original")
        .unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, &staged.item_id, "admin")
        .unwrap();
    let replacement = staged.source_path.with_extension("replacement");
    fs::write(&replacement, b"modified").unwrap();
    fs::rename(replacement, &staged.source_path).unwrap();
    let mut opened = plan.open().unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    drop(opened);
    assert_eq!(
        engine
            .prepare_workspace_content(&workspace.workspace_id, &staged.item_id, "admin")
            .unwrap()
            .open()
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ChecksumMismatch
    );
}

#[test]
fn upload_preparation_releases_engine_and_revalidates_workspace_without_lost_updates() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let input = fixture.temp.path().join("upload-input");
    fs::write(&input, b"prepared upload bytes").unwrap();
    let item = new_id("entry");
    let plan = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap();
    assert_eq!(
        engine
            .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::FileWriteBusy
    );
    // The copy plan is detached from Engine; unrelated writer work is allowed.
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("parallel").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let prepared = plan.execute().unwrap();
    let directory = engine.workspace_data_directory(&workspace).unwrap();
    assert!(fs::read_dir(&directory).unwrap().any(|e| {
        e.unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "prepared")
    }));
    // Fence changes while copying cannot be overwritten by a stale manifest.
    engine
        .workspaces
        .get_mut(&workspace.workspace_id)
        .unwrap()
        .fencing_token += 1;
    assert_eq!(
        engine.finish_upload(prepared).unwrap_err().code(),
        storage_contract::ErrorCode::WorkspaceChanged
    );
    assert!(input.exists());
    assert!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .files
            .is_empty()
    );
    assert!(!fs::read_dir(&directory).unwrap().any(|e| {
        e.unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "prepared")
    }));
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap()
        .execute()
        .unwrap();
    let staged = engine.finish_upload(prepared).unwrap();
    assert_eq!(
        fs::read(staged.source_path).unwrap(),
        b"prepared upload bytes"
    );
}

#[test]
fn dropped_upload_preparation_cleans_only_owned_scratch_and_releases_claim() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let input = fixture.temp.path().join("upload-input");
    fs::write(&input, b"retained HTTP spool").unwrap();
    let item = new_id("entry");
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap()
        .execute()
        .unwrap();
    drop(prepared);
    assert_eq!(fs::read(&input).unwrap(), b"retained HTTP spool");
    let plan = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap();
    drop(plan);
    assert!(
        engine
            .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
            .is_ok()
    );
}

#[test]
fn upload_preparation_counts_crash_scratch_and_rechecks_final_quota() {
    let mut fixture = Fixture::new();
    fixture.config.max_file_bytes = 8;
    fixture.config.max_workspace_bytes = 10;
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let directory = engine.workspace_data_directory(&workspace).unwrap();
    let input = fixture.temp.path().join("upload-input");
    fs::write(&input, b"12345678").unwrap();
    let crash_scratch = directory.join(".zxs-upload-crashed.prepared");
    fs::write(&crash_scratch, b"old").unwrap();
    let item = new_id("entry");
    assert_eq!(
        engine
            .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    // A failed admission must release its claim, and new scratch counts once.
    fs::remove_file(&crash_scratch).unwrap();
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap()
        .execute()
        .unwrap();
    fs::write(&crash_scratch, b"old").unwrap();
    assert_eq!(
        engine.finish_upload(prepared).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert_eq!(fs::read(&crash_scratch).unwrap(), b"old");
    assert!(input.exists());
    fs::remove_file(crash_scratch).unwrap();
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(engine.finish_upload(prepared).unwrap().size, 8);
}

#[test]
fn upload_preparation_rejects_parent_swap_and_cleans_original_directory_only() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let directory = engine.workspace_data_directory(&workspace).unwrap();
    let input = fixture.temp.path().join("upload-input");
    fs::write(&input, b"safe payload").unwrap();
    let item = new_id("entry");
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "prepared", &input, 1)
        .unwrap()
        .execute()
        .unwrap();
    let scratch_name = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .find(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "prepared")
        })
        .unwrap();
    let moved = directory.with_extension("moved");
    fs::rename(&directory, &moved).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join(&scratch_name),
        b"do not delete replacement directory file",
    )
    .unwrap();
    assert_eq!(
        engine.finish_upload(prepared).unwrap_err().code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert_eq!(
        fs::read(directory.join(&scratch_name)).unwrap(),
        b"do not delete replacement directory file"
    );
    assert!(!moved.join(&scratch_name).exists());
    assert!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .files
            .is_empty()
    );
}

fn detached_commit_fixture(engine: &mut Engine, repo: &Repository) -> (Workspace, CommitRequest) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "detached-a",
            b"first detached file",
        )
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "detached-b",
            b"second detached file",
        )
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "detached preparation".into(),
        request_digest: None,
    };
    (workspace, request)
}

#[test]
fn detached_commit_retry_preserves_actor_binding_after_restart() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"saved")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "single-account save".into(),
        request_digest: None,
    };
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    let original = engine.finish_workspace_commit(prepared).unwrap();
    let sequence = engine.state.sequence;
    // The synchronous retry already accepts the same actor-bound request.
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), original);
    for reopen in [false, true] {
        if reopen {
            drop(engine);
            engine = Engine::open(&fixture.root).unwrap();
        }
        let prepared = engine
            .prepare_workspace_commit(request.clone(), None)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(engine.finish_workspace_commit(prepared).unwrap(), original);
        assert_eq!(engine.state.sequence, sequence);
    }
    let mut changed_actor = request;
    changed_actor.principal = "different-actor".into();
    assert_eq!(
        engine
            .prepare_workspace_commit(changed_actor, None)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::PermissionDenied
    );
}

#[test]
fn detached_workspace_preparation_writes_no_decision_and_replays_original_operation() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let (workspace, request) = detached_commit_fixture(&mut engine, &repo);
    let sequence = engine.state.sequence;
    let before: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| journal::scan(disk).unwrap().records.len())
        .collect();
    let plan = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap();
    assert_eq!(
        engine
            .prepare_workspace_commit(request.clone(), None)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    let prepared = plan.execute().unwrap();
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(
        engine
            .members()
            .iter()
            .map(|disk| journal::scan(disk).unwrap().records.len())
            .collect::<Vec<_>>(),
        before
    );
    assert!(!engine.state.operations.contains_key(&request.operation_id));
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "open"
    );
    assert!(
        !engine
            .root
            .join("workspaces")
            .join(&workspace.workspace_id)
            .join("commit-request.json")
            .exists()
    );
    let result = engine.finish_workspace_commit(prepared).unwrap();
    assert_eq!(result.operation_id, request.operation_id);
    assert_eq!(engine.state.sequence, sequence + 1);
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap().len(), 2);
    let retry = engine
        .prepare_workspace_commit(request, None)
        .unwrap()
        .execute()
        .unwrap();
    let replay = engine.finish_workspace_commit(retry).unwrap();
    assert_eq!(replay.request_digest, result.request_digest);
    assert_eq!(replay.sequence, result.sequence);
}

#[test]
fn detached_workspace_commit_stale_base_is_definite_retryable_conflict() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let (workspace, request) = detached_commit_fixture(&mut engine, &repo);
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    let busy = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("unrelated-writer").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(
        busy.code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert!(busy.is_retryable());
    // Simulate an externally invalidated base to retain the final fence test.
    std::sync::Arc::make_mut(&mut engine.state).business_generation += 1;
    let error = engine.finish_workspace_commit(prepared).unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::CommitPreparationStale
    );
    assert!(!engine.state.operations.contains_key(&request.operation_id));
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "open"
    );
    for staged in engine
        .workspace(&workspace.workspace_id)
        .unwrap()
        .files
        .values()
    {
        assert!(staged.source_path.exists());
    }
    let retry = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        engine.finish_workspace_commit(retry).unwrap().operation_id,
        request.operation_id
    );
}

#[test]
fn detached_workspace_commit_rechecks_fence_and_releases_cancelled_admission() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let (workspace, request) = detached_commit_fixture(&mut engine, &repo);
    let plan = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap();
    drop(plan);
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    drop(prepared);
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(
        engine.finish_workspace_commit(prepared).unwrap_err().code(),
        storage_contract::ErrorCode::CommitPreparationStale
    );
    assert!(!engine.state.operations.contains_key(&request.operation_id));
}

#[test]
fn detached_workspace_commit_consumes_real_fault_only_at_decision_boundary() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let (_, request) = detached_commit_fixture(&mut engine, &repo);
    engine.inject_fault_once("after_payloads");
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        engine.finish_workspace_commit(prepared).unwrap_err().code(),
        storage_contract::ErrorCode::InjectedFailure
    );
    assert!(!engine.blocked);
    assert!(!engine.state.operations.contains_key(&request.operation_id));
    let retry = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        engine.finish_workspace_commit(retry).unwrap().operation_id,
        request.operation_id
    );
}

#[test]
fn detached_workspace_commit_checks_actual_source_owner_and_topology() {
    use std::os::unix::fs::PermissionsExt;
    for changed in ["source", "owner", "topology"] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 1);
        let (workspace, request) = detached_commit_fixture(&mut engine, &repo);
        let plan = engine
            .prepare_workspace_commit(request.clone(), None)
            .unwrap();
        if changed == "source" {
            let staged = engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .files
                .into_values()
                .next()
                .unwrap();
            fs::set_permissions(&staged.source_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&staged.source_path, b"altered after snapshot").unwrap();
            assert_eq!(
                plan.execute().err().unwrap().code(),
                storage_contract::ErrorCode::ChecksumMismatch
            );
        } else {
            let prepared = plan.execute().unwrap();
            if changed == "owner" {
                let live = engine.workspaces.get_mut(&workspace.workspace_id).unwrap();
                live.principal = "different-actor".into();
            } else {
                std::sync::Arc::make_mut(&mut engine.state).topology_generation += 1;
            }
            let error = engine.finish_workspace_commit(prepared).unwrap_err();
            assert_eq!(
                error.code(),
                if changed == "owner" {
                    storage_contract::ErrorCode::PermissionDenied
                } else {
                    storage_contract::ErrorCode::CommitPreparationStale
                }
            );
        }
        assert!(!engine.state.operations.contains_key(&request.operation_id));
        assert!(!engine.blocked);
    }
}

#[test]
fn detached_public_wire_commit_keeps_exact_digest_and_retry_binding() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "wire-file", b"wire save")
        .unwrap();
    let mut request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: String::new(),
        request_digest: None,
    };
    let mut body = engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .map(|seal| json_codec::to_value(seal).unwrap())
        .unwrap();
    request.operation_id = body["operation_id"].as_str().unwrap().parse().unwrap();
    request.message.clear();
    body["request_digest"] =
        json_codec::json!(commit_request_digest(&workspace.workspace_id, &body).unwrap());
    let prepared = engine
        .prepare_workspace_commit(request.clone(), Some(public_envelope(body.clone())))
        .unwrap()
        .execute()
        .unwrap();
    let result = engine.finish_workspace_commit(prepared).unwrap();
    assert_eq!(
        result.request_digest,
        body["request_digest"].as_str().unwrap()
    );
    let retry = engine
        .prepare_workspace_commit(request.clone(), Some(public_envelope(body.clone())))
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        engine.finish_workspace_commit(retry).unwrap().commit_id,
        result.commit_id
    );
    body["message"] = json_codec::json!("changed retry");
    let plan = engine
        .prepare_workspace_commit(request, Some(public_envelope(body)))
        .unwrap();
    assert_eq!(
        plan.execute().err().unwrap().code(),
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn detached_preparation_fences_physical_namespace_writes_but_not_reads() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    save(
        &mut engine,
        &repo.main_branch_id,
        "original.txt",
        b"published",
    );
    let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "original.txt", b"replacement")
        .unwrap();
    let workspace_id = workspace.workspace_id.clone();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "fenced preparation".into(),
        request_digest: None,
    };
    let plan = engine.prepare_workspace_commit(request, None).unwrap();
    let rename = NamespaceOperation::parse(
        storage_contract::NamespaceOperationKind::Rename,
        "original.txt",
        Some("renamed.txt"),
    )
    .unwrap();
    let error = engine
        .file_namespace(
            &repo.repository_id,
            rename.clone(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert!(error.is_retryable());
    assert_eq!(bytes(&engine, &version), b"published");
    let prepared = plan.execute().unwrap();
    assert_eq!(
        engine.cleanup_purged().unwrap_err().code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert_eq!(
        engine
            .stage_bytes(&workspace_id, "other.txt", b"x")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    drop(prepared);
    engine
        .file_namespace(
            &repo.repository_id,
            rename,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(bytes(&engine, &version), b"published");
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].path,
        "renamed.txt"
    );
}

#[test]
fn detached_commit_rechecks_lease_expiry_at_finish() {
    let mut fixture = Fixture::new();
    fixture.config.workspace_ttl_seconds = 1;
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let (_, request) = detached_commit_fixture(&mut engine, &repo);
    let prepared = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap()
        .execute()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let before = engine.state.sequence;
    let error = engine.finish_workspace_commit(prepared).unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::FencingTokenExpired
    );
    assert_eq!(engine.state.sequence, before);
    assert!(!engine.state.operations.contains_key(&request.operation_id));
    assert!(engine.require_write().is_ok());
}

#[test]
fn pending_operation_reads_exact_snapshot_replica_without_repair() {
    for kind in ["snapshot", "candidate", "state-page"] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let operation = storage_contract::OperationId::generate();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("pending").unwrap(),
                    "P01",
                    3,
                    "admin",
                    operation
                )
                .is_err()
        );
        let members = engine.members();
        let (source, record) = members
            .iter()
            .find_map(|disk| {
                crate::journal::scan(disk)
                    .unwrap()
                    .records
                    .into_iter()
                    .find(|(record, _)| record.record_type == RecordType::Accept)
                    .map(|(record, _)| (disk, record))
            })
            .unwrap();
        let candidate: crate::journal::Candidate =
            crate::journal::load_object(source, "candidate", &record.candidate_hash).unwrap();
        let hash = match kind {
            "candidate" => record.candidate_hash.clone(),
            "snapshot" => candidate.snapshot_hash.clone(),
            _ => {
                let snapshot: crate::journal::Snapshot =
                    crate::journal::load_object(source, "snapshot", &candidate.snapshot_hash)
                        .unwrap();
                snapshot.pages["operation_principals"][0].clone()
            }
        };
        let path = crate::journal::object_path(source, kind, &hash);
        fs::write(&path, b"broken").unwrap();
        let result = engine.operation(operation).unwrap();
        assert_eq!(result.commit_state, storage_contract::CommitState::InDoubt);
        assert_eq!(
            engine.operation_owner(operation).unwrap().as_deref(),
            Some("admin")
        );
        assert_eq!(fs::read(&path).unwrap(), b"broken");
        for disk in &members {
            fs::write(crate::journal::object_path(disk, kind, &hash), b"broken").unwrap();
        }
        assert!(engine.operation(operation).is_err());
        assert!(engine.operation_owner(operation).is_err());
    }
}
#[test]
fn history_export_reads_exact_evidence_replicas_without_repair() {
    for kind in ["candidate", "publication", "seal"] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let members = engine.members();
        let record = crate::journal::scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap()
            .0;
        let path_for = |disk: &DiskConfig| match kind {
            "candidate" => crate::journal::object_path(disk, kind, &record.candidate_hash),
            "publication" => crate::journal::object_path(disk, kind, &record.object_hash),
            _ => crate::journal::publication_seal_path(disk, &record.candidate_hash),
        };
        let path = path_for(&members[0]);
        let mut damaged: json_codec::Value =
            json_codec::from_slice(&fs::read(&path).unwrap()).unwrap();
        if kind == "seal" {
            damaged["receipts"] = json_codec::json!([]);
        } else if kind == "candidate" {
            damaged["request_digest"] = json_codec::json!("wrong digest");
        } else {
            damaged["candidate_hash"] = json_codec::json!("0".repeat(64));
        }
        let damaged = json_codec::to_vec(&damaged).unwrap();
        // A wrong but parseable majority must never defeat the one exact copy.
        for disk in &members[..2] {
            fs::write(path_for(disk), &damaged).unwrap();
        }
        let destination = fixture.root.join("export");
        engine
            .export_history_package(&repo.history_tree_id, &destination)
            .unwrap();
        offline_check_history_package(&destination).unwrap();
        assert_eq!(fs::read(&path).unwrap(), damaged);
        for disk in &members {
            fs::write(path_for(disk), &damaged).unwrap();
        }
        assert!(
            engine
                .export_history_package(&repo.history_tree_id, &fixture.root.join("rejected"))
                .is_err()
        );
    }
}

#[test]
fn pending_operation_requires_a_trusted_decision_not_prepared_objects() {
    for fault in ["after_prepare_3", "after_accept_1"] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let operation = storage_contract::OperationId::generate();
        engine.inject_fault_once(fault);
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("pending").unwrap(),
                    "P01",
                    3,
                    "admin",
                    operation
                )
                .is_err()
        );
        if fault == "after_accept_1" {
            let disk = engine
                .members()
                .into_iter()
                .find(|disk| {
                    crate::journal::scan(disk)
                        .unwrap()
                        .records
                        .iter()
                        .any(|(record, _)| record.record_type == RecordType::Accept)
                })
                .unwrap();
            fs::write(disk.root.join("disk.json"), b"untrusted").unwrap();
        }
        assert_eq!(
            engine.operation(operation).unwrap_err().code(),
            storage_contract::ErrorCode::OperationNotFound
        );
        assert_eq!(engine.operation_owner(operation).unwrap(), None);
    }
}

#[test]
fn history_export_rejects_conflicting_flat_shard_duplicates_without_repair() {
    for kind in ["candidate", "publication"] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let members = engine.members();
        let record = crate::journal::scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap()
            .0;
        let hash = if kind == "candidate" {
            &record.candidate_hash
        } else {
            &record.object_hash
        };
        let flat = |disk: &DiskConfig| {
            disk.root
                .join("metadata/objects")
                .join(kind)
                .join(format!("{hash}.json"))
        };
        let expected = fs::read(crate::journal::object_path(&members[0], kind, hash)).unwrap();
        for disk in &members[..2] {
            fs::write(flat(disk), b"{}").unwrap();
        }
        let destination = fixture.root.join("mixed-export");
        engine
            .export_history_package(&repo.history_tree_id, &destination)
            .unwrap();
        offline_check_history_package(&destination).unwrap();
        for disk in &members[..2] {
            assert_eq!(fs::read(flat(disk)).unwrap(), b"{}");
            assert_eq!(
                fs::read(crate::journal::object_path(disk, kind, hash)).unwrap(),
                expected
            );
        }
        fs::write(flat(&members[2]), b"{}").unwrap();
        assert!(
            engine
                .export_history_package(
                    &repo.history_tree_id,
                    &fixture.root.join("rejected-mixed-export")
                )
                .is_err()
        );
    }
}

#[test]
fn upload_preparation_reads_the_opened_inode_after_source_path_swap() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let input = fixture.temp.path().join("upload-input");
    fs::write(&input, b"verified stream").unwrap();
    let item = new_id("entry");
    let hash = ContentHash::from_digest(blake3::hash(b"verified stream"));
    let mut source = fs::File::open(&input).unwrap();
    std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(4)).unwrap();
    let plan = engine
        .prepare_upload_file(
            &workspace.workspace_id,
            &item,
            &name_policy::PortablePath::parse("prepared").unwrap(),
            source.try_clone().unwrap(),
            1,
            Some((15, hash)),
        )
        .unwrap();
    fs::rename(&input, input.with_extension("held")).unwrap();
    fs::write(&input, b"replacement bytes").unwrap();
    let staged = engine.finish_upload(plan.execute().unwrap()).unwrap();
    assert_eq!(std::io::Seek::stream_position(&mut source).unwrap(), 4);
    assert_eq!(staged.size, 15);
    assert_eq!(staged.content_hash, hash);
    assert_eq!(fs::read(&staged.source_path).unwrap(), b"verified stream");
}

#[test]
fn upload_preparation_rejects_copied_bytes_not_matching_verified_stream() {
    for wrong_size in [false, true] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let input = fixture.temp.path().join("upload-input");
        fs::write(&input, b"verified stream").unwrap();
        let item = new_id("entry");
        let plan = engine
            .prepare_upload_file(
                &workspace.workspace_id,
                &item,
                &name_policy::PortablePath::parse("prepared").unwrap(),
                fs::File::open(&input).unwrap(),
                1,
                Some((
                    if wrong_size { 14 } else { 15 },
                    ContentHash::from_digest(blake3::hash(b"verified stream")),
                )),
            )
            .unwrap();
        if !wrong_size {
            fs::write(&input, b"modified stream").unwrap();
        }
        assert_eq!(
            plan.execute().err().unwrap().code(),
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .files
                .is_empty()
        );
        let directory = engine.workspace_data_directory(&workspace).unwrap();
        assert!(!directory.join(format!("{item}-1.full")).exists());
        assert!(!fs::read_dir(directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "prepared")
        }));
    }
}
#[path = "retained_workspace_budget.rs"]
mod retained_workspace_budget;
#[path = "runtime_workspace_limits.rs"]
mod runtime_workspace_limits;

#[test]
fn borrowed_tree_shares_the_snapshot_and_preserves_owned_read_checks() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"content");
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let borrowed = engine.list_tree_ref(&repo.main_branch_id).unwrap();
    assert!(!borrowed.is_empty());
    assert_eq!(borrowed, engine.list_tree(&repo.main_branch_id).unwrap());
    assert_eq!(
        borrowed,
        engine.list_tree_at(&repo.main_branch_id, &head).unwrap()
    );
    assert_eq!(
        borrowed.as_ptr(),
        engine.state.commits[&head].entries.as_ptr()
    );
    save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"updated content",
    );
    let current_head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    assert_ne!(head, current_head);
    for selected in [&head, &current_head] {
        let borrowed = engine
            .list_tree_at_ref(&repo.main_branch_id, selected)
            .unwrap();
        assert_eq!(
            borrowed.as_ptr(),
            engine.state.commits[selected].entries.as_ptr()
        );
        assert_eq!(
            borrowed,
            engine.list_tree_at(&repo.main_branch_id, selected).unwrap()
        );
    }
    let historical = engine
        .list_tree_at_ref(&repo.main_branch_id, &head)
        .unwrap();
    let current = engine
        .list_tree_at_ref(&repo.main_branch_id, &current_head)
        .unwrap();
    assert_ne!(historical[0].version_id, current[0].version_id);
    assert_eq!(historical[0].size, 7);
    assert_eq!(current[0].size, 15);
    assert_eq!(
        engine.list_tree_ref(&repo.main_branch_id).unwrap().as_ptr(),
        current.as_ptr()
    );
    assert_eq!(
        engine.list_tree_ref("missing").unwrap_err().code(),
        storage_contract::ErrorCode::BranchNotFound
    );
    assert_eq!(
        engine.list_tree("missing").unwrap_err().code(),
        storage_contract::ErrorCode::BranchNotFound
    );

    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let foreign_head = engine.state.branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    for (branch, selected, code) in [
        (
            "missing",
            head.as_str(),
            storage_contract::ErrorCode::BranchNotFound,
        ),
        (
            "missing",
            "missing",
            storage_contract::ErrorCode::BranchNotFound,
        ),
        (
            repo.main_branch_id.as_str(),
            "missing",
            storage_contract::ErrorCode::CommitNotFound,
        ),
        (
            repo.main_branch_id.as_str(),
            foreign_head.as_str(),
            storage_contract::ErrorCode::CommitNotFound,
        ),
    ] {
        let borrowed = engine.list_tree_at_ref(branch, selected).unwrap_err();
        let owned = engine.list_tree_at(branch, selected).unwrap_err();
        assert_eq!(borrowed.code(), code);
        assert_eq!(borrowed.code(), owned.code());
        assert_eq!(borrowed.message(), owned.message());
    }
    // Exercise identical validation when the current head itself is invalid.
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .head_commit_id = foreign_head.clone();
    assert_eq!(
        engine
            .list_tree_ref(&repo.main_branch_id)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::CommitNotFound
    );
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap_err().code(),
        storage_contract::ErrorCode::CommitNotFound
    );
    // An explicit valid snapshot is independent of the current head.
    assert_eq!(
        engine
            .list_tree_at_ref(&repo.main_branch_id, &head)
            .unwrap()
            .as_ptr(),
        engine.state.commits[&head].entries.as_ptr()
    );
    let branch = std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap();
    branch.head_commit_id = head.clone();
    branch.state = storage_contract::BranchState::Migrated;
    assert_eq!(
        engine
            .list_tree_ref(&repo.main_branch_id)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap_err().code(),
        storage_contract::ErrorCode::TreeMoved
    );
    // Migration wins over commit lookup, including missing and foreign IDs.
    for selected in [head.as_str(), foreign_head.as_str(), "missing"] {
        let borrowed = engine
            .list_tree_at_ref(&repo.main_branch_id, selected)
            .unwrap_err();
        let owned = engine
            .list_tree_at(&repo.main_branch_id, selected)
            .unwrap_err();
        assert_eq!(borrowed.code(), storage_contract::ErrorCode::TreeMoved);
        assert_eq!(borrowed.code(), owned.code());
        assert_eq!(borrowed.message(), owned.message());
    }
}

mod state_read_tests;

fn public_envelope(body: json_codec::Value) -> storage_contract::PublicCommitEnvelope {
    let workspace = body["workspace_id"].as_str().unwrap().to_owned();
    storage_contract::PublicCommitEnvelope::parse(&workspace, body).unwrap()
}
