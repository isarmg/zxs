//! An ordinary disk name or first path component cannot select a different
//! namespace. All generated media in these proofs belong to this test.
use super::*;
use storage_contract::OperationId;

fn op() -> OperationId { OperationId::generate().unwrap() }
fn owner() -> Identity {
    Identity { admission_session: None, smb_binding: None, principal: "owner".into() }
}
fn folder(engine: &mut Engine, slot: &str, path: &str, operation_id: OperationId)
    -> Result<storage_contract::OperationResult, ApiError> {
    mkdir(engine, &owner(), crate::CreateDiskDirectory {
        slot_id: slot.into(), path: name_policy::PortablePath::parse(path).unwrap(), operation_id,
    })
}
fn lookup(engine: &mut Engine, slot: &str, path: &str) -> Result<crate::SmbPathLocation, ApiError> {
    resolve_smb_path(engine, &owner(), crate::ResolveSmbPath {
        slot_id: slot.into(), path: name_policy::LookupPath::parse(path).unwrap(),
        intent: crate::SmbPathIntent::Lookup,
    })
}
fn workspace(engine: &mut Engine, slot: &str, path: &str) -> Result<crate::DiskWriteSession, ApiError> {
    write_session(engine, &owner(), crate::CreateDiskWriteSession {
        slot_id: slot.into(), path: name_policy::PortablePath::parse(path).unwrap(), branch_id: None,
    })
}
fn setup() -> (tempfile::TempDir, Engine, String, String) {
    let root = crate::tests::private_tempdir();
    let config = storage_engine::Config::independent_roots(storage_contract::Mode::Development,
        crate::test_disks([root.path().join("one"), root.path().join("two")])).unwrap();
    let slots = config.disks.iter().map(|d| d.slot_id.clone()).collect::<Vec<_>>();
    let engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    (root, engine, slots[0].clone(), slots[1].clone())
}

#[test]
fn a_used_operation_and_a_read_cannot_register_another_disk() {
    let (_root, mut engine, a, b) = setup();
    let original = op();
    let first = folder(&mut engine, &a, "current", original).unwrap();
    let before = engine.state().clone();
    assert!(lookup(&mut engine, &b, "current").is_err());
    let error = folder(&mut engine, &b, "new", original).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::IdempotencyKeyReused);
    assert!(engine.ordinary_root_for_slot(&b).unwrap().is_none());
    assert_eq!(engine.state(), &before);
    // Same disk and same original mkdir still have one native outcome.
    assert_eq!(folder(&mut engine, &a, "current", original).unwrap(), first);
    assert_eq!(engine.state(), &before);
}

#[test]
fn smb_and_upload_preserve_the_entire_path_in_the_original_slot() {
    let (root, mut engine, a, b) = setup();
    folder(&mut engine, &b, "only-on-second", op()).unwrap();
    let second = engine.ordinary_root_for_slot(&b).unwrap().unwrap().clone();
    // Use the actual other registered name: it is still just an ordinary folder here.
    let directory = format!("{}/资料", second.name);
    folder(&mut engine, &a, &directory, op()).unwrap();
    let first = engine.ordinary_root_for_slot(&a).unwrap().unwrap().clone();
    assert_ne!(first.repository_id, second.repository_id);
    let path = format!("{directory}/project.zxs");
    let smb = lookup(&mut engine, &a, &path).unwrap();
    assert_eq!(smb.repository.repository_id, first.repository_id);
    let crate::SmbRelativePath::Entry(relative) = smb.relative_path else { panic!("ordinary path lost") };
    assert_eq!(relative.as_str(), path);
    let session = workspace(&mut engine, &a, &path).unwrap();
    assert_eq!(session.relative_path, path);
    assert_eq!(session.display_path, path);
    assert_eq!(session.workspace.branch_id, first.main_branch_id);
    engine.stage_bytes(&session.workspace.workspace_id, &session.relative_path, b"opaque ZIP").unwrap();
    engine.commit_workspace(storage_engine::CommitRequest {
        workspace_id: session.workspace.workspace_id, operation_id: op(), principal: "owner".into(),
        expected_head: session.workspace.base_head, fencing_token: session.workspace.fencing_token,
        message: String::new(), request_digest: None,
    }).unwrap();
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert!(engine.namespace_tree(&first.main_branch_id).unwrap().get(&path).is_some());
    assert!(engine.namespace_tree(&second.main_branch_id).unwrap().get(&path).is_none());
    assert!(engine.namespace_tree(&second.main_branch_id).unwrap().get("only-on-second").is_some());
}

#[test]
fn a_same_spelling_replacement_never_grants_smb_read_or_a_new_workspace() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut engine, slot, _) = setup();
    folder(&mut engine, &slot, "docs", op()).unwrap();
    let before = engine.state().clone();
    let capacity = engine.workspace_capacity("owner");
    let common = root.path().join("one/common-data");
    std::fs::rename(&common, root.path().join("held-original")).unwrap();
    std::fs::create_dir(&common).unwrap();
    std::fs::set_permissions(&common, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(lookup(&mut engine, &slot, "docs").is_err());
    assert!(workspace(&mut engine, &slot, "docs/report").is_err());
    assert!(folder(&mut engine, &slot, "new", op()).is_err());
    assert_eq!(engine.state(), &before);
    assert_eq!(engine.workspace_capacity("owner"), capacity);
    assert!(std::fs::read_dir(common).unwrap().next().is_none());
}
