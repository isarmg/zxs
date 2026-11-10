use super::*;
#[test]
fn first_storage_names_are_exact_distinct_direct_children() {
    assert!(
        layout::first_storage_names(&[
            "/zxs/data/disks/primary-a".into(),
            "/zxs/data/disks/资料".into(),
            "/zxs/data/disks/Archive".into(),
        ])
        .is_ok()
    );
    for roots in [
        vec![],
        vec!["/tmp/disk".into()],
        vec!["/zxs/data/disks/../state".into()],
        vec!["/zxs/data/disks//one".into()],
        vec!["/zxs/data/disks/one/".into()],
        vec!["/zxs/data/disks/one/child".into()],
        vec!["/zxs/data/disks/one".into(), "/zxs/data/disks/one".into()],
        vec!["/zxs/data/disks/CON".into()],
        vec!["/zxs/data/disks/a:b".into()],
        vec!["/zxs/data/disks/cafe\u{301}".into()],
        vec!["/zxs/data/disks/Disk".into(), "/zxs/data/disks/disk".into()],
        vec![
            "/zxs/data/disks/Straße".into(),
            "/zxs/data/disks/STRASSE".into(),
        ],
        (0..33).map(|n| format!("/zxs/data/disks/p{n}")).collect(),
    ] {
        assert!(layout::first_storage_names(&roots).is_err(), "{roots:?}");
    }
}
fn plan() -> Plan {
    let inode = json!({"device":"1","inode":"2"});
    json_codec::from_value(json!({"format":"zxs-helper-first-storage-batch",
        "context":{"host":inode,"data":inode,"state":inode,"management":inode,"discovery":inode,
        "account":{"name":"zxs","uid":1001,"gid":1001,"source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}},
        "credential":{"principal":"admin","generation":"1","fingerprint":"b".repeat(64)},"selections":[]})).unwrap()
}
#[test]
fn first_storage_confirmation_and_exact_digest_precede_system_access() {
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: "a".repeat(64),
        },
        confirmation: "initialize-system".into(),
        external_writers_stopped: true,
        plan: plan(),
    };
    assert!(
        commit(input)
            .unwrap_err()
            .to_string()
            .contains("before system access")
    );
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: "a".repeat(64),
        },
        confirmation: "initialize-first-storage-batch".into(),
        external_writers_stopped: true,
        plan: plan(),
    };
    assert!(
        commit(input)
            .unwrap_err()
            .to_string()
            .contains("exact preview digest")
    );
    assert!(request("../other").is_err());
    assert!(request(&"A".repeat(64)).is_err());
}
#[test]
fn first_storage_dto_cannot_override_uid_mode_or_start_services() {
    let mut input = json!({"service_user":"zxs","roots":["/zxs/data/disks/one"]});
    assert!(json_codec::from_value::<Input>(input.clone()).is_ok());
    input["uid"] = 0.into();
    assert!(json_codec::from_value::<Input>(input).is_err());
    let mut encoded = json_codec::to_value(plan()).unwrap();
    encoded["mode"] = "development".into();
    assert!(json_codec::from_value::<Plan>(encoded).is_err());
    let receipt = preview_response(plan(), "a".repeat(64), false);
    let encoded = json_codec::to_value(receipt).unwrap();
    assert_eq!(encoded["starts_service"], false);
    assert_eq!(encoded["complete_installation_ready"], false);
    assert_eq!(encoded["formats_or_mounts_devices"], false);
}
