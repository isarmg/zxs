//! Protocol-only receipt gates. These facts are scripted, not proof that any
//! storage publication occurred; real backend scenarios live in identity_tests.
use super::*;
use storage_contract::file_relocation::{NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveFact};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn id(prefix: &str, number: u8) -> String {
    format!("{prefix}_{number:032x}")
}

fn request(operation_id: OperationId) -> crate::MoveCurrentFile {
    let binding = |number| crate::auth::SmbBinding {
        repository_id: id("repo", number),
        branch_id: id("br", number),
        generation: 7,
    };
    crate::MoveCurrentFile {
        source_binding: binding(1),
        target_binding: binding(2),
        source_path: name_policy::LookupPath::parse("e\u{301}.txt").unwrap(),
        target_name: crate::MoveTargetName::parse("folder/arrived.txt").unwrap(),
        expected_source_entry_id: id("entry", 1),
        expected_source_file_id: HistoryFileId::parse(&id("file", 1)).unwrap(),
        operation_id,
    }
}

fn current(operation: OperationId, replace: bool) -> MoveReceipt {
    let replacement = replace.then(|| ReplacementIntent {
        entry_id: id("entry", 2),
        file_id: id("file", 2),
    });
    MoveReceipt::current(&request(operation), "P02", replacement.as_ref()).unwrap()
}

fn object_result(expected: &MoveReceipt) -> OperationResult {
    let mut result = receipt(expected.operation_id);
    result.kind = OBJECT_MOVE_KIND.into();
    result.request_digest = expected.movement.digest("alice").unwrap();
    result.result = json!({
        "movement": ObjectMoveFact {
            request: expected.movement.clone(),
            principal: "alice".into(),
            target_repository_id: HistoryRepositoryId::parse(&id("repo", 2)).unwrap(),
            target_entry_id: NamespaceEntryId::parse(&id("entry", 3)).unwrap(),
            source_file_id: Some(expected.source_file_id.clone()),
            displaced_target_file_id: expected.displaced_target_file_id.clone(),
        },
    });
    result
}

#[test]
fn bound_object_reply_accepts_original_lookup_and_exact_captured_files() {
    for replace in [false, true] {
        let operation = OperationId::generate().unwrap();
        let expected = current(operation, replace);
        assert_eq!(expected.movement.source_path.as_str(), "e\u{301}.txt");
        assert_eq!(expected.movement.target_path.as_str(), "folder/arrived.txt");
        let result = object_result(&expected);
        let reply = expected.published_reply(result, "alice").unwrap();
        let fact = &reply["result"]["movement"];
        assert_eq!(reply["operation_kind"], OBJECT_MOVE_KIND);
        assert_eq!(fact["source_file_id"], id("file", 1));
        assert_eq!(fact["request"]["replace"], replace);
        assert_eq!(
            fact["displaced_target_file_id"],
            if replace {
                json!(id("file", 2))
            } else {
                Value::Null
            }
        );
    }
}

#[test]
fn bound_object_reply_rejects_other_file_facts_despite_the_same_request_digest() {
    for field in ["source_file_id", "displaced_target_file_id"] {
        let expected = current(OperationId::generate().unwrap(), true);
        let mut result = object_result(&expected);
        result.result["movement"][field] = id("file", 4).into();
        assert_eq!(
            expected.published_reply(result, "alice").unwrap_err().code,
            storage_contract::ErrorCode::StateInvalid,
            "{field}"
        );
    }
}

#[test]
fn empty_target_reply_preserves_the_absent_repository_after_root_publication() {
    let operation = OperationId::generate().unwrap();
    let source = request(operation);
    let command = crate::MoveCurrentFileToEmptyDisk {
        source_binding: source.source_binding,
        target_slot_id: "P02".into(),
        target_disk_id: id("disk", 2),
        target_binding_generation: 7,
        source_path: source.source_path,
        target_name: crate::MoveTargetName::parse("arrived.txt").unwrap(),
        expected_source_entry_id: source.expected_source_entry_id,
        expected_source_file_id: source.expected_source_file_id,
        operation_id: operation,
    };
    let expected = MoveReceipt::empty(&command).unwrap();
    assert_eq!(expected.movement.target_repository_id, None);
    let result = object_result(&expected);
    let reply = expected.published_reply(result, "alice").unwrap();
    assert_eq!(
        reply["result"]["movement"]["request"]["target_repository_id"],
        Value::Null
    );
    assert_eq!(
        reply["result"]["movement"]["target_repository_id"],
        id("repo", 2)
    );
}

