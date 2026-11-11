use super::*;

fn movement(target_slot: &str, occupied: bool) -> ObjectMoveRequest {
    ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(
            "repo_00000000000000000000000000000001",
        )
        .unwrap(),
        source_entry_id: NamespaceEntryId::parse("entry_00000000000000000000000000000001")
            .unwrap(),
        source_path: LookupPath::parse("folder/e\u{301}.txt").unwrap(),
        target_slot_id: target_slot.into(),
        target_repository_id: occupied.then(|| {
            HistoryRepositoryId::parse("repo_00000000000000000000000000000002").unwrap()
        }),
        target_path: PortablePath::parse("folder/é.txt").unwrap(),
        replace: occupied,
        target_entry_id: occupied.then(|| {
            NamespaceEntryId::parse("entry_00000000000000000000000000000002").unwrap()
        }),
    }
}

#[test]
fn typed_move_command_preserves_wire_bytes_digest_and_optional_target_facts() {
    for occupied in [false, true] {
        for target_slot in ["P02".to_owned(), "disk_02-X".into(), "A".repeat(64)] {
            let original = movement(&target_slot, occupied);
            let wire = json_codec::to_vec(&original).unwrap();
            let canonical = storage_contract::canonical_json(&original).unwrap();
            let digest = original.digest("alice").unwrap();
            let operation_id = OperationId::from_random([7; 16]);
            let command = MoveDiskObject::new(
                SlotId::parse("P01").unwrap(),
                original.clone(),
                operation_id,
            )
            .unwrap();
            assert_eq!(command.source_slot_id.as_str(), "P01");
            assert_eq!(command.target_slot_id.as_str(), target_slot);
            assert_eq!(command.operation_id, operation_id);
            let restored = command.into_request();
            assert_eq!(restored, original);
            assert_eq!(json_codec::to_vec(&restored).unwrap(), wire);
            assert_eq!(storage_contract::canonical_json(&restored).unwrap(), canonical);
            assert_eq!(restored.digest("alice").unwrap(), digest);
        }
    }
}

#[test]
fn target_slot_is_admitted_after_raw_request_decoding() {
    for target_slot in ["", "0disk", "_disk", "disk/name", "é", &"A".repeat(65)] {
        let wire = json_codec::to_vec(&movement(target_slot, false)).unwrap();
        let decoded = json_codec::from_slice::<ObjectMoveRequest>(&wire).unwrap();
        let error = MoveDiskObject::new(
            SlotId::parse("P01").unwrap(),
            decoded,
            OperationId::from_random([7; 16]),
        )
        .err()
        .unwrap();
        assert_eq!(error, name_policy::Error::InvalidSlot);
    }
}
