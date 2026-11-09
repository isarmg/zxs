use super::*;
use json_codec::json;

fn request() -> wire::StartRequest {
    let selection = |path: &str| {
        json!({
            "selection_id":"directory_observed", "discovery_root":"/fresh",
            "discovery_identity":{"device":"1","inode":"1","ctime_seconds":"0","ctime_nanoseconds":0},
            "mount_path":"/fresh", "root_path":path,
            "mount_identity":{"mount_id":"1","device_major":1,"device_minor":1,"root_inode":"1","filesystem":"ext4","filesystem_root":"/"},
            "root_identity":{"device":"1","inode":"2","ctime_seconds":"0","ctime_nanoseconds":0}
        })
    };
    json_codec::from_value(json!({
        "operation_id":"op_11111111111111111111111111111111",
        "source_slot_id":"P01", "expected_sequence":"17",
        "confirmation":"prepare_independent_complete_copies",
        "targets":[
            {"selection":selection("/fresh/a"),"disk_id":"disk_a","slot_id":"B01","upstream_slot_id":"P01"},
            {"selection":selection("/fresh/b"),"disk_id":"disk_b","slot_id":"B02","upstream_slot_id":"B01"}
        ]
    })).unwrap()
}

#[test]
fn adapter_binds_current_principal_and_exact_target_order() {
    let request = request();
    let core = core_request(&request, "current-administrator");
    assert_eq!(core.preparation.principal, "current-administrator");
    assert_eq!(core.preparation.operation_id, request.operation_id);
    assert_eq!(
        core.preparation.target_paths,
        vec![PathBuf::from("/fresh/a"), PathBuf::from("/fresh/b")]
    );
    assert_eq!(core.targets[1].upstream_slot_id, "B01");
    assert_eq!(core.preparation.expected_sequence, 17);
}

#[test]
fn readback_preserves_targets_but_does_not_disclose_core_principal() {
    let request = request();
    let original = original_wire(core_request(&request, "current-administrator")).unwrap();
    assert_eq!(original.operation_id, request.operation_id);
    assert_eq!(original.targets[0].root_path, "/fresh/a");
    assert_eq!(original.targets[1].root_path, "/fresh/b");
    let encoded = json_codec::to_value(original).unwrap();
    assert!(encoded.get("principal").is_none());
}

#[test]
fn incomplete_retained_target_order_cannot_be_zipped_into_success() {
    let mut core = core_request(&request(), "current-administrator");
    core.targets.pop();
    assert_eq!(
        original_wire(core).unwrap_err().code,
        ErrorCode::StateInvalid
    );
}

#[test]
fn unsupported_non_utf8_original_path_is_not_lossily_replaced() {
    use std::os::unix::ffi::OsStringExt;
    let mut core = core_request(&request(), "current-administrator");
    core.preparation.target_paths[0] = std::ffi::OsString::from_vec(b"/fresh/\xff".to_vec()).into();
    assert!(original_wire(core).is_err());
}
