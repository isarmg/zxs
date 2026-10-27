use super::*;
use std::{
    fs,
    io::{Read, Seek},
    path::PathBuf,
};
fn pending() -> (tempfile::TempDir, PathBuf, Config, String, OperationId) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("repair").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "a", b"verified original bytes")
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_2");
    assert_eq!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InDoubt
    );
    drop(engine);
    (temp, root, config, repo.main_branch_id, operation)
}
#[test]
fn recovery_repairs_missing_replicas_without_replacing_the_candidate_or_route() {
    let (_temp, root, config, branch, operation) = pending();
    let histories = config.disks[0].root.join("histories");
    let history = fs::read_dir(&histories)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let version = fs::read_dir(history.join("versions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let metadata: FileVersion = read_json(&version.join("version.json")).unwrap();
    fs::remove_dir_all(&version).unwrap();
    let backup = version_dir(
        &config.disks[1],
        &metadata.history_tree_id,
        &metadata.version_id,
    );
    fs::remove_file(backup.join("checksums.bin")).unwrap();
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    let entries = engine.list_tree(&branch).unwrap();
    assert_eq!(entries[0].version_id, metadata.version_id);
    let mut opened = engine.open_content(&metadata.version_id).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"verified original bytes");
    assert!(engine.check().unwrap().issues.is_empty());
    assert!(
        fs::read_dir(config.disks[1].root.join("quarantine"))
            .unwrap()
            .next()
            .is_some()
    );
}
#[test]
fn recovery_uses_retained_workspace_bytes_when_all_candidate_replicas_are_lost() {
    let (_temp, root, config, branch, operation) = pending();
    for disk in &config.disks {
        fs::remove_dir_all(disk.root.join("histories")).unwrap();
    }
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(engine.list_tree(&branch).unwrap().len(), 1);
    assert!(engine.check().unwrap().issues.is_empty());
}

#[test]
fn pending_repair_keeps_quarantine_and_reprepared_sources_on_the_original_disks() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, system, config, _branch, operation) = pending();
    let engine = Engine::open_for_recovery(&system).unwrap();
    let member = &config.disks[0];
    let held = &engine.disk_resources.get(member).unwrap().root;
    let scanned = crate::journal::scan_at(held, member).unwrap();
    let hash = &scanned.records.last().unwrap().0.candidate_hash;
    let candidate: Candidate =
        crate::journal::load_object_at(held, member, "candidate", hash).unwrap();
    assert_eq!(candidate.operation_id, operation);
    let frozen = canonical_json(&candidate).unwrap();
    let version = &candidate.versions[0].version;
    let logs: Vec<_> = config
        .disks
        .iter()
        .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
        .collect();
    // The first healthy body becomes the repair source, then moves into the
    // quarantine bundle while its damaged metadata is replaced.
    for disk in &config.disks[..2] {
        fs::remove_file(
            version_dir(disk, &version.history_tree_id, &version.version_id).join("checksums.bin"),
        )
        .unwrap();
    }
    for disk in &config.disks {
        fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(disk.root.join("replacement-only"), b"not a repair source").unwrap();
    }
    engine.repair_pending_payloads(&candidate).unwrap();
    assert_eq!(canonical_json(&candidate).unwrap(), frozen);
    for (disk, original_log) in config.disks.iter().zip(logs) {
        let owner = engine.disk_resources.get(disk).unwrap();
        let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
        let mut bytes = Vec::new();
        let mut payload = owner.verify_payload(&directory, version).unwrap();
        payload.rewind().unwrap();
        payload.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"verified original bytes");
        assert_eq!(
            owner
                .root
                .read_bytes(&crate::journal::log_path(disk), original_log.len())
                .unwrap(),
            original_log
        );
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
    }
    assert!(
        fs::read_dir(
            config.disks[0]
                .root
                .with_extension("held")
                .join("quarantine")
        )
        .unwrap()
        .next()
        .is_some()
    );
}
