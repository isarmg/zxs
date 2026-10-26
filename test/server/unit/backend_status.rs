use super::*;
#[test]
fn capabilities_preserve_the_complete_current_wire_and_typed_limit() {
    let expected = json_codec::json!({"name_policy":name_policy::NAME_POLICY,"path_policy":"PortablePath","unicode_data_version":name_policy::UNICODE_VERSION,"supported_save_modes":["full"],"full_only":true,"full_branches":false,"branch_creation":"record-until-write","branch_content":"inherit-parent-until-first-write","file_branches":true,"branch_scope":"file","directory_branches":true,"directory_branch_creation":"inherit-directory-tree","directory_subtrees":true,"namespace_batch":true,"smb_default_branch":"selected-root-directory","file_manager":true,"historical_save_creates_branch":true,"merge_behavior":"explicit-file-node-adoption","file_branch_merge":true,"directory_branch_merge":false,"directory_merge_status":"design-pending","file_node_tags":true,"backup_configuration":true,"range_upload":false,"complete_upload":true,"max_upload_bytes":"42","authentication":"session-bearer-or-cookie","platforms":{"web":true,"smb":true},"operations":["repositories","tree","workspace","commit","history","graph","branch","merge-link","file-branch-merge","file-node-tags","backup-configuration","purge-plan","split-plan","version-range-read","file-manager","file-branches","file-branch-rename","file-path-nodes","directory-branches","directory-subtrees","namespace-batch","default-branch","file-notes","system-status"]});
    assert_eq!(
        json_codec::to_value(CapabilitiesResponse::from(Capabilities::for_engine(42))).unwrap(),
        expected
    );
    let mut limited = expected;
    limited["max_upload_bytes"] = "17".into();
    assert_eq!(
        json_codec::to_value(CapabilitiesResponse::from(
            Capabilities::for_engine(42).with_upload_limit(17)
        ))
        .unwrap(),
        limited
    );
    assert_eq!(
        json_codec::to_value(CapabilitiesResponse::from(
            Capabilities::unavailable().with_upload_limit(17)
        ))
        .unwrap(),
        json_codec::json!({"features":[],"max_upload_bytes":"0"})
    );
}
#[test]
fn storage_status_preserves_embedded_omission_and_all_managed_states() {
    assert_eq!(
        json_codec::to_value(StorageStatusResponse::from(StorageStatus::EmbeddedReady)).unwrap(),
        json_codec::json!({"state":"ready","retryable":false})
    );
    for state in [
        StorageState::Ready,
        StorageState::ReadOnly,
        StorageState::Unavailable,
        StorageState::Unconfigured,
        StorageState::RecoveryRequired,
    ] {
        for recovering in [false, true] {
            let status = StorageStatus::Managed { state, recovering };
            let wire = json_codec::json!({"state":state.as_str(),"retryable":state!=StorageState::Ready,"recovering":recovering});
            assert_eq!(
                json_codec::to_value(StorageStatusResponse::from(status)).unwrap(),
                wire
            );
            let capabilities = Capabilities::unavailable().with_storage(status);
            assert_eq!(
                json_codec::to_value(CapabilitiesResponse::from(capabilities)).unwrap(),
                json_codec::json!({"features":[],"max_upload_bytes":"0","storage":wire})
            );
        }
    }
}

#[test]
fn capabilities_keep_full_width_upload_limits_at_the_wire_boundary() {
    for limit in [0, (1u64 << 53) + 1, u64::MAX] {
        let capabilities = Capabilities::for_engine(limit);
        assert_eq!(capabilities.max_upload_bytes(), limit);
        let wire = json_codec::to_value(CapabilitiesResponse::from(capabilities)).unwrap();
        assert_eq!(wire["max_upload_bytes"], limit.to_string());
        assert!(wire["max_upload_bytes"].is_string());
    }
}

#[test]
fn readiness_wire_preserves_managed_omission_and_engine_counts() {
    assert_eq!(
        json_codec::to_value(ReadinessResponse::from(Readiness::Managed)).unwrap(),
        json_codec::json!({"ready":true})
    );
    for count in [1, 2, 3, 4] {
        let ready = Readiness::Engine(EngineReadiness {
            mode: storage_contract::Mode::Development,
            sequence: u64::MAX,
            storage_disk_count: count,
            metadata_member_count: count,
        });
        assert_eq!(
            json_codec::to_value(ReadinessResponse::from(ready)).unwrap(),
            json_codec::json!({"ready":true,"mode":"development","sequence":"18446744073709551615","storage_disk_count":count,"metadata_member_count":count,"metadata_redundancy":if count==1 {"none"} else {"multiple-copies"}})
        );
    }
}
