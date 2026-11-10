//! Shared request/receipt boundaries. Publication evidence is tested by engine
//! integration tests; constructing a fact here is not a publication proof.
use crate::file_relocation::{
    NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveFact, ObjectMoveRequest,
};
use crate::history_window::{HistoryFileId, HistoryRepositoryId};
use json_codec::{json, Value};

fn request(empty: bool) -> ObjectMoveRequest {
    ObjectMoveRequest {
        source_repository_id: repository(1),
        source_entry_id: entry(1),
        source_path: name_policy::LookupPath::parse("folder/e\u{301}.txt").unwrap(),
        target_slot_id: "P02".into(),
        target_repository_id: (!empty).then(|| repository(2)),
        target_path: name_policy::PortablePath::parse("folder/é.txt").unwrap(),
        replace: false,
        target_entry_id: None,
    }
}

fn repository(id: u8) -> HistoryRepositoryId {
    HistoryRepositoryId::parse(&format!("repo_{id:032x}")).unwrap()
}

fn entry(id: u8) -> NamespaceEntryId {
    NamespaceEntryId::parse(&format!("entry_{id:032x}")).unwrap()
}

fn file(id: u8) -> HistoryFileId {
    HistoryFileId::parse(&format!("file_{id:032x}")).unwrap()
}

fn receipt(request: &ObjectMoveRequest) -> Value {
    json!({
        "operation_kind": OBJECT_MOVE_KIND,
        "request_digest": request.digest("alice").unwrap(),
        "result": {
            "movement": ObjectMoveFact {
                request: request.clone(),
                principal: "alice".into(),
                target_repository_id: repository(2),
                target_entry_id: entry(2),
                source_file_id: Some(file(1)),
                displaced_target_file_id: request.target_entry_id.as_ref().map(|_| file(2)),
            },
        },
    })
}

#[test]
fn original_lookup_spelling_and_explicit_target_absence_survive_wire_roundtrip() {
    for empty in [false, true] {
        let original = request(empty);
        let wire = json_codec::to_value(&original).unwrap();
        assert_eq!(wire["source_path"], "folder/e\u{301}.txt");
        assert_eq!(wire["target_path"], "folder/é.txt");
        assert_eq!(wire["target_entry_id"], Value::Null);
        assert_eq!(wire["target_repository_id"].is_null(), empty);
        assert_eq!(
            crate::decode_strict::<ObjectMoveRequest>(&json_codec::to_vec(&wire).unwrap()).unwrap(),
            original
        );
        for key in ["target_repository_id", "target_entry_id"] {
            let mut missing = wire.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(
                json_codec::from_value::<ObjectMoveRequest>(missing).is_err(),
                "{key}"
            );
        }
        let mut unknown = wire;
        unknown["source_file_id"] = json!("file_00000000000000000000000000000001");
        assert!(json_codec::from_value::<ObjectMoveRequest>(unknown).is_err());
    }
}

#[test]
fn occupied_target_requires_replacement_and_a_real_target_repository() {
    let mut movement = request(false);
    movement.require_target_intent().unwrap();
    movement.replace = true;
    movement.require_target_intent().unwrap();
    movement.target_entry_id = Some(entry(2));
    movement.require_target_intent().unwrap();
    movement.replace = false;
    assert!(movement.require_target_intent().is_err());
    movement.replace = true;
    movement.target_repository_id = None;
    assert!(movement.require_target_intent().is_err());
    movement.target_entry_id = None;
    movement.require_target_intent().unwrap();
    movement.target_slot_id.clear();
    assert!(movement.require_target_intent().is_err());
}

#[test]
fn receipt_matches_captured_request_including_empty_disk_intention() {
    for empty in [false, true] {
        let original = request(empty);
        let published = receipt(&original);
        assert!(original.matches_receipt("alice", &published));
        assert!(!original.matches_receipt("bob", &published));

        // An empty-disk request stays None even after the real root is recorded.
        // Supplying today's new root changes the original operation.
        if empty {
            let mut recaptured = original.clone();
            recaptured.target_repository_id = Some(repository(2));
            assert!(!recaptured.matches_receipt("alice", &published));
        }
        for (field, changed) in [
            ("source_repository_id", json!(repository(3))),
            ("source_entry_id", json!(entry(3))),
            ("source_path", json!("folder/é.txt")),
            ("target_slot_id", json!("P03")),
            ("target_repository_id", json!(repository(3))),
            ("target_path", json!("other.txt")),
            ("replace", json!(true)),
            ("target_entry_id", json!(entry(3))),
        ] {
            let mut wire = json_codec::to_value(&original).unwrap();
            wire[field] = changed;
            let changed: ObjectMoveRequest = json_codec::from_value(wire).unwrap();
            assert!(!changed.matches_receipt("alice", &published), "{field}");
            assert_ne!(
                changed.digest("alice").unwrap(),
                original.digest("alice").unwrap()
            );
        }
    }
}

#[test]
fn request_digest_alone_cannot_substitute_another_fact_or_legacy_move_receipt() {
    let original = request(false);
    let published = receipt(&original);
    for kind in [
        "file.history.move",
        "directory.history.move",
        "file.namespace.rename",
    ] {
        let mut wrong = published.clone();
        wrong["operation_kind"] = kind.into();
        assert!(!original.matches_receipt("alice", &wrong), "{kind}");
    }
    for (field, changed) in [
        ("principal", json!("bob")),
        ("target_repository_id", json!(repository(3))),
        ("target_entry_id", json!("invalid-entry")),
        ("request", Value::Null),
    ] {
        let mut wrong = published.clone();
        wrong["result"]["movement"][field] = changed;
        assert!(!original.matches_receipt("alice", &wrong), "{field}");
    }
    let mut unwrapped = published.clone();
    unwrapped["result"] = published["result"]["movement"].clone();
    assert!(!original.matches_receipt("alice", &unwrapped));
    let mut wrong_digest = published;
    wrong_digest["request_digest"] = original.digest("bob").unwrap().into();
    assert!(!original.matches_receipt("alice", &wrong_digest));
}


#[test]
fn fact_retains_explicit_source_and_displaced_file_identities_for_core_replay() {
    let mut movement = request(false);
    movement.replace = true;
    movement.target_entry_id = Some(entry(2));
    let published = receipt(&movement);
    let fact: ObjectMoveFact =
        json_codec::from_value(published["result"]["movement"].clone()).unwrap();
    assert_eq!(fact.source_file_id, Some(file(1)));
    assert_eq!(fact.displaced_target_file_id, Some(file(2)));
    assert!(movement.matches_receipt("alice", &published));
    for field in ["source_file_id", "displaced_target_file_id"] {
        let mut missing = published.clone();
        missing["result"]["movement"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!movement.matches_receipt("alice", &missing), "{field}");
    }
    let directory = ObjectMoveFact {
        request: request(false),
        source_file_id: None,
        displaced_target_file_id: None,
        ..fact
    };
    let wire = json_codec::to_value(&directory).unwrap();
    assert_eq!(wire["source_file_id"], Value::Null);
    assert_eq!(wire["displaced_target_file_id"], Value::Null);
    assert_eq!(
        json_codec::from_value::<ObjectMoveFact>(wire).unwrap(),
        directory
    );
}
