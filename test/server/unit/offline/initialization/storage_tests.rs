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
// Codec-only inputs exercise request binding, never host/disk admission.
fn named_selection(name: &str) -> NamedStorageSelection {
    use storage_engine::storage_directories::{
        CandidateSelection, DirectoryIdentity, MountIdentity,
    };
    let identity = DirectoryIdentity {
        device: 1,
        inode: 2,
        ctime_seconds: "0".into(),
        ctime_nanoseconds: 0,
    };
    NamedStorageSelection {
        name: name_policy::Name::parse(name).unwrap(),
        selection: CandidateSelection {
            selection_id: "c".repeat(64),
            discovery_root: "/zxs/data/disks".into(),
            discovery_identity: identity.clone(),
            mount_path: "/".into(),
            root_path: "/zxs/data/disks/not-the-display-name".into(),
            mount_identity: MountIdentity {
                mount_id: 1,
                device_major: 1,
                device_minor: 0,
                root_inode: 1,
                filesystem: "ext4".into(),
                filesystem_root: "/".into(),
            },
            root_identity: identity,
        },
    }
}
fn plan() -> Plan {
    let inode = json!({"device":"1","inode":"2"});
    json_codec::from_value(json!({"format":"zxs-helper-first-storage-batch",
        "context":{"host":inode,"data":inode,"state":inode,"management":inode,"discovery":inode,
        "account":{"name":"zxs","uid":1001,"gid":1001,"source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}},
        "credential":{"principal":"admin","generation":"1","fingerprint":"b".repeat(64)},
        "selections":[named_selection("Archive")]})).unwrap()
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
    let original = plan();
    let original_digest = digest(&original).unwrap();
    let mut renamed = original.clone();
    renamed.selections[0].name = name_policy::Name::parse("Different").unwrap();
    assert_eq!(original.roots(), renamed.roots());
    assert_ne!(digest(&renamed).unwrap(), original_digest);
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: original_digest,
        },
        confirmation: "initialize-first-storage-batch".into(),
        external_writers_stopped: true,
        plan: renamed,
    };
    assert!(
        commit(input)
            .unwrap_err()
            .to_string()
            .contains("exact preview digest")
    );
    let retained: Plan = json_codec::from_slice(&json_codec::to_vec(&original).unwrap()).unwrap();
    assert_eq!(retained.activation().selections, original.selections);
    assert_eq!(retained.requested_disks()[0].name.as_str(), "Archive");
    assert_eq!(retained.requested_disks()[0].root_path, original.roots()[0]);
    assert!(request("../other").is_err());
    assert!(request(&"A".repeat(64)).is_err());
}
#[test]
fn first_storage_dto_cannot_override_uid_mode_or_start_services() {
    let mut input = json!({"service_user":"zxs", "disks":[
        {"name":"Archive", "root_path":"/zxs/data/disks/one"}
    ]});
    let parsed: Input = json_codec::from_value(input.clone()).unwrap();
    verify_configuration(&parsed.disks).unwrap();
    assert_eq!(parsed.disks[0].name.as_str(), "Archive");
    assert_eq!(parsed.roots(), vec!["/zxs/data/disks/one".to_owned()]);
    assert!(json_codec::from_value::<directories::Input>(input.clone()).is_ok());
    for invalid in [
        json!({"service_user":"zxs", "roots":["/zxs/data/disks/one"]}),
        json!({"service_user":"zxs", "disks":[{"root_path":"/zxs/data/disks/one"}]}),
        json!({"service_user":"zxs", "disks":[{
            "name":"CON", "root_path":"/zxs/data/disks/one"
        }]}),
    ] {
        assert!(json_codec::from_value::<Input>(invalid.clone()).is_err());
        assert!(json_codec::from_value::<directories::Input>(invalid).is_err());
    }
    let duplicate: Input = json_codec::from_value(json!({"service_user":"zxs", "disks":[
        {"name":"Archive", "root_path":"/zxs/data/disks/one"},
        {"name":"archive", "root_path":"/zxs/data/disks/two"}
    ]})).unwrap();
    let error = verify_configuration(&duplicate.disks).unwrap_err();
    assert_eq!(
        error.downcast_ref::<storage_engine::Error>().unwrap().code(),
        storage_engine::ErrorCode::NameConflict,
    );
    input["uid"] = 0.into();
    assert!(json_codec::from_value::<Input>(input).is_err());
    let mut unnamed_plan = json_codec::to_value(plan()).unwrap();
    unnamed_plan["selections"] = json!([named_selection("Archive").selection]);
    assert!(json_codec::from_value::<Plan>(unnamed_plan).is_err());
    let mut encoded = json_codec::to_value(plan()).unwrap();
    encoded["mode"] = "development".into();
    assert!(json_codec::from_value::<Plan>(encoded).is_err());
    let receipt = preview_response(plan(), "a".repeat(64), false);
    let encoded = json_codec::to_value(receipt).unwrap();
    assert_eq!(encoded["starts_service"], false);
    assert_eq!(encoded["complete_installation_ready"], false);
    assert_eq!(encoded["formats_or_mounts_devices"], false);
}

#[test]
fn first_storage_execution_budget_precedes_host_access_and_retained_intent() {
    let original = plan();
    let request = execution_request(&original).unwrap();
    assert!(request.context == original.context);
    let Action::Execute { plan: exact } = request.action else {
        panic!("first-storage execution must retain the original action");
    };
    assert_eq!(digest(&exact).unwrap(), digest(&original).unwrap());

    // Codec-only boundary case, not an admitted filesystem or fake worker.
    // The stored Plan fits exactly, while its repeated-context envelope cannot.
    let mut large = original;
    let length = json_codec::to_vec(&large).unwrap().len();
    large.selections[0].selection.mount_identity.filesystem
        .push_str(&"x".repeat(MAX_RECORD_BYTES - length));
    assert_eq!(json_codec::to_vec(&large).unwrap().len(), MAX_RECORD_BYTES);
    assert!(execution_request(&large).is_err());
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: digest(&large).unwrap(),
        },
        confirmation: "initialize-first-storage-batch".into(),
        external_writers_stopped: true,
        plan: large,
    };
    assert!(commit(input).unwrap_err().to_string().contains("IPC budget"));
}


#[test]
fn first_storage_rejects_an_executable_plan_whose_original_inspection_cannot_cross_ipc() {
    let mut plan = plan();
    let execute = |p: &Plan| WorkerRequest {
        context: p.context.clone(), action: Action::Execute { plan: p.clone() },
    };
    let used = json_codec::to_vec(&execute(&plan)).unwrap().len();
    plan.selections[0].selection.mount_identity.filesystem
        .push_str(&"x".repeat(MAX_RECORD_BYTES - used));
    assert_eq!(json_codec::to_vec(&execute(&plan)).unwrap().len(), MAX_RECORD_BYTES);
    let inspect = WorkerRequest {
        context: plan.context.clone(), action: Action::InspectOriginal { plan: plan.clone() },
    };
    assert!(json_codec::to_vec(&inspect).unwrap().len() > MAX_RECORD_BYTES);
    let error = execution_request(&plan).err().expect("recovery inspection is over budget");
    assert!(error.to_string().contains("original inspection"));
    let input = Commit {
        input: Original { service_user: "zxs".into(), request_digest: digest(&plan).unwrap() },
        confirmation: "initialize-first-storage-batch".into(), external_writers_stopped: true, plan,
    };
    assert!(commit(input).unwrap_err().to_string().contains("original inspection"));
}
