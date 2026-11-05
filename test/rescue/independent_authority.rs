use json_codec::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use storage_engine::{
    OperationId,
    disk_authority::{Digest, DiskSelection, Identity, Origin, Role, Token},
};

fn id(label: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: Token::new("pool").unwrap(),
            disk_id: Token::new(label).unwrap(),
            slot_id: Token::new(label).unwrap(),
            binding_generation: 1,
        },
        incarnation: Digest::body(label.as_bytes()),
        role,
    }
}
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .arg("authority")
        .args(args)
        .output()
        .unwrap()
}
fn good(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    json_codec::from_slice(&output.stdout).unwrap()
}
fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn initialize(root: &Path, label: &str, role: Role) -> (PathBuf, PathBuf) {
    let disk = root.join(label);
    let identity = id(label, role);
    let identity_path = root.join(format!("{label}-identity.json"));
    fs::write(&identity_path, json_codec::to_vec(&identity).unwrap()).unwrap();
    good(&[
        "initialize",
        path(&disk),
        path(&identity_path),
        "--accept-new-format",
    ]);
    let selection = root.join(format!("{label}-selection.json"));
    fs::write(
        &selection,
        json_codec::to_vec(&DiskSelection {
            path: disk.clone(),
            identity,
            known_checkpoint: None,
        })
        .unwrap(),
    )
    .unwrap();
    (disk, selection)
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn review_file(selection: &Path, operation: &str, input: &Path) -> Value {
    good(&[
        "review-file",
        path(selection),
        operation,
        "--base",
        "empty",
        "--history",
        "history",
        "--file",
        "file",
        path(input),
    ])
}
fn publish_file(selection: &Path, operation: &str, input: &Path, digest: &str) -> Output {
    run(&[
        "publish-file",
        path(selection),
        operation,
        "--base",
        "empty",
        "--history",
        "history",
        "--file",
        "file",
        path(input),
        "--request-digest",
        digest,
        "--confirm-write",
    ])
}

#[test]
fn authority_requires_explicit_new_format_and_write_confirmation_before_opening_paths() {
    let temp = tempfile::tempdir().unwrap();
    let disk = temp.path().join("disk");
    let identity = temp.path().join("missing");
    let output = run(&["initialize", path(&disk), path(&identity)]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--accept-new-format"));
    assert!(!disk.exists());
    let operation = OperationId::generate().unwrap().to_string();
    let digest = Digest::body(b"review").to_string();
    let output = run(&[
        "publish-file",
        path(&identity),
        &operation,
        "--base",
        "empty",
        "--history",
        "history",
        "--file",
        "file",
        path(&identity),
        "--request-digest",
        &digest,
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--confirm-write"));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn real_cli_primary_backup_portability_projection_and_export_use_only_per_disk_decisions() {
    let temp = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (primary, selection) = initialize(temp.path(), "primary", Role::Primary);
    let input = temp.path().join("input");
    fs::write(&input, b"independent content").unwrap();
    let operation = OperationId::generate().unwrap().to_string();
    let before = files(&primary);
    let review = review_file(&selection, &operation, &input);
    assert_eq!(files(&primary), before);
    let digest = review["request_digest"].as_str().unwrap();
    let output = publish_file(&selection, &operation, &input, digest);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = json_codec::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["sequence"], 1);
    let published = files(&primary);
    let status = good(&["status", path(&selection), &operation]);
    assert_eq!(status["stage"], "published");
    assert_eq!(files(&primary), published);
    let replay = good(&[
        "resume",
        path(&selection),
        &operation,
        "--request-digest",
        digest,
        "--confirm-write",
    ]);
    assert_eq!(replay, receipt);
    assert_eq!(files(&primary), published);
    let (backup, backup_selection) = initialize(temp.path(), "backup", Role::Secondary);
    let sources = temp.path().join("sources.json");
    let selected: Value = json_codec::from_slice(&fs::read(&selection).unwrap()).unwrap();
    fs::write(&sources, json_codec::to_vec(&vec![selected]).unwrap()).unwrap();
    let backup_op = OperationId::generate().unwrap().to_string();
    let review = good(&[
        "review-backup",
        path(&backup_selection),
        &backup_op,
        "--base",
        "empty",
        path(&sources),
    ]);
    good(&[
        "publish-backup",
        path(&backup_selection),
        &backup_op,
        "--base",
        "empty",
        path(&sources),
        "--request-digest",
        review["request_digest"].as_str().unwrap(),
        "--confirm-write",
    ]);
    assert_eq!(files(&primary), published);
    fs::remove_dir_all(&primary).unwrap();
    let report = good(&["inspect", path(&backup_selection)]);
    assert_eq!(report["copies"].as_array().unwrap().len(), 1);
    let commit = report["copies"][0]["commit"].as_str().unwrap();
    let output = temp.path().join("exported");
    good(&["export", path(&backup_selection), commit, path(&output)]);
    assert_eq!(fs::read(&output).unwrap(), b"independent content");
    assert!(
        !run(&["export", path(&backup_selection), commit, path(&output)])
            .status
            .success()
    );
    assert_eq!(fs::read(&output).unwrap(), b"independent content");
    let cache = temp.path().join("system-cache");
    let cached_selections = temp.path().join("cached-selections.json");
    let selected: Value = json_codec::from_slice(&fs::read(&backup_selection).unwrap()).unwrap();
    fs::write(
        &cached_selections,
        json_codec::to_vec(&vec![selected]).unwrap(),
    )
    .unwrap();
    let preserved = files(&backup);
    let rows = good(&["rebuild-projection", path(&cache), path(&cached_selections)]);
    fs::remove_file(cache.join("cache.db")).unwrap();
    assert_eq!(
        good(&["rebuild-projection", path(&cache), path(&cached_selections)]),
        rows
    );
    assert_eq!(files(&backup), preserved);
    fs::remove_file(backup.join("bodies").join(format!("{commit}.bin"))).unwrap();
    let report = good(&["inspect", path(&backup_selection)]);
    assert_eq!(
        report["copies"][0]["availability"],
        "temporarily_unavailable"
    );
    let missing_output = temp.path().join("missing-output");
    assert!(
        !run(&[
            "export",
            path(&backup_selection),
            commit,
            path(&missing_output)
        ])
        .status
        .success()
    );
    assert!(!missing_output.exists());
}

#[test]
fn changed_input_cannot_reuse_the_review_digest_or_modify_target_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "primary", Role::Primary);
    let input = temp.path().join("input");
    fs::write(&input, b"reviewed").unwrap();
    let operation = OperationId::generate().unwrap().to_string();
    let review = review_file(&selection, &operation, &input);
    let before = files(&disk);
    fs::write(&input, b"changed").unwrap();
    let output = publish_file(
        &selection,
        &operation,
        &input,
        review["request_digest"].as_str().unwrap(),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("review changed"));
    assert_eq!(files(&disk), before);
}

#[test]
fn authority_initialization_does_not_upgrade_or_modify_current_format_contents() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let disk = temp.path().join("current");
    fs::create_dir(&disk).unwrap();
    fs::set_permissions(&disk, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        disk.join("seal.json"),
        json_codec::to_vec(&json!({"format":"managed-directory"})).unwrap(),
    )
    .unwrap();
    let identity_path = temp.path().join("identity.json");
    fs::write(
        &identity_path,
        json_codec::to_vec(&id("p", Role::Primary)).unwrap(),
    )
    .unwrap();
    let before = files(&disk);
    assert!(
        !run(&[
            "initialize",
            path(&disk),
            path(&identity_path),
            "--accept-new-format"
        ])
        .status
        .success()
    );
    assert_eq!(files(&disk), before);
}
