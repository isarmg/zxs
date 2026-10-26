use super::*;

#[test]
fn version_wire_is_exact_and_preserves_full_width_numbers_and_stored_states() {
    for state in [
        storage_contract::ContentState::Available,
        storage_contract::ContentState::Purged,
        storage_contract::ContentState::Migrated,
    ] {
        let version = storage_contract::FileVersion {
            history_tree_id: "history".into(),
            version_id: "version".into(),
            file_id: "file".into(),
            creation_primary_slot_id: "P01".into(),
            logical_size: u64::MAX,
            version_sequence: u64::MAX,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(
                b"version wire fixture",
            )),
            checksum_chunk_bytes: 4096,
            checksum_manifest_hash: "manifest".into(),
            creation_name: "Café.txt".into(),
            storage_name: "body".into(),
            created_at: "now".into(),
            source_version_ids: vec!["parent".into()],
            created_as_full_checkpoint: true,
        };
        let snapshot = VersionSnapshot {
            version,
            content_state: state,
        };
        assert_eq!(
            json_codec::to_value(VersionResponse::from(snapshot)).unwrap(),
            json_codec::json!({
                "history_tree_id": "history",
                "version_id": "version",
                "file_id": "file",
                "creation_primary_slot_id": "P01",
                "logical_size": "18446744073709551615",
                "version_sequence": "18446744073709551615",
                "content_hash": storage_contract::ContentHash::from_digest(
                    blake3::hash(b"version wire fixture")
                ).to_string(),
                "checksum_chunk_bytes": 4096,
                "checksum_manifest_hash": "manifest",
                "creation_name": "Café.txt",
                "storage_name": "body",
                "created_at": "now",
                "source_version_ids": ["parent"],
                "created_as_full_checkpoint": true,
                "content_state": state.as_str()
            })
        );
    }
}

#[test]
fn repository_wire_preserves_binding_width_and_optional_root_marker() {
    for file_manager_root in [false, true] {
        let repository = storage_contract::Repository {
            repository_id: "repository".into(),
            name: "Café".into(),
            history_tree_id: "history".into(),
            main_branch_id: "branch".into(),
            primary_slot_id: "P01".into(),
            target_copies: 3,
            name_policy: name_policy::NAME_POLICY.into(),
            unicode_version: name_policy::UNICODE_VERSION.into(),
            smb_enabled: true,
            smb_binding_generation: u64::MAX,
            file_manager_root,
        };
        let mut expected = json_codec::json!({"repository_id":"repository","name":"Café","history_tree_id":"history","main_branch_id":"branch","primary_slot_id":"P01","target_copies":3,"name_policy":name_policy::NAME_POLICY,"unicode_version":name_policy::UNICODE_VERSION,"smb_enabled":true,"smb_binding_generation":"18446744073709551615"});
        if file_manager_root {
            expected["file_manager_root"] = true.into();
        }
        assert_eq!(
            json_codec::to_value(RepositoryResponse::from(repository)).unwrap(),
            expected
        );
    }
}
