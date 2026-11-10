//! Protocol-only receipt gates. These facts are scripted, not proof that any
//! storage publication occurred; real backend scenarios live in identity_tests.
use super::*;
use storage_contract::file_relocation::{
    NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveFact, ObjectMoveRequest,
};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn id(prefix: &str, number: u8) -> String {
    format!("{prefix}_{number:032x}")
}

pub(super) fn current(operation: OperationId, replace: bool) -> MoveReceipt {
    let mut command = object_request(operation);
    command.movement.replace = replace;
    command.movement.target_entry_id =
        replace.then(|| NamespaceEntryId::parse(&id("entry", 2)).unwrap());
    command.expected_target_file_id =
        replace.then(|| HistoryFileId::parse(&id("file", 2)).unwrap());
    MoveReceipt::object(&command).unwrap()
}

fn object_request(operation: OperationId) -> crate::MoveCurrentObject {
    let binding = |number| crate::auth::SmbBinding {
        repository_id: id("repo", number),
        branch_id: id("br", number),
        generation: 7,
    };
    crate::MoveCurrentObject {
        source_binding: binding(1),
        source_slot_id: "P01".into(),
        source_disk_id: id("disk", 1),
        source_binding_generation: 7,
        target: crate::MoveObjectTarget::Repository {
            binding: binding(2),
            disk_id: id("disk", 2),
            binding_generation: 7,
        },
        movement: ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
            source_entry_id: NamespaceEntryId::parse(&id("entry", 1)).unwrap(),
            source_path: name_policy::LookupPath::parse("e\u{301}.txt").unwrap(),
            target_slot_id: "P02".into(),
            target_repository_id: Some(HistoryRepositoryId::parse(&id("repo", 2)).unwrap()),
            target_path: name_policy::PortablePath::parse("folder/arrived.txt").unwrap(),
            replace: false,
            target_entry_id: None,
        },
        expected_source_file_id: Some(HistoryFileId::parse(&id("file", 1)).unwrap()),
        expected_target_file_id: None,
        operation_id: operation,
    }
}

