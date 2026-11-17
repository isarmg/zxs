use super::*;

#[test]
fn repair_confirmation_precedes_root_payload_or_system_path_access() {
    for (confirmation, stopped) in [
        ("install-program-only", true),
        ("repair-same-payload-program-only", false),
    ] {
        let error = commit(Commit {
            request_digest: "invalid".into(),
            confirmation: confirmation.into(),
            external_administration_stopped: stopped,
        })
        .expect_err("confirmation must fail before host access");
        assert!(error.to_string().contains("before reading or writing"));
    }
}

#[test]
fn original_digest_and_wire_never_accept_an_uploaded_replacement_or_path() {
    for value in ["", "abc", &"A".repeat(64), &"a/".repeat(32)] {
        assert!(digest(value).is_err());
    }
    assert!(digest(&"a".repeat(64)).is_ok());
    let body = r#"{"request_digest":"a","confirmation":"repair-same-payload-program-only","external_administration_stopped":true,"path":"/tmp/other"}"#;
    assert!(json_codec::from_str::<Commit>(body).is_err());
    assert!(
        json_codec::from_str::<Original>(r#"{"request_digest":"a","payload":"upload"}"#).is_err()
    );
}

fn fixture() -> (tempfile::TempDir, Context, Plan, String) {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let host_path = root.path().join("private");
    let bin = root.path().join("bin");
    fs::create_dir(&host_path).unwrap();
    fs::create_dir(&bin).unwrap();
    for d in [&host_path, &bin] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let file = bin.join("zxs");
    fs::write(&file, b"original damaged binary").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
    let host = BundleDestination::open(&host_path).unwrap();
    let program = ProgramDestination::open(&file).unwrap();
    // Exercise actual held bundle and exchange logic. No host binary, service
    // or external path is touched; only the release-byte fixture is synthetic.
    let payload = payload::Payload {
        bytes: b"original embedded payload",
        version: env!("CARGO_PKG_VERSION"),
        commit: "fixture",
        tree: "fixture",
        target: "x86_64-unknown-linux-gnu",
        sha256: "fixture",
    };
    let baseline = baseline::expected(&host, &program, payload).unwrap();
    host.publish(baseline::NAME, &[("receipt.json", &baseline)])
        .unwrap();
    let context = Context {
        host,
        program,
        payload,
        baseline,
    };
    let plan = context.fresh().unwrap();
    let digest = plan_digest(&plan).unwrap();
    (root, context, plan, digest)
}

#[test]
fn completed_same_payload_repair_requires_independent_original_without_rebuilding_it() {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    let (root, context, plan, digest) = fixture();
    let result = publish(&context, &plan, &digest).unwrap();
    assert_eq!(
        result["receipt"]["original_inode_and_independent_backup_retained"],
        true
    );
    let file = root.path().join("bin/zxs");
    let installed = fs::metadata(&file).unwrap().ino();
    let host = root.path().join("private");
    let copy = host.join(backup_name(&digest));
    let retained = host.join("retained-elsewhere");
    let record = host.join(completion_name(&digest)).join("receipt.json");
    let original_receipt = fs::read(&record).unwrap();
    fs::rename(&copy, &retained).unwrap();
    assert!(publish(&context, &plan, &digest).is_err());
    assert!(!copy.exists());
    assert_eq!(fs::read(&record).unwrap(), original_receipt);
    assert_eq!(fs::metadata(&file).unwrap().ino(), installed);
    assert_eq!(
        fs::read(retained.join("original.bin")).unwrap(),
        b"original damaged binary"
    );
    fs::rename(&retained, &copy).unwrap();
    assert_eq!(publish(&context, &plan, &digest).unwrap(), result);
}

#[test]
fn exchanged_program_without_completion_cannot_refill_a_missing_or_pending_backup() {
    use std::fs;
    let (root, context, plan, digest) = fixture();
    let host = root.path().join("private");
    context
        .host
        .publish(
            &intent_name(&digest),
            &[("plan.json", &json_codec::to_vec_pretty(&plan).unwrap())],
        )
        .unwrap();
    let backup = backup_name(&digest);
    context
        .host
        .publish(&backup, &[("original.bin", b"original damaged binary")])
        .unwrap();
    context
        .program
        .repair_install(&digest, &plan.original, context.payload.bytes, || Ok(()))
        .unwrap();
    let pending = host.join(format!("{backup}.pending"));
    fs::rename(host.join(&backup), &pending).unwrap();
    assert!(publish(&context, &plan, &digest).is_err());
    assert!(!host.join(&backup).exists());
    assert!(pending.is_dir());
    assert!(!host.join(completion_name(&digest)).exists());
    fs::rename(&pending, host.join("original-set-aside")).unwrap();
    assert!(publish(&context, &plan, &digest).is_err());
    assert!(!host.join(&backup).exists());
    assert!(!host.join(completion_name(&digest)).exists());
}
