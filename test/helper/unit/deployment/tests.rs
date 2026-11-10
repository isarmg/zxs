use super::*;
use crate::private_io::{BundleDestination, BundlePublication};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn input() -> Input {
    Input {
        service_user: "zxs".into(),
        service_group: "zxs".into(),
        listen: "0.0.0.0:7400".parse().unwrap(),
        smb: Some(SmbInput {
            account: "zxs-owner".into(),
            peer_uid: 0,
        }),
    }
}
#[test]
fn typed_configuration_rejects_injection_and_private_service_account_sharing() {
    for account in [
        "root",
        "",
        "a\nExecStart=/bin/sh",
        "a b",
        "%u",
        "a;cmd",
        "a$HOME",
        "../a",
        "-a",
    ] {
        let mut value = input();
        value.service_user = account.into();
        let expected = model::Settings::try_from(value.clone()).err().unwrap();
        let actual = context::Context::open(&value).err().unwrap();
        assert_eq!(actual.to_string(), expected.to_string(), "{account:?}");
    }
    let mut same = input();
    same.smb.as_mut().unwrap().account = "zxs".into();
    assert!(context::Context::open(&same).is_err());
    let mut zero = input();
    zero.listen.set_port(0);
    assert!(context::Context::open(&zero).is_err());
}
#[test]
fn generated_unit_and_samba_share_exact_socket_and_account_without_secrets() {
    let settings = model::Settings::try_from(input()).unwrap();
    let (unit, samba) = render::files(&settings);
    let samba = samba.unwrap();
    assert!(unit.content.contains("ExecStart=/usr/local/bin/zxs "));
    assert!(
        unit.content
            .contains("--smb-peer-uid 0 --smb-user zxs-owner")
    );
    assert!(unit.content.contains("--smb-socket /zxs/run/smb.sock"));
    assert!(unit.content.contains("TimeoutStopSec=infinity"));
    assert!(samba.content.contains("valid users = zxs-owner"));
    assert!(samba.content.contains("zxs:socket = /zxs/run/smb.sock"));
    assert!(samba.content.contains("map to guest = Never"));
    assert!(samba.content.contains("smb encrypt = required"));
    assert!(!unit.content.contains('@'));
    assert!(!samba.content.contains('@'));
}
#[test]
fn disabled_smb_has_no_invented_account_or_socket_binding() {
    let mut value = input();
    value.smb = None;
    let (unit, samba) = render::files(&model::Settings::try_from(value).unwrap());
    assert!(samba.is_none());
    assert!(!unit.content.contains("--smb-"));
}
#[test]
fn wrong_confirmation_refuses_before_payload_or_system_access() {
    let error = prepare(Prepare {
        input: input(),
        request_digest: "wrong".into(),
        confirmation: "activate".into(),
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("explicit prepare-inactive-configuration")
    );
}
#[test]
fn bundle_publication_is_noclobber_durable_retry_and_directory_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let parent = BundleDestination::open(temp.path()).unwrap();
    let files: &[(&str, &[u8])] = &[("manifest.json", b"manifest"), ("zxs.service", b"unit")];
    assert!(matches!(
        parent.publish("candidate", files).unwrap(),
        BundlePublication::Created
    ));
    assert!(!temp.path().join("candidate.pending").exists());
    assert_eq!(
        fs::metadata(temp.path().join("candidate"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(temp.path().join("candidate/zxs.service"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert!(matches!(
        parent.publish("candidate", files).unwrap(),
        BundlePublication::Existing
    ));
    assert!(
        parent
            .publish(
                "candidate",
                &[("manifest.json", b"different"), ("zxs.service", b"unit")]
            )
            .is_err()
    );
    assert_eq!(
        fs::read(temp.path().join("candidate/manifest.json")).unwrap(),
        b"manifest"
    );
}
#[test]
fn exact_partial_bundle_resumes_but_ambiguous_material_and_symlinks_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let pending = temp.path().join("candidate.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(pending.join("manifest.json"), b"manifest").unwrap();
    fs::set_permissions(
        pending.join("manifest.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let parent = BundleDestination::open(temp.path()).unwrap();
    let files: &[(&str, &[u8])] = &[("manifest.json", b"manifest"), ("zxs.service", b"unit")];
    assert!(matches!(
        parent.publish("candidate", files).unwrap(),
        BundlePublication::Resumed
    ));
    fs::write(temp.path().join("candidate/unexpected"), b"retain").unwrap();
    assert!(parent.publish("candidate", files).is_err());
    assert_eq!(
        fs::read(temp.path().join("candidate/unexpected")).unwrap(),
        b"retain"
    );
    symlink(temp.path().join("candidate"), temp.path().join("linked")).unwrap();
    assert!(parent.publish("linked", files).is_err());
}
#[test]
fn bundle_parent_lock_and_identity_changes_refuse_without_touching_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("parent");
    fs::create_dir(&path).unwrap();
    let held = BundleDestination::open(&path).unwrap();
    assert!(BundleDestination::open(&path).is_err());
    fs::rename(&path, temp.path().join("old-parent")).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(
        held.publish("candidate", &[("manifest.json", b"same")])
            .is_err()
    );
    assert!(fs::read_dir(path).unwrap().next().is_none());
}

#[test]
fn live_path_installation_rejects_missing_confirmation_before_any_host_access() {
    for (confirmation, external_administration_stopped) in [
        ("activate", true),
        ("install-configuration-files-only", false),
    ] {
        let error = install::commit(install::Commit {
            input: input(),
            request_digest: "unreviewed".into(),
            confirmation: confirmation.into(),
            external_administration_stopped,
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("before inspecting or writing any configuration")
        );
    }
}

#[test]
fn configuration_installation_request_is_strict_and_has_no_activation_switch() {
    let good = json_codec::json!({
        "input":{"service_user":"zxs","service_group":"zxs","listen":"0.0.0.0:7400","smb":null},
        "request_digest":"exact",
        "confirmation":"install-configuration-files-only",
        "external_administration_stopped":true
    });
    assert!(json_codec::from_value::<install::Commit>(good.clone()).is_ok());
    let mut unknown = good;
    unknown["start_service"] = true.into();
    assert!(json_codec::from_value::<install::Commit>(unknown).is_err());
}

#[test]
fn protected_single_record_can_resume_a_complete_pending_file_without_extra_material() {
    let temp = tempfile::tempdir().unwrap();
    let pending = temp.path().join("operation.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(pending.join("reset.json"), b"record").unwrap();
    fs::set_permissions(
        pending.join("reset.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let destination = BundleDestination::open(temp.path()).unwrap();
    assert_eq!(
        destination
            .read_operation("operation", "reset.json", 100)
            .unwrap()
            .unwrap(),
        b"record"
    );
    destination
        .publish("operation", &[("reset.json", b"record")])
        .unwrap();
    assert_eq!(
        destination
            .read_operation("operation", "reset.json", 100)
            .unwrap()
            .unwrap(),
        b"record"
    );
    fs::write(temp.path().join("operation/unexpected"), b"keep").unwrap();
    assert!(
        destination
            .read_operation("operation", "reset.json", 100)
            .is_err()
    );
    assert_eq!(
        fs::read(temp.path().join("operation/unexpected")).unwrap(),
        b"keep"
    );
}

#[test]
fn activation_confirmation_refuses_before_shared_deployment_admission() {
    for (confirmation, external_administration_stopped) in [
        ("start-installed-zxs-services", true),
        ("exit-helper-and-start-installed-zxs-services", false),
    ] {
        let result = activation::activate(activation::Commit {
            input: activation::Input {
                deployment: input(),
                enable_on_boot: false,
            },
            request_digest: "unreviewed".into(),
            confirmation: confirmation.into(),
            external_administration_stopped,
        });
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("unconfirmed handoff was admitted"),
        };
        assert!(error.to_string().contains("confirm helper exit"));
    }
}