pub(super) fn object_result(expected: &MoveReceipt) -> OperationResult {
    let mut result = receipt(expected.operation_id);
    result.kind = OBJECT_MOVE_KIND.into();
    result.request_digest = expected.movement.digest("alice").unwrap();
    result.result = json!({
        "version_mapping": std::collections::BTreeMap::from([(id("ver", 1), id("ver", 2))]),
        "movement": ObjectMoveFact {
            request: expected.movement.clone(),
            principal: "alice".into(),
            target_repository_id: HistoryRepositoryId::parse(&id("repo", 2)).unwrap(),
            target_entry_id: NamespaceEntryId::parse(&id("entry", 3)).unwrap(),
            source_file_id: expected.source_file_id.clone(),
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
        let reply = expected.published_object_reply(result, "alice").unwrap();
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
            expected
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::StateInvalid,
            "{field}"
        );
    }
}

#[test]
fn empty_target_reply_preserves_the_absent_repository_after_root_publication() {
    let operation = OperationId::generate().unwrap();
    let mut command = object_request(operation);
    command.target = crate::MoveObjectTarget::EmptyDisk {
        disk_id: id("disk", 2),
        binding_generation: 7,
    };
    command.movement.target_repository_id = None;
    command.movement.target_path = name_policy::PortablePath::parse("arrived.txt").unwrap();
    let expected = MoveReceipt::object(&command).unwrap();
    assert_eq!(expected.movement.target_repository_id, None);
    let result = object_result(&expected);
    let reply = expected.published_object_reply(result, "alice").unwrap();
    assert_eq!(
        reply["result"]["movement"]["request"]["target_repository_id"],
        Value::Null
    );
    assert_eq!(
        reply["result"]["movement"]["target_repository_id"],
        id("repo", 2)
    );
}

#[test]
fn captured_directory_reply_requires_null_file_identity_and_the_object_kind() {
    let mut command = object_request(OperationId::generate().unwrap());
    command.expected_source_file_id = None;
    let expected = MoveReceipt::object(&command).unwrap();
    let mut result = object_result(&expected);
    result.protection_at_publish = ProtectionAtPublish::NotApplicable;
    result.result["version_mapping"] = json!({});
    let reply = expected.published_object_reply(result, "alice").unwrap();
    assert_eq!(reply["result"]["movement"]["source_file_id"], Value::Null);

    let expected = MoveReceipt::object(&command).unwrap();
    let mut missing_mapping = object_result(&expected);
    missing_mapping.protection_at_publish = ProtectionAtPublish::NotApplicable;
    missing_mapping
        .result
        .as_object_mut()
        .unwrap()
        .remove("version_mapping");
    assert_eq!(
        expected
            .published_object_reply(missing_mapping, "alice")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::StateInvalid
    );

    let expected = MoveReceipt::object(&command).unwrap();
    let mut wrong_file = object_result(&expected);
    wrong_file.result["movement"]["source_file_id"] = id("file", 1).into();
    assert_eq!(
        expected
            .published_object_reply(wrong_file, "alice")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::StateInvalid
    );
    // A newly captured object spool cannot consume a legacy file receipt.
    assert_eq!(
        MoveReceipt::object(&command)
            .unwrap()
            .published_object_reply(receipt(command.operation_id), "alice")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::StateInvalid
    );
}

#[test]
fn object_reply_distinguishes_case_rename_from_a_distinct_alias() {
    let mut command = object_request(OperationId::generate().unwrap());
    command.movement.target_repository_id = Some(command.movement.source_repository_id.clone());
    command.movement.target_entry_id = Some(command.movement.source_entry_id.clone());
    command.movement.target_slot_id = "P01".into();
    command.movement.target_path = name_policy::PortablePath::parse("É.TXT").unwrap();
    command.movement.replace = true;
    command.expected_target_file_id = command.expected_source_file_id.clone();
    command.target = crate::MoveObjectTarget::Repository {
        binding: crate::auth::SmbBinding {
            repository_id: command.source_binding.repository_id.clone(),
            branch_id: command.source_binding.branch_id.clone(),
            generation: command.source_binding.generation,
        },
        disk_id: command.source_disk_id.clone(),
        binding_generation: command.source_binding_generation,
    };
    let digest = command.movement.digest("alice").unwrap();
    let expected = MoveReceipt::object(&command).unwrap();
    assert_eq!(expected.movement, command.movement);
    assert_eq!(expected.displaced_target_file_id, None);
    let mut result = object_result(&expected);
    result.protection_at_publish = ProtectionAtPublish::NotApplicable;
    result.result["version_mapping"] = json!({});
    result.result["movement"]["target_repository_id"] = id("repo", 1).into();
    result.result["movement"]["target_entry_id"] = id("entry", 1).into();
    let reply = expected.published_object_reply(result, "alice").unwrap();
    assert_eq!(reply["request_digest"], digest);
    assert_eq!(
        reply["result"]["movement"]["displaced_target_file_id"],
        Value::Null
    );

    command.expected_target_file_id = Some(HistoryFileId::parse(&id("file", 4)).unwrap());
    assert_eq!(
        MoveReceipt::object(&command).err().unwrap().code,
        storage_contract::ErrorCode::StateInvalid
    );

    // The same entry/file can have another current occurrence. Replacing that
    // alias consumes a distinct target path, even without a new history graph.
    command.movement.target_path = name_policy::PortablePath::parse("alias.txt").unwrap();
    command.expected_target_file_id = command.expected_source_file_id.clone();
    let expected = MoveReceipt::object(&command).unwrap();
    assert_eq!(
        expected.displaced_target_file_id,
        command.expected_source_file_id
    );
    let mut result = object_result(&expected);
    result.protection_at_publish = ProtectionAtPublish::NotApplicable;
    result.result["version_mapping"] = json!({});
    result.result["movement"]["target_repository_id"] = id("repo", 1).into();
    result.result["movement"]["target_entry_id"] = id("entry", 1).into();
    result.result["movement"]["displaced_target_file_id"] = json!(command.expected_source_file_id);
    let exact_result = result.clone();
    let reply = expected.published_object_reply(result, "alice").unwrap();
    assert_eq!(
        reply["request_digest"],
        command.movement.digest("alice").unwrap()
    );
    assert_eq!(
        reply["result"]["movement"]["displaced_target_file_id"],
        id("file", 1)
    );
    for wrong in [Value::Null, json!(id("file", 4))] {
        let mut result = exact_result.clone();
        result.result["movement"]["displaced_target_file_id"] = wrong;
        assert_eq!(
            MoveReceipt::object(&command)
                .unwrap()
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::StateInvalid
        );
    }
}
