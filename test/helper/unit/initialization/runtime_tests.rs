use super::*;

#[test]
fn runtime_confirmation_refuses_before_root_account_or_filesystem_access() {
    for (confirmation, stopped) in [
        ("prepare-private-layout-only", true),
        ("prepare-empty-runtime-only", false),
    ] {
        let error = prepare(Prepare {
            input: Input {
                service_user: "not an account".into(),
            },
            request_digest: "unused".into(),
            confirmation: confirmation.into(),
            external_administration_stopped: stopped,
        })
        .err()
        .expect("confirmation must precede local account access");
        assert!(error.to_string().contains("prepare-empty-runtime-only"));
    }
}

#[test]
fn runtime_wire_requires_explicit_external_ack_and_accepts_no_arbitrary_path() {
    let accepted = r#"{"input":{"service_user":"zxs"},"request_digest":"abc","confirmation":"prepare-empty-runtime-only","external_administration_stopped":true}"#;
    assert!(json_codec::from_str::<Prepare>(accepted).is_ok());
    let path = accepted.replace(
        "\"service_user\":\"zxs\"",
        "\"service_user\":\"zxs\",\"path\":\"/tmp/elsewhere\"",
    );
    assert!(json_codec::from_str::<Prepare>(&path).is_err());
    let missing = accepted.replace(",\"external_administration_stopped\":true", "");
    assert!(json_codec::from_str::<Prepare>(&missing).is_err());
}

#[test]
fn completion_is_directory_identity_evidence_not_installation_readiness() {
    let receipt = Receipt {
        format: "zxs-helper-runtime-directories-prepared",
        revision: 1,
        request_digest: "a".repeat(64),
        directories: vec![directories::Observation::Prepared {
            destination: "/zxs/run",
            device: "8".into(),
            inode: "42".into(),
        }],
        service_started: false,
        system_initialized: false,
        complete_installation_ready: false,
    };
    let first = json_codec::to_vec_pretty(&receipt).unwrap();
    let mut changed = receipt;
    changed.directories = vec![directories::Observation::Prepared {
        destination: "/zxs/run",
        device: "8".into(),
        inode: "43".into(),
    }];
    assert_ne!(first, json_codec::to_vec_pretty(&changed).unwrap());
    let value: json_codec::Value = json_codec::from_slice(&first).unwrap();
    assert_eq!(value["complete_installation_ready"], false);
    assert_eq!(value["system_initialized"], false);
    assert_eq!(value["service_started"], false);
}
