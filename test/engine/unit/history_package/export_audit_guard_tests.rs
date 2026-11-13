//! Temporary export safety fence, not evidence that portable audit is implemented.
use super::*;
use storage_contract::OperationId;

fn disk_image(root: &Path) -> BTreeMap<PathBuf, (u64, u32, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, rows: &mut BTreeMap<PathBuf, (u64, u32, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_dir() {
            Vec::new()
        } else if metadata.file_type().is_symlink() {
            fs::read_link(path).unwrap().as_os_str().as_encoded_bytes().to_vec()
        } else {
            assert!(metadata.is_file());
            fs::read(path).unwrap()
        };
        rows.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (metadata.ino(), metadata.mode(), bytes),
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), rows);
            }
        }
    }
    let mut rows = BTreeMap::new();
    visit(root, root, &mut rows);
    rows
}

fn assert_refused_without_writes(engine: &Engine, history: &str, root: &Path) {
    let existing = root.join("existing-output");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("owner-data"), b"must not replace or remove").unwrap();
    let state = canonical_json(engine.state()).unwrap();
    let image = disk_image(root);
    for destination in [root.join("missing-parent/package"), existing] {
        let error = engine.export_history_package(history, &destination).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
        assert!(matches!(
            error,
            Error::PathChange(crate::PathChangeError::PackageAuditUnsupported)
        ));
        assert_eq!(canonical_json(engine.state()).unwrap(), state);
        // Includes every source file's bytes, inode and mode, existing output,
        // and the absence of newly created parent/staging/package directories.
        assert_eq!(disk_image(root), image);
    }
}

#[test]
fn native_rename_export_refuses_before_writes_but_audit_free_packages_still_restore() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = initialize(&temporary.path().join("source"));
    let repo = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap()).unwrap();
    save(&mut engine, &repo.main_branch_id, b"immutable body");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File).unwrap();
    let body = engine.valid_replica(&entry.version_id).unwrap().0.path().to_owned();
    let inode = fs::metadata(&body).unwrap().ino();
    let proof = canonical_json(&engine.state.versions[&entry.version_id]).unwrap();
    let package = temporary.path().join("before-rename");
    engine.export_history_package(&repo.history_tree_id, &package).unwrap();
    let mut restored = initialize(&temporary.path().join("restored"));
    restored.restore_history_package(
        &package, "P01", 3, "alice", OperationId::generate().unwrap(),
    ).unwrap();
    assert_eq!(
        fs::read(restored.valid_replica(&entry.version_id).unwrap().0.path()).unwrap(),
        b"immutable body"
    );
    engine.file_namespace(
        &repo.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "file.txt", Some("renamed.txt"),
        ).unwrap(),
        "alice", OperationId::generate().unwrap(),
    ).unwrap();
    assert_eq!(canonical_json(&engine.state.versions[&entry.version_id]).unwrap(), proof);
    let renamed_body = engine.valid_replica(&entry.version_id).unwrap().0.path().to_owned();
    assert_eq!(fs::metadata(&renamed_body).unwrap().ino(), inode);
    assert_eq!(fs::read(&renamed_body).unwrap(), b"immutable body");
    assert_refused_without_writes(&engine, &repo.history_tree_id, temporary.path());
}

#[test]
fn retained_copy_path_audit_is_not_mistaken_for_an_audit_free_history() {
    let temporary = tempfile::tempdir().unwrap();
    let mut config = Config::development(
        (0..6).map(|index| temporary.path().join(format!("disk-{index}"))),
    ).unwrap();
    for disk in config.disks.iter_mut().skip(3) {
        disk.level = 1;
        disk.upstream_slot_id = None;
        disk.metadata_member = false;
    }
    let mut engine = Engine::initialize(&temporary.path().join("state"), config).unwrap();
    let source = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap()).unwrap();
    let clean = engine.create_disk_root("D04", "alice", OperationId::generate().unwrap()).unwrap();
    let copied = engine.create_disk_root("D05", "alice", OperationId::generate().unwrap()).unwrap();
    save(&mut engine, &source.main_branch_id, b"copy body");
    let request = |engine: &Engine, target: &crate::Repository, path: &str| {
        crate::FileHistoryCopyRequest {
            source_repository_id: source.repository_id.clone(),
            source_path: path.into(),
            expected_source_head: engine.state.branches[&source.main_branch_id]
                .head_commit_id.clone(),
            target_repository_id: target.repository_id.clone(),
            target_name: "copy.txt".into(),
            expected_target_head: engine.state.branches[&target.main_branch_id]
                .head_commit_id.clone(),
        }
    };
    let first = request(&engine, &clean, "file.txt");
    engine.copy_file_history(&first, "alice", OperationId::generate().unwrap()).unwrap();
    engine.file_namespace(
        &source.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "file.txt", Some("renamed.txt"),
        ).unwrap(),
        "alice", OperationId::generate().unwrap(),
    ).unwrap();
    // An unrelated native rename must not prevent the selected clean history's export.
    engine.export_history_package(&clean.history_tree_id, &temporary.path().join("clean-copy"))
        .unwrap();
    let second = request(&engine, &copied, "renamed.txt");
    let receipt = engine.copy_file_history(&second, "alice", OperationId::generate().unwrap())
        .unwrap();
    let file = receipt.result["file_id"].as_str().unwrap();
    let retained = crate::business::history_copy::retained::read(&receipt, file).unwrap();
    assert_eq!(retained.path_changes.len(), 1);
    assert_ne!(retained.path_changes[0].source.history_tree_id.to_string(), copied.history_tree_id);
    assert_eq!(retained.path_changes[0].history_tree_id.to_string(), copied.history_tree_id);
    assert_refused_without_writes(&engine, &copied.history_tree_id, temporary.path());
}
