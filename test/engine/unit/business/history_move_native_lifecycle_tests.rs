//! Pure receipt/lifecycle boundary fixtures, not journal or full State proofs.
//! The existing move_candidate integration target exercises real stored objects.
use super::*;

fn id(prefix: &str, number: u32) -> String {
    format!("{prefix}_{number:032x}")
}

struct Fixture {
    state: State,
    operation: OperationId,
    node: FileDeletionNode,
    target_file: String,
}

fn repository(number: u32, slot: &str) -> Repository {
    Repository {
        repository_id: id("repo", number),
        name: format!("root-{number}"),
        history_tree_id: id("history", number),
        main_branch_id: id("br", 100 + number),
        primary_slot_id: slot.into(),
        target_copies: 1,
        name_policy: "portable".into(),
        unicode_version: "17.0.0".into(),
        smb_enabled: true,
        smb_binding_generation: 1,
        file_manager_root: true,
    }
}

impl Fixture {
    fn new() -> Self {
        let operation: OperationId = id("op", 10).parse().unwrap();
        let source = repository(1, "P01");
        let target = repository(2, "D04");
        let source_file = id("file", 1);
        let source_version = id("ver", 1);
        let target_file = history_copy::identity(operation, &source_file).unwrap();
        let target_version = history_copy::identity(operation, &source_version).unwrap();
        let commit_id = history_copy::identity(operation, "commit_arrival").unwrap();
        let request = FileHistoryCopyRequest {
            source_repository_id: source.repository_id.clone(),
            source_path: "source.txt".into(),
            expected_source_head: id("commit", 1),
            target_repository_id: target.repository_id.clone(),
            target_name: "moved.txt".into(),
            expected_target_head: id("commit", 2),
        };
        let hash = object_hash(
            "file-history-move-deletion",
            &json!([operation, source_file]),
        )
        .unwrap();
        let node = FileDeletionNode {
            node_id: format!("fdel_{}", &hash[..32]),
            repository_id: source.repository_id.clone(),
            history_tree_id: source.history_tree_id.clone(),
            file_id: source_file.clone(),
            path: name_policy::PortablePath::parse("source.txt").unwrap(),
            branch_heads: [(id("br", 1), source_version.clone())].into(),
            parent_node_ids: vec![source_version.clone()],
            operation_id: operation,
            author: "admin".into(),
            sequence: 11,
            created_at: 1234,
        };
        let route = CopyRoute {
            created_at: node.created_at,
            slots: vec!["D04".into()],
            bindings: [("D04".into(), 7)].into(),
        };
        let lifecycle = CopiedLifecycle {
            source_file_id: source_file.clone(),
            source_history_tree_id: source.history_tree_id.clone(),
            source_sequence: 10,
            source_records_hash: "a".repeat(64),
            file_id: target_file.clone(),
            history_tree_id: target.history_tree_id.clone(),
            events: Vec::new(),
        };
        let versions = [(source_version, target_version.clone())].into();
        let branches = [(
            id("br", 1),
            history_copy::identity(operation, &id("br", 1)).unwrap(),
        )]
        .into();
        let input = Request {
            namespace: &request,
            source_file_id: &source_file,
            replacement: None,
        };
        let digest = Engine::native_request_digest(KIND, "admin", &json!(input)).unwrap();
        let body = ResultBody {
            move_request: input,
            move_route: &route,
            source_file_id: &source_file,
            file_id: &target_file,
            source_sequence: 10,
            source_records_hash: lifecycle.source_records_hash.clone(),
            deletion_node: &node,
            replacement: None,
            retained_lifecycle: &lifecycle,
            version_ids: vec![target_version.clone()],
            version_mapping: &versions,
            branch_mapping: &branches,
            branch_id: &target.main_branch_id,
            history_tree_id: &target.history_tree_id,
            commit_id: &commit_id,
            branch_generation: 1,
            timestamp: node.created_at,
            membership_epoch: 1,
            recovery_digest: "b".repeat(64),
            decision_inputs_digest: "c".repeat(64),
            version_protection: vec![Version {
                version_id: target_version.clone(),
                primary_slot_id: "D04".into(),
                target_copies: 1,
                required_slots: route.slots.clone(),
                topology_generation: 1,
                slots: vec![Slot {
                    slot_id: "D04".into(),
                    copy_level: 1,
                    disk_id: id("disk", 4),
                    binding_generation: 7,
                    state: SlotState::CompleteVerified,
                }],
            }],
        };
        let receipt = OperationResult {
            operation_id: operation,
            kind: KIND.into(),
            commit_id: Some(commit_id.clone()),
            history_tree_id: Some(target.history_tree_id.clone()),
            branch_id: Some(target.main_branch_id.clone()),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::TargetMet,
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: node.sequence,
            result: json_codec::to_value(body).unwrap(),
        };
        let mut state = State {
            sequence: node.sequence,
            ..State::default()
        };
        state.commits.insert(
            commit_id.clone(),
            Commit {
                commit_id: commit_id.clone(),
                history_tree_id: target.history_tree_id.clone(),
                kind: "file_history_move".into(),
                parent_commit_ids: vec![request.expected_target_head],
                tree_id: history_copy::identity(operation, "tree_arrival").unwrap(),
                sequence: node.sequence,
                operation_id: operation,
                author: "admin".into(),
                message: "Move complete file history; soft delete source".into(),
                entries: vec![Entry {
                    entry_id: id("entry", 1),
                    path: name_policy::PortablePath::parse("moved.txt").unwrap(),
                    kind: NamespaceEntryKind::File,
                    file_id: target_file.clone(),
                    version_id: target_version,
                    size: 4,
                }],
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            },
        );
        for repository in [source, target] {
            state
                .repositories
                .insert(repository.repository_id.clone(), repository);
        }
        state.operations.insert(operation, receipt);
        state.operation_principals.insert(operation, "admin".into());
        state.file_deletions.insert(source_file, node.clone());
        state.events.push(Event {
            sequence: node.sequence,
            operation_id: operation,
            kind: KIND.into(),
            object_id: commit_id,
        });
        Self {
            state,
            operation,
            node,
            target_file,
        }
    }

