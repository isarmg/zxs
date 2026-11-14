use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Workspace, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("uploads").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    let source = temp.path().join("external-source");
    fs::write(&source, b"verified upload bytes").unwrap();
    (temp, engine, workspace, source)
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect test descriptor: {error}"),
            },
        )
        .count()
}

#[test]
fn idle_preparation_revokes_real_target_directory_and_cannot_start_copy() {
    let (_temp, engine, workspace, source) = fixture();
    let directory = engine.workspace_data_directory(&workspace).unwrap();
    let before = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    let item = new_id("entry").unwrap();
    let plan = engine
        .prepare_upload(&workspace.workspace_id, &item, "body", &source, 1)
        .unwrap();
    assert_eq!(descriptors(&directory), 1);
    let binding = engine
        .disk_resources
        .get(&engine.config.disks[0])
        .unwrap()
        .root
        .io_binding()
        .unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&directory), 0);
    assert_eq!(descriptors(&source), 0);
    assert_eq!(
        plan.execute().err().unwrap().code(),
        ErrorCode::DiskUnavailable
    );
    assert!(engine.upload_preparations.lock().unwrap().is_empty());
    assert_eq!(
        fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        before
    );
    assert_eq!(fs::read(&source).unwrap(), b"verified upload bytes");
}

#[test]
fn prepared_upload_revocation_cleans_scratch_without_waiting_for_caller_drop() {
    let (_temp, mut engine, workspace, source) = fixture();
    let directory = engine.workspace_data_directory(&workspace).unwrap();
    let item = new_id("entry").unwrap();
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "body", &source, 1)
        .unwrap()
        .execute()
        .unwrap();
    let scratch = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "prepared")
        })
        .unwrap();
    assert_eq!(descriptors(&scratch), 1);
    let binding = engine
        .disk_resources
        .get(&engine.config.disks[0])
        .unwrap()
        .root
        .io_binding()
        .unwrap()
        .clone();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&scratch), 0);
    assert_eq!(descriptors(&directory), 0);
    assert!(!scratch.exists());
    fs::write(&scratch, b"unrelated after drain").unwrap();
    assert_eq!(
        engine.finish_upload(prepared).unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(fs::read(&scratch).unwrap(), b"unrelated after drain");
    assert!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .files
            .is_empty()
    );
    assert!(engine.upload_preparations.lock().unwrap().is_empty());
}

#[test]
fn retry_verified_file_is_revoked_without_deleting_the_staged_generation() {
    let (_temp, mut engine, workspace, source) = fixture();
    let item = new_id("entry").unwrap();
    let staged = engine
        .stage_upload(&workspace.workspace_id, &item, "body", &source, 1)
        .unwrap();
    let prepared = engine
        .prepare_upload(&workspace.workspace_id, &item, "body", &source, 1)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(descriptors(&staged.source_path), 1);
    let binding = engine
        .disk_resources
        .get(&engine.config.disks[0])
        .unwrap()
        .root
        .io_binding()
        .unwrap()
        .clone();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&staged.source_path), 0);
    drop(prepared);
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"verified upload bytes"
    );
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().files[&item].source_path,
        staged.source_path
    );
}

fn existing_file_fixture() -> (tempfile::TempDir, Engine, Workspace, std::path::PathBuf, Entry) {
    let (temporary, mut engine, initial, source) = fixture();
    engine
        .stage_bytes(&initial.workspace_id, "report.txt", b"original body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: initial.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: initial.base_head,
            fencing_token: initial.fencing_token,
            message: "source occurrence fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree(&initial.branch_id).unwrap().remove(0);
    let workspace = engine.create_workspace(&initial.branch_id, "admin").unwrap();
    (temporary, engine, workspace, source, entry)
}

#[test]
fn prepared_upload_keeps_source_identity_through_case_rename_and_cold_retry() {
    let (temporary, mut engine, workspace, source, original) = existing_file_fixture();
    let prepared = engine
        .prepare_upload(
            &workspace.workspace_id,
            &original.entry_id,
            "report.txt",
            &source,
            1,
        )
        .unwrap()
        .execute()
        .unwrap();
    let repository = engine.repository_for_branch(&workspace.branch_id).unwrap().clone();
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "report.txt",
                Some("REPORT.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let current = engine.list_tree(&workspace.branch_id).unwrap().remove(0);
    assert_eq!(current.entry_id, original.entry_id);
    assert_eq!(current.file_id, original.file_id);
    assert_eq!(current.version_id, original.version_id);
    assert_eq!(current.path(), "REPORT.txt");
    let published = canonical_json(engine.state()).unwrap();
    let staged = engine.finish_upload(prepared).unwrap();
    assert_eq!(staged.base_version_id.as_ref(), Some(&original.version_id));
    assert_eq!(canonical_json(engine.state()).unwrap(), published);
    assert_eq!(fs::read(&staged.source_path).unwrap(), b"verified upload bytes");
    let manifest = canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap();
    drop(engine);
    let mut engine = Engine::open(&temporary.path().join("state")).unwrap();
    let replay = engine
        .stage_upload(
            &workspace.workspace_id,
            &original.entry_id,
            "report.txt",
            &source,
            1,
        )
        .unwrap();
    assert_eq!(canonical_json(&replay).unwrap(), canonical_json(&staged).unwrap());
    assert_eq!(canonical_json(engine.state()).unwrap(), published);
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        manifest,
    );
    assert_eq!(engine.list_tree(&workspace.branch_id).unwrap(), vec![current]);
    let mut historical = engine.open_content(&original.version_id).unwrap();
    let mut bytes = Vec::new();
    historical.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original body");
}

#[test]
fn prepared_upload_same_body_selection_change_preserves_staged_generation_after_reopen() {
    let (temporary, mut engine, workspace, source, original) = existing_file_fixture();
    let retained = engine
        .stage_upload(
            &workspace.workspace_id,
            &original.entry_id,
            "report.txt",
            &source,
            1,
        )
        .unwrap();
    fs::write(&source, b"later pending bytes").unwrap();
    let prepared = engine
        .prepare_upload(
            &workspace.workspace_id,
            &original.entry_id,
            "report.txt",
            &source,
            2,
        )
        .unwrap()
        .execute()
        .unwrap();
    let selected = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("same body selection").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(
            &original.file_id,
            &selected,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let current = engine.list_tree(&workspace.branch_id).unwrap().remove(0);
    assert_eq!(current.version_id, original.version_id);
    let published = canonical_json(engine.state()).unwrap();
    let manifest = canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap();
    assert_eq!(
        engine.finish_upload(prepared).unwrap_err().code(),
        ErrorCode::FileBranchConflict,
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), published);
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        manifest,
    );
    assert_eq!(fs::read(&retained.source_path).unwrap(), b"verified upload bytes");
    assert_eq!(fs::read(&source).unwrap(), b"later pending bytes");
    drop(engine);
    let engine = Engine::open(&temporary.path().join("state")).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), published);
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        manifest,
    );
    assert_eq!(
        engine
            .prepare_upload(
                &workspace.workspace_id,
                &original.entry_id,
                "report.txt",
                &source,
                2,
            )
            .err()
            .unwrap()
            .code(),
        ErrorCode::FileBranchConflict,
    );
    assert_eq!(fs::read(&retained.source_path).unwrap(), b"verified upload bytes");
}
