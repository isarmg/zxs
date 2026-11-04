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