    fn admit(&self) -> Result<()> {
        crate::file_deletion::publication::admit_current(&self.state)
    }

    fn undo(&mut self) -> OperationId {
        let operation: OperationId = id("op", 12).parse().unwrap();
        self.state.sequence = 12;
        let input = json!({"file_id": self.node.file_id, "deletion_node_id": self.node.node_id});
        let receipt = OperationResult {
            operation_id: operation,
            kind: "file.delete.undo".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest("file.delete.undo", "restorer", &input)
                .unwrap(),
            sequence: 12,
            result: json!({"removed_deletion_node": self.node, "restored_file_id": self.node.file_id,
                "timestamp": "1235", "membership_epoch": "1", "version_protection": []}),
        };
        self.state
            .file_deletions
            .remove(&self.node.file_id)
            .unwrap();
        self.state.operations.insert(operation, receipt);
        self.state
            .operation_principals
            .insert(operation, "restorer".into());
        self.state.events.push(Event {
            sequence: 12,
            operation_id: operation,
            kind: "file.delete.undo".into(),
            object_id: operation.to_string(),
        });
        operation
    }
}

#[test]
fn one_native_receipt_creates_only_its_source_deletion() {
    let mut fixture = Fixture::new();
    fixture.admit().unwrap();
    assert_eq!(fixture.state.operations.len(), 1);
    assert_eq!(
        admit_source_deletion(
            &fixture.state,
            &fixture.state.operations[&fixture.operation]
        )
        .unwrap(),
        fixture.node
    );
    let mut extra = fixture.node.clone();
    extra.file_id = fixture.target_file.clone();
    fixture
        .state
        .file_deletions
        .insert(extra.file_id.clone(), extra);
    assert!(fixture.admit().is_err());
}

#[test]
fn prospective_staging_or_degraded_receipts_do_not_become_cold_success() {
    let changes: [fn(&mut OperationResult); 6] = [
        |receipt| receipt.commit_state = CommitState::Staging,
        |receipt| receipt.commit_state = CommitState::PublishedDegraded,
        |receipt| receipt.protection_at_publish = ProtectionAtPublish::NotApplicable,
        |receipt| receipt.current_health = CurrentHealth::Unknown,
        |receipt| {
            receipt
                .result
                .as_object_mut()
                .unwrap()
                .remove("decision_inputs_digest");
        },
        |receipt| receipt.result["accepted"] = json!(true),
    ];
    for change in changes {
        let mut fixture = Fixture::new();
        change(
            fixture
                .state
                .operations
                .get_mut(&fixture.operation)
                .unwrap(),
        );
        assert!(fixture.admit().is_err());
    }
}

