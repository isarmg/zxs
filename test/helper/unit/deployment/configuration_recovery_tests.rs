use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn input() -> Input {
    Input {
        service_user: "zxs".into(),
        service_group: "zxs".into(),
        listen: "0.0.0.0:7400".parse().unwrap(),
        smb: None,
    }
}
fn record(digest: &str) -> Vec<u8> {
    json_codec::to_vec_pretty(&Record {
        format: Format::OriginalConfiguration,
        revision: 2,
        request_digest: digest.into(),
        input: input(),
    })
    .unwrap()
}
fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn original_digest_is_canonical_before_host_access() {
    assert!(require_digest(&"a".repeat(64)).is_ok());
    for bad in [
        "".to_owned(),
        "A".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        "../host".into(),
    ] {
        assert!(require_digest(&bad).is_err());
        let error = match preview(Original {
            request_digest: bad,
        }) {
            Ok(_) => panic!("unexpected preview"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("before host access"));
    }
}
#[test]
fn continuation_needs_both_confirmations_before_host_access() {
    for (confirmation, stopped) in [
        ("install-configuration-files-only", true),
        ("resume-original-configuration-files-only", false),
    ] {
        let error = commit(Commit {
            request_digest: "not-even-a-digest".into(),
            confirmation: confirmation.into(),
            external_administration_stopped: stopped,
        })
        .unwrap_err();
        assert!(error.to_string().contains("before any host access"));
    }
}
#[test]
fn recovery_wire_has_no_replacement_input_destination_or_activation() {
    let original = json_codec::json!({"request_digest":"a".repeat(64)});
    assert!(json_codec::from_value::<Original>(original.clone()).is_ok());
    for key in ["input", "destination", "service_user", "start_service"] {
        let mut changed = original.clone();
        changed[key] = true.into();
        assert!(json_codec::from_value::<Original>(changed).is_err());
    }
    let mut commit = json_codec::json!({"request_digest":"a".repeat(64),"confirmation":"resume-original-configuration-files-only","external_administration_stopped":true});
    assert!(json_codec::from_value::<Commit>(commit.clone()).is_ok());
    commit["input"] = json_codec::to_value(input()).unwrap();
    assert!(json_codec::from_value::<Commit>(commit).is_err());
}
#[test]
fn exact_original_is_read_from_complete_pending_without_publishing_it() {
    let temp = tempfile::tempdir().unwrap();
    let digest = "b".repeat(64);
    let pending = temp
        .path()
        .join(format!("{}.pending", request_name(&digest)));
    private_dir(&pending);
    let bytes = record(&digest);
    private_file(&pending.join("request.json"), &bytes);
    let host = BundleDestination::open(temp.path()).unwrap();
    let loaded = read_original(&host, &digest).unwrap();
    assert_eq!(loaded.service_user, "zxs");
    assert_eq!(loaded.listen.to_string(), "0.0.0.0:7400");
    assert!(pending.is_dir());
    assert!(!temp.path().join(request_name(&digest)).exists());
    assert_eq!(fs::read(pending.join("request.json")).unwrap(), bytes);
}
#[test]
fn unsupported_mismatched_and_missing_original_records_are_not_reconstructed() {
    let digest = "c".repeat(64);
    for kind in ["revision", "digest", "extra", "empty", "link"] {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join(request_name(&digest));
        private_dir(&folder);
        let mut value: json_codec::Value = json_codec::from_slice(&record(&digest)).unwrap();
        match kind {
            "revision" => value["revision"] = 1.into(),
            "digest" => value["request_digest"] = "d".repeat(64).into(),
            "extra" => value["input"]["start_service"] = true.into(),
            "empty" | "link" => {}
            _ => unreachable!(),
        }
        let bytes = json_codec::to_vec(&value).unwrap();
        if kind == "link" {
            private_file(&temp.path().join("outside"), &bytes);
            symlink(temp.path().join("outside"), folder.join("request.json")).unwrap();
        } else if kind != "empty" {
            private_file(&folder.join("request.json"), &bytes);
        }
        let host = BundleDestination::open(temp.path()).unwrap();
        assert!(read_original(&host, &digest).is_err(), "{kind}");
        assert!(folder.is_dir());
        if kind != "empty" && kind != "link" {
            assert_eq!(fs::read(folder.join("request.json")).unwrap(), bytes);
        }
    }
}
#[test]
fn stage_order_and_completion_missing_files_cannot_be_repaired_by_retry() {
    use BundleState::{Absent, Pending, Published};
    for (request, intent, completion, present) in [
        (Absent, Published, Absent, vec![true]),
        (Pending, Pending, Absent, vec![false]),
        (Published, Absent, Pending, vec![true]),
        (Published, Pending, Published, vec![true]),
        (Published, Published, Pending, vec![true, false]),
        (Published, Published, Published, vec![false]),
    ] {
        assert!(ordered_state(request, intent, completion, &present).is_err());
    }
    assert!(matches!(
        ordered_state(Absent, Absent, Absent, &[false]).unwrap(),
        RecoveryState::NotRecorded
    ));
    assert!(matches!(
        ordered_state(Pending, Absent, Absent, &[false]).unwrap(),
        RecoveryState::OriginalRequestRecorded
    ));
    assert!(matches!(
        ordered_state(Published, Pending, Absent, &[false]).unwrap(),
        RecoveryState::IntentPreparationPending
    ));
    assert!(matches!(
        ordered_state(Published, Published, Absent, &[true, false]).unwrap(),
        RecoveryState::ConfigurationFilesPending
    ));
    assert!(matches!(
        ordered_state(Published, Published, Pending, &[true, true]).unwrap(),
        RecoveryState::CompletionPublicationPending
    ));
    assert!(matches!(
        ordered_state(Published, Published, Published, &[true]).unwrap(),
        RecoveryState::CompletedConfigurationFiles
    ));
}

#[test]
fn oversized_and_conflicting_original_records_are_preserved() {
    let digest = "e".repeat(64);
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join(request_name(&digest));
    private_dir(&target);
    let oversized = vec![b' '; MAX_RECORD_BYTES + 1];
    private_file(&target.join("request.json"), &oversized);
    let host = BundleDestination::open(temporary.path()).unwrap();
    let error = match read_original(&host, &digest) {
        Ok(_) => panic!("oversized original record was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("size"));
    assert_eq!(fs::read(target.join("request.json")).unwrap(), oversized);

    let bytes = record(&digest);
    private_file(&target.join("request.json"), &bytes);
    let pending = temporary
        .path()
        .join(format!("{}.pending", request_name(&digest)));
    private_dir(&pending);
    private_file(&pending.join("request.json"), &bytes);
    assert!(read_original(&host, &digest).is_err());
    assert_eq!(fs::read(target.join("request.json")).unwrap(), bytes);
    assert_eq!(fs::read(pending.join("request.json")).unwrap(), bytes);
}
