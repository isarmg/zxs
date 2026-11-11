use crate::files::{
    SaveStatus,
    deletion::{
        batch_restore,
        recovery::{Queue, Record, Retained},
        request::{Attempt, Intent},
    },
};
use json_codec::json;
use std::sync::Arc;
use storage_contract::{FileDeletionNode, OperationId};

fn attempt() -> Attempt {
    let original: OperationId = "op_11111111111111111111111111111111".parse().unwrap();
    let node: FileDeletionNode = json_codec::from_value(json!({
        "node_id":"fdel_11111111111111111111111111111111",
        "history_tree_id":"history_11111111111111111111111111111111",
        "file_id":"file_11111111111111111111111111111111",
        "repository_id":"repo_11111111111111111111111111111111",
        "path":"A.txt",
        "branch_heads":{"br_11111111111111111111111111111111":"ver_11111111111111111111111111111111"},
        "parent_node_ids":["ver_11111111111111111111111111111111"],
        "operation_id":original,"author":"alice","sequence":"1","created_at":"123"
    })).unwrap();
    let intent = Intent::UndoBatch(Arc::new(
        batch_restore::Request::admit(original, vec![node]).unwrap(),
    ));
    Attempt {
        epoch: 1,
        principal: "alice".into(),
        operation: "op_22222222222222222222222222222222".into(),
        request_digest: intent.digest("alice").unwrap(),
        intent,
        in_flight: false,
    }
}
#[test]
fn retained_batch_roundtrip_preserves_original_request_and_unknown_fence() {
    let attempt = attempt();
    let record = Record::from_attempt(&attempt).unwrap();
    let restored = Record::decode(&record.key(), "alice", &record.bytes).unwrap();
    assert_eq!(restored.intent, attempt.intent);
    assert_eq!(restored.operation.to_string(), attempt.operation);
    assert_eq!(restored.request_digest, attempt.request_digest);
    assert_eq!(restored.bytes, record.bytes);
    let mut queue = Queue::default();
    queue.merge(vec![Retained::unknown(restored)]).unwrap();
    assert!(!queue.settle(&attempt.operation, SaveStatus::Unknown));
    assert_eq!(queue.records.len(), 1);
    assert!(queue.records[0].settled.is_none());
}
#[test]
fn stored_batch_cannot_substitute_account_or_original_operation() {
    let record = Record::from_attempt(&attempt()).unwrap();
    assert!(Record::decode(&record.key(), "bob", &record.bytes).is_err());
    let changed = record.bytes.replace(
        "\"deletion_operation_id\":\"op_11111111111111111111111111111111\"",
        "\"deletion_operation_id\":\"op_33333333333333333333333333333333\"",
    );
    assert!(Record::decode(&record.key(), "alice", &changed).is_err());
}

#[test]
fn retained_batch_rejects_inconsistent_member_sets_before_recovery() {
    let record = Record::from_attempt(&attempt()).unwrap();
    let mut valid: json_codec::Value = json_codec::from_str(&record.bytes).unwrap();
    let mut second = valid["request"]["preview"]["deletion_nodes"][0].clone();
    second["file_id"] = json!("file_22222222222222222222222222222222");
    second["node_id"] = json!("fdel_22222222222222222222222222222222");
    second["path"] = json!("B.txt");
    second["branch_heads"] =
        json!({"br_22222222222222222222222222222222":"ver_22222222222222222222222222222222"});
    second["parent_node_ids"] = json!(["ver_22222222222222222222222222222222"]);
    valid["request"]["preview"]["deletion_nodes"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let decode = |value: &json_codec::Value| {
        Record::decode(
            &record.key(),
            "alice",
            &json_codec::to_string(value).unwrap(),
        )
    };
    assert!(decode(&valid).is_ok());
    for change in 0..8 {
        let mut damaged = valid.clone();
        let nodes = damaged["request"]["preview"]["deletion_nodes"]
            .as_array_mut()
            .unwrap();
        match change {
            0 => nodes[1] = nodes[0].clone(),
            1 => nodes.reverse(),
            2 => nodes[1]["node_id"] = nodes[0]["node_id"].clone(),
            3 => nodes[1]["repository_id"] = json!("repo_22222222222222222222222222222222"),
            4 => nodes[1]["history_tree_id"] = json!("history_22222222222222222222222222222222"),
            5 => nodes[1]["sequence"] = json!("2"),
            6 => nodes[1]["author"] = json!("bob"),
            7 => {
                for node in nodes {
                    node["sequence"] = json!("0");
                }
            }
            _ => unreachable!(),
        }
        assert!(decode(&damaged).is_err(), "case {change}");
    }
}

#[test]
fn stored_batch_rejects_positional_preview_and_member_arrays_without_rewriting() {
    let record = Record::from_attempt(&attempt()).unwrap();
    let valid: json_codec::Value = json_codec::from_str(&record.bytes).unwrap();
    let preview = &valid["request"]["preview"];
    let node = &preview["deletion_nodes"][0];
    let mut array_preview = valid.clone();
    array_preview["request"]["preview"] =
        json!([preview["deletion_operation_id"], preview["deletion_nodes"]]);
    let mut array_member = valid.clone();
    array_member["request"]["preview"]["deletion_nodes"][0] = json!([
        node["node_id"],
        node["repository_id"],
        node["history_tree_id"],
        node["file_id"],
        node["path"],
        node["branch_heads"],
        node["parent_node_ids"],
        node["operation_id"],
        node["author"],
        node["sequence"],
        node["created_at"]
    ]);
    for damaged in [array_preview, array_member] {
        let raw = json_codec::to_string(&damaged).unwrap();
        let retained = raw.clone();
        assert!(Record::decode(&record.key(), "alice", &raw).is_err());
        assert_eq!(raw, retained);
    }
    let admitted = Record::decode(&record.key(), "alice", &record.bytes).unwrap();
    assert_eq!(admitted.bytes, record.bytes);
}