#[test]
fn source_node_request_clock_and_both_identity_maps_remain_bound() {
    let changes: [fn(&mut json_codec::Value); 10] = [
        |body| body["deletion_node"]["node_id"] = json!(id("fdel", 99)),
        |body| body["deletion_node"]["author"] = json!("other"),
        |body| body["deletion_node"]["created_at"] = json!("999"),
        |body| body["move_request"]["namespace"]["target_name"] = json!("another.txt"),
        |body| body["branch_mapping"] = json!({}),
        |body| body["version_mapping"] = json!({}),
        |body| body["version_ids"] = json!([]),
        |body| body["retained_lifecycle"]["source_records_hash"] = json!("d".repeat(64)),
        |body| body["move_route"]["bindings"]["D04"] = json!("8"),
        |body| body["version_protection"][0]["slots"][0]["state"] = json!("COMPLETE"),
    ];
    for change in changes {
        let mut fixture = Fixture::new();
        change(
            &mut fixture
                .state
                .operations
                .get_mut(&fixture.operation)
                .unwrap()
                .result,
        );
        assert!(fixture.admit().is_err());
    }
}

#[test]
fn move_event_is_the_exact_arrival_not_a_fabricated_delete_event() {
    let mut fixture = Fixture::new();
    fixture.state.events[0].object_id = fixture.operation.to_string();
    assert!(fixture.admit().is_err());
    let mut fixture = Fixture::new();
    let event = fixture.state.events[0].clone();
    fixture.state.events.push(event);
    assert!(fixture.admit().is_err());
    let mut fixture = Fixture::new();
    let commit = fixture.state.operations[&fixture.operation]
        .commit_id
        .clone()
        .unwrap();
    fixture
        .state
        .commits
        .get_mut(&commit)
        .unwrap()
        .entries
        .clear();
    assert!(fixture.admit().is_err());
}

#[test]
fn exact_undo_consumes_the_move_d_without_rewriting_its_original_receipt() {
    let mut fixture = Fixture::new();
    let original = fixture.state.operations[&fixture.operation].clone();
    let undo = fixture.undo();
    fixture.admit().unwrap();
    assert!(fixture.state.file_deletions.is_empty());
    assert_eq!(fixture.state.operations[&fixture.operation], original);
    fixture.state.operations.get_mut(&undo).unwrap().result["removed_deletion_node"]["created_at"] =
        json!("999");
    assert!(fixture.admit().is_err());
}

#[test]
fn later_ordinary_delete_joins_the_same_ordered_lifecycle() {
    let mut fixture = Fixture::new();
    fixture.undo();
    let operation: OperationId = id("op", 13).parse().unwrap();
    let mut node = fixture.node.clone();
    node.node_id = id("fdel", 13);
    node.operation_id = operation;
    node.sequence = 13;
    node.created_at = 1236;
    let input = json!({"repository_id": node.repository_id, "file_id": node.file_id, "path": node.path.key()});
    fixture.state.sequence = 13;
    fixture.state.operations.insert(operation, OperationResult {
        operation_id: operation,
        kind: "file.delete".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: Engine::native_request_digest("file.delete", "admin", &input).unwrap(),
        sequence: 13,
        result: json!({"deletion_node": node, "timestamp": "1236", "membership_epoch": "1", "version_protection": []}),
    });
    fixture
        .state
        .operation_principals
        .insert(operation, "admin".into());
    fixture.state.events.push(Event {
        sequence: 13,
        operation_id: operation,
        kind: "file.delete".into(),
        object_id: operation.to_string(),
    });
    fixture
        .state
        .file_deletions
        .insert(node.file_id.clone(), node);
    fixture.admit().unwrap();
}

#[test]
fn imported_audit_count_is_bounded_before_decoding_any_node() {
    let mut fixture = Fixture::new();
    fixture
        .state
        .operations
        .get_mut(&fixture.operation)
        .unwrap()
        .result["retained_lifecycle"]["events"] = json!(vec![json_codec::Value::Null; 65_537]);
    assert_eq!(
        fixture.admit().unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn cold_lifecycle_admission_still_cannot_authorize_ordinary_publication() {
    let fixture = Fixture::new();
    fixture.admit().unwrap();
    assert_eq!(
        reject_unclosed_publication(
            &State::default(),
            &fixture.state,
            &fixture.state.operations[&fixture.operation],
        )
        .unwrap_err()
        .code(),
        ErrorCode::UnsupportedOperation,
    );
}
