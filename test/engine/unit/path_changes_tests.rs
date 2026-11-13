use crate::persist::path_changes::{PathChanged, attach, identity, read};
use crate::OperationResult;
use storage_contract::file_lifecycle::PathLocation;
use storage_contract::{
    CommitState, CurrentHealth, Principal, ProtectionAtPublish, Sequence, Timestamp,
};

fn fact() -> PathChanged {
    let operation_id = "op_00000000000000000000000000000001".parse().unwrap();
    let file_id = "file_00000000000000000000000000000002".parse().unwrap();
    let entry_id = "entry_00000000000000000000000000000003".parse().unwrap();
    let repository_id = "repo_00000000000000000000000000000004".parse().unwrap();
    let before = PathLocation {
        repository_id,
        path: name_policy::PortablePath::parse("old/a.txt").unwrap(),
    };
    let after = PathLocation {
        repository_id,
        path: name_policy::PortablePath::parse("new/a.txt").unwrap(),
    };
    PathChanged {
        event_id: identity(operation_id, file_id, entry_id, &before, &after).unwrap(),
        operation_id,
        file_id,
        entry_id,
        history_tree_id: "history_00000000000000000000000000000005".parse().unwrap(),
        branch_id: "br_00000000000000000000000000000006".parse().unwrap(),
        before_version_id: "ver_00000000000000000000000000000007".parse().unwrap(),
        after_version_id: "ver_00000000000000000000000000000007".parse().unwrap(),
        before,
        after,
        sequence: Sequence::new(17),
        occurred_at: Timestamp::from_unix_seconds(19),
        author: Principal::new("admin".into()).unwrap(),
    }
}

fn receipt(fact: PathChanged) -> OperationResult {
    OperationResult {
        operation_id: fact.operation_id,
        kind: "workspace.commit".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: "a".repeat(64),
        sequence: fact.sequence.get(),
        result: attach(json_codec::json!({}), vec![fact]).unwrap(),
    }
}

#[test]
fn permanent_path_facts_bind_identity_location_operation_and_sequence() {
    let original = fact();
    let record = receipt(original.clone());
    assert_eq!(read(&record).unwrap(), vec![original.clone()]);
    let mut forged = record.clone();
    forged.sequence += 1;
    assert!(read(&forged).is_err());
    let mut forged = record.clone();
    forged.operation_id = "op_00000000000000000000000000000008".parse().unwrap();
    assert!(read(&forged).is_err());
    let mut forged = record.clone();
    forged.result["path_changes"][0]["after"]["path"] = json_codec::json!("other.txt");
    assert!(read(&forged).is_err());
    let mut alias = original.clone();
    alias.before.path = name_policy::PortablePath::parse("alias/a.txt").unwrap();
    alias.event_id = identity(
        alias.operation_id, alias.file_id, alias.entry_id, &alias.before, &alias.after,
    ).unwrap();
    assert_ne!(alias.event_id, original.event_id);
    let result = attach(json_codec::json!({}), vec![alias, original]).unwrap();
    let both = read(&OperationResult { result, ..record }).unwrap();
    assert_eq!(both.len(), 2);
    assert!(both[0].event_id < both[1].event_id);
}

#[test]
fn path_facts_reject_missing_unknown_duplicate_and_noncanonical_records() {
    let record = receipt(fact());
    let mut missing = record.clone();
    missing.result = json_codec::json!({});
    assert!(read(&missing).is_err());
    let mut unknown = record.clone();
    unknown.result["path_changes"][0]["node_id"] = json_codec::json!("old-node");
    assert!(read(&unknown).is_err());
    let mut duplicate = record.clone();
    let value = duplicate.result["path_changes"][0].clone();
    duplicate.result["path_changes"] = json_codec::json!([value.clone(), value]);
    assert!(read(&duplicate).is_err());
    assert!(attach(json_codec::json!({}), vec![fact(), fact()]).is_err());
    assert!(attach(record.result, Vec::new()).is_err());
    let mut unchanged = fact();
    unchanged.after = unchanged.before.clone();
    unchanged.event_id = identity(
        unchanged.operation_id, unchanged.file_id, unchanged.entry_id,
        &unchanged.before, &unchanged.after,
    ).unwrap();
    assert!(attach(json_codec::json!({}), vec![unchanged]).is_err());
}
