//! Allocation-free encoded-length counting and pre-decode event count fences.
use super::*;

#[test]
fn retained_history_copy_budget_counts_exact_encoded_bytes_without_a_value_copy() {
    bounded(&"x".repeat(MAX_BYTES - 2)).unwrap();
    assert_eq!(
        bounded(&"x".repeat(MAX_BYTES - 1)).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    // Escaped characters count their encoded representation, not string length.
    assert_eq!(
        bounded(&"\n".repeat(MAX_BYTES / 2)).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn retained_history_copy_budget_rejects_event_count_before_decoding_any_node() {
    let receipt = OperationResult {
        operation_id: OperationId::generate().unwrap(),
        kind: RETAINED_KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: crate::CommitState::Published,
        protection_at_publish: crate::ProtectionAtPublish::TargetMet,
        current_health: crate::CurrentHealth::Healthy,
        request_digest: "count-only".into(),
        sequence: 1,
        result: json!({"retained_lifecycle":{"events":vec![Value::Null; 65_537]}}),
    };
    assert_eq!(
        read(&receipt, "not-decoded").unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn path_fact_prefix_survives_graph_copy_recopy_and_cold_open_without_source_retargeting() {
    use crate::{CommitRequest, Config, Engine, MembershipExitAction, NamespaceOperation};
    use storage_contract::{NamespaceOperationKind, history_window::HistoryFileId};
    for current in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let mut config = Config::development(
            (0..6).map(|index| temporary.path().join(format!("disk-{index}"))),
        ).unwrap();
        for disk in config.disks.iter_mut().skip(3) {
            disk.level = 1;
            disk.upstream_slot_id = None;
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&system, config.clone()).unwrap();
        let source = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
        let target = engine.create_disk_root("D04", "admin", OperationId::generate().unwrap()).unwrap();
        let third = engine.create_disk_root("D05", "admin", OperationId::generate().unwrap()).unwrap();
        let workspace = engine.create_workspace(&source.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "a.txt", b"real source body").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "path provenance source".into(),
            request_digest: None,
        }).unwrap();
        if current {
            engine.begin_disk_exit(
                &config.disks[5].disk_id,
                MembershipExitAction::Deregister,
                "admin",
                OperationId::generate().unwrap(),
            ).unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine.enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new()).unwrap();
            assert!(engine.resume_membership_joint_decision().unwrap().all_certificates_synchronized());
            engine.activate_pending_disk_exit().unwrap();
        }
        let moved = engine.file_namespace(
            &source.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "a.txt", Some("b.txt")).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        ).unwrap();
        let original = crate::persist::path_changes::read(&moved).unwrap().remove(0);
        let request = FileHistoryCopyRequest {
            source_repository_id: source.repository_id.clone(),
            source_path: "b.txt".into(),
            expected_source_head: engine.state().branches[&source.main_branch_id].head_commit_id.clone(),
            target_repository_id: target.repository_id.clone(),
            target_name: "copy.txt".into(),
            expected_target_head: engine.state().branches[&target.main_branch_id].head_commit_id.clone(),
        };
        let first = engine.copy_file_history(&request, "admin", OperationId::generate().unwrap()).unwrap();
        let target_file = first.result["file_id"].as_str().unwrap().to_owned();
        let prefix = read(&first, &target_file).unwrap();
        assert_eq!(prefix.path_changes.len(), 1);
        assert_eq!(prefix.path_changes[0].source, original);
        assert_ne!(prefix.path_changes[0].file_id, original.file_id);
        assert_ne!(prefix.path_changes[0].before_version_id, original.before_version_id);
        let target_move = engine.file_namespace(
            &target.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename, "copy.txt", Some("renamed-copy.txt"),
            ).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        ).unwrap();
        let target_fact = crate::persist::path_changes::read(&target_move).unwrap().remove(0);
        let again = FileHistoryCopyRequest {
            source_repository_id: target.repository_id.clone(),
            source_path: "renamed-copy.txt".into(),
            expected_source_head: engine.state().branches[&target.main_branch_id].head_commit_id.clone(),
            target_repository_id: third.repository_id.clone(),
            target_name: "third.txt".into(),
            expected_target_head: engine.state().branches[&third.main_branch_id].head_commit_id.clone(),
        };
        let second = engine.copy_retained_file_history(
            &again, "admin", OperationId::generate().unwrap(),
        ).unwrap();
        let third_file = second.result["file_id"].as_str().unwrap().to_owned();
        let twice = read(&second, &third_file).unwrap();
        assert_eq!(twice.path_changes.len(), 2);
        assert_eq!(twice.path_changes[0].source, original);
        assert_eq!(twice.path_changes[1].source, target_fact);
        assert!(twice.path_changes.iter().all(|fact| fact.file_id.to_string() == third_file));
        assert_ne!(twice.path_changes[0].before_version_id, prefix.path_changes[0].before_version_id);
        engine.file_namespace(
            &source.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "b.txt", Some("later.txt")).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        ).unwrap();
        drop(engine);
        let engine = Engine::open(&system).unwrap();
        let projection = crate::state_projection::ProjectionSnapshot::new(engine.state());
        let file = HistoryFileId::parse(&third_file).unwrap();
        assert_eq!(path_changes(projection.view(), &file).unwrap(), twice.path_changes);
        assert!(engine.state().file_path_nodes.is_empty());
        let recaptured = crate::container_storage::file_database::capture_projected_copy(
            projection.view(), &third_file, &third.history_tree_id,
        ).unwrap();
        assert!(!recaptured.records_hash.is_empty());
    }
}

#[test]
fn retained_prefix_requires_path_field_and_one_shared_event_budget_before_decode() {
    let missing = json!({"events": []});
    assert!(require_event_budget(&missing).is_err());
    let combined = json!({
        "events": vec![Value::Null; 32_768],
        "path_changes": vec![Value::Null; 32_769],
    });
    assert_eq!(require_event_budget(&combined).unwrap_err().code(), ErrorCode::ResourceLimit);
    require_event_budget(&json!({"events": [], "path_changes": []})).unwrap();
}
