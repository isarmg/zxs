//! The live Backup boundary must reject mixed receipts before persisting its
//! own intent. This injects an in-memory bad row, not a fabricated native seal.
use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

type Media = BTreeMap<PathBuf, (u64, u64, u32, Option<Vec<u8>>)>;

fn media(root: &Path) -> Media {
    fn visit(base: &Path, path: &Path, result: &mut Media) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let bytes = if metadata.is_dir() { None } else { Some(fs::read(path).unwrap()) };
        result.insert(path.strip_prefix(base).unwrap().to_path_buf(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(base, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}


#[test]
fn backup_rejects_retired_or_disguised_receipts_before_intent_and_preserves_opaque_file() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700)).tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::development(crate::test_disks(
        (0..3).map(|index| temp.path().join(format!("disk-{index}"))),
    )).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let op = || OperationId::generate().unwrap();
    let repo = engine.create_disk_root("P01", "admin", op()).unwrap();
    let work = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&work.workspace_id, "project.zxs", b"opaque ZIP bytes").unwrap();
    let save = op();
    engine.commit_workspace(CommitRequest {
        workspace_id: work.workspace_id, operation_id: save, principal: "admin".into(),
        expected_head: work.base_head, fencing_token: work.fencing_token,
        message: "ordinary FILE".into(), request_digest: None,
    }).unwrap();
    let target = temp.path().join("selected");
    fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
    let request = BackupPreparationRequest { operation_id: op(), principal: "admin".into(),
        source_slot_id: "B01".into(), target_paths: vec![target.clone()],
        expected_sequence: engine.state().sequence };
    engine.check_backup_preparation_scope(&request).unwrap();
    let admitted = Arc::clone(&engine.state);
    let original = media(temp.path());
    for kind in [Some("snapshot.repository.reserve"), None] {
        let receipt = Arc::make_mut(&mut engine.state).operations.get_mut(&save).unwrap();
        match kind {
            Some(kind) => receipt.kind = kind.into(),
            None => receipt.result["snapshot_worktree"] = json_codec::json!({}),
        }
        let error = engine.start_backup_preparation(request.clone(), File::open(&target).unwrap())
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::StateInvalid);
        assert!(engine.backup_preparation_status().is_none());
        assert!(!system.join(JOB).exists());
        assert_eq!(media(temp.path()), original);
        engine.state = Arc::clone(&admitted);
    }
    drop(engine);
    let reopened = Engine::open(&system).unwrap();
    assert_eq!(reopened.state().operations[&save], admitted.operations[&save]);
    let view = reopened.namespace_tree(&repo.main_branch_id).unwrap();
    let entry = view.get("project.zxs").unwrap();
    assert_eq!(entry.kind, storage_contract::NamespaceEntryKind::File);
    let mut bytes = Vec::new();
    reopened.open_content(&entry.version_id).unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"opaque ZIP bytes");
    assert!(reopened.backup_preparation_status().is_none());
}
