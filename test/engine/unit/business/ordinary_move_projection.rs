//! A pure logical projection test, not permission to install a whole-container move.
use super::*;
use name_policy::{LookupPath, Name, PortablePath};
use storage_contract::{CommitRequest, Mode, file_relocation::ObjectMoveRequest};
use std::os::unix::fs::PermissionsExt;

#[test]
fn move_replay_uses_the_same_ordinary_file_and_parent_without_extra_history() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(Mode::Development,
        [(Name::parse("Primary").unwrap(), disk)]).unwrap();
    let registration = config.disks[0].clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&registration.slot_id, "admin", OperationId::generate().unwrap()).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "folder/file.txt", b"one original body").unwrap();
    engine.commit_workspace(CommitRequest {
        operation_id: OperationId::generate().unwrap(), workspace_id: workspace.workspace_id,
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "original ordinary FILE".into(), principal: "admin".into(), request_digest: None,
    }).unwrap();
    let original = engine.namespace_tree(&repo.main_branch_id).unwrap()
        .get("folder/file.txt").unwrap().clone();
    let request = ObjectMoveRequest {
        source_repository_id: repo.repository_id.parse().unwrap(),
        source_entry_id: NamespaceEntryId::parse(&original.entry_id).unwrap(),
        source_path: LookupPath::parse("folder/file.txt").unwrap(),
        target_slot_id: registration.slot_id.clone(),
        target_repository_id: Some(repo.repository_id.parse().unwrap()),
        target_path: PortablePath::parse("folder/renamed.txt").unwrap(),
        replace: false, target_entry_id: None,
    };
    let operation = OperationId::generate().unwrap();
    let before = engine.state();
    let view = engine.admitted_current_namespaces().unwrap();
    let source = basis(before, view, &request).unwrap();
    let input = MoveRequest::new(&request, "admin").unwrap();
    let unit = capture(before, &input, &source, engine.config().max_entries).unwrap();
    let route = CopyRoute {
        created_at: now().unwrap(),
        bindings: [(registration.slot_id.clone(), registration.binding_generation)].into(),
        slots: vec![registration.slot_id],
    };
    let (next, receipt, facts) = compose(before, unit, source, &route, &input, operation, engine.next_slot).unwrap();
    verify_transition(before, &next, view, &receipt).unwrap();
    assert_eq!(next.versions, before.versions);
    assert_eq!(next.placements, before.placements);
    assert_eq!(next.namespace_directories, before.namespace_directories);
    let root_commit = &next.commits[&next.branches[&repo.main_branch_id].head_commit_id];
    let moved = root_commit.entries.iter().find(|entry| entry.path() == "folder/renamed.txt").unwrap();
    assert_eq!(moved.entry_id, original.entry_id);
    assert_eq!(moved.file_id, original.file_id);
    assert_eq!(moved.version_id, original.version_id);
    assert_eq!(facts.path_changes.len(), 1);
    // The original receipt cannot bless a different immutable body in the replayed projection.
    let mut wrong = next.clone();
    wrong.versions.get_mut(&original.version_id).unwrap().logical_size += 1;
    assert!(verify_transition(before, &wrong, view, &receipt).is_err());
    // No physical decision was issued, the live source stays untouched.
    assert!(engine.namespace_tree(&repo.main_branch_id).unwrap().get("folder/file.txt").is_some());
    assert!(engine.operation(operation).is_err());
}
