#[path = "../server/service_namespace.rs"]
mod service_namespace;
const HOST_NAMESPACE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../test/server/service_namespace.py"
);

use json_codec::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use storage_engine::{
    OperationId,
    disk_authority::{
        Digest, DiskSelection, FileWrite, Identity, IndependentDisk, Origin, Role, Token,
    },
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
    Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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

fn fixture_file(selection: &Path, bytes: &[u8]) -> Digest {
    let selection: DiskSelection = json_codec::from_slice(&fs::read(selection).unwrap()).unwrap();
    let disk = IndependentDisk::open(&selection).unwrap();
    let before = disk.report().unwrap();
    let review = disk
        .review_file(FileWrite {
            operation: OperationId::generate().unwrap(),
            expected_base: before.checkpoint,
            history: &Token::new("history").unwrap(),
            file: &Token::new("file").unwrap(),
            bytes,
        })
        .unwrap();
    disk.execute(&review).unwrap();
    disk.report()
        .unwrap()
        .copies
        .into_iter()
        .find(|row| !before.copies.iter().any(|old| old.commit == row.commit))
        .unwrap()
        .commit
}

fn checked_copy(report: &Value, commit: Digest) -> &Value {
    report["copies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["commit"] == commit.to_string())
        .unwrap()
}

fn rejected_check(selection: &Path) -> Output {
    let output = run(&["check", path(selection)]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    assert!(!output.stderr.is_empty());
    output
}

#[test]
fn authority_requires_explicit_new_format_and_write_confirmation_before_opening_paths() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_requires_explicit_new_format_and_write_confirmation_before_opening_paths",
    ) {
        return;
    }
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "real_cli_primary_backup_portability_projection_and_export_use_only_per_disk_decisions",
    ) {
        return;
    }
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "changed_input_cannot_reuse_the_review_digest_or_modify_target_bytes",
    ) {
        return;
    }
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_initialization_does_not_upgrade_or_modify_current_format_contents",
    ) {
        return;
    }
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

#[test]
fn authority_check_reports_empty_without_creating_system_state_or_modifying_disk() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_reports_empty_without_creating_system_state_or_modifying_disk",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "empty", Role::Primary);
    let system = temp.path().join("absent-system");
    let before = files(&disk);
    let report = good(&["--root", path(&system), "check", path(&selection)]);
    assert_eq!(report["format"], "independent-disk-check");
    assert_eq!(report["scope"], "local-retained-published-content");
    assert_eq!(report["state"], "no_published_content");
    assert_eq!(report["checkpoint"], Value::Null);
    assert_eq!(report["sequence"], 0);
    assert!(report["copies"].as_array().unwrap().is_empty());
    assert_eq!(files(&disk), before);
    assert!(!system.exists());
}

#[test]
fn authority_check_does_not_fall_back_to_the_current_pool_format() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_does_not_fall_back_to_the_current_pool_format",
    ) {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let disk = temp.path().join("current-pool-disk");
    fs::create_dir(&disk).unwrap();
    fs::set_permissions(&disk, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(disk.join("disk.json"), br#"{"format":"managed-directory"}"#).unwrap();
    let selection = temp.path().join("selection.json");
    fs::write(
        &selection,
        json_codec::to_vec(&DiskSelection {
            path: disk.clone(),
            identity: id("primary", Role::Primary),
            known_checkpoint: None,
        })
        .unwrap(),
    )
    .unwrap();
    let before = files(&disk);
    rejected_check(&selection);
    assert_eq!(files(&disk), before);
}

#[test]
fn authority_check_keeps_good_versions_visible_beside_missing_and_corrupt_bodies() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_keeps_good_versions_visible_beside_missing_and_corrupt_bodies",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "primary", Role::Primary);
    let corrupt = fixture_file(&selection, b"first");
    let missing = fixture_file(&selection, b"second");
    let valid = fixture_file(&selection, b"third");
    fs::write(disk.join("bodies").join(format!("{corrupt}.bin")), b"XXXXX").unwrap();
    fs::remove_file(disk.join("bodies").join(format!("{missing}.bin"))).unwrap();
    let before = files(&disk);
    let inspect = run(&["inspect", path(&selection)]);
    assert!(!inspect.status.success());
    assert!(inspect.stdout.is_empty());
    let output = run(&["check", path(&selection)]);
    assert!(!output.status.success());
    let report: Value = json_codec::from_slice(&output.stdout).unwrap();
    assert_eq!(report["state"], "content_issues");
    assert_eq!(report["sequence"], 3);
    assert_eq!(report["copies"].as_array().unwrap().len(), 3);
    assert_eq!(
        checked_copy(&report, valid)["content"]["status"],
        "verified"
    );
    assert_eq!(checked_copy(&report, valid)["expected_bytes"], 5);
    assert_eq!(
        checked_copy(&report, missing)["content"]["status"],
        "temporarily_unavailable"
    );
    let bad = checked_copy(&report, corrupt);
    assert_eq!(bad["body"], Digest::body(b"first").to_string());
    assert_eq!(bad["content"]["status"], "corrupt");
    assert_eq!(files(&disk), before);
    let rescued = temp.path().join("rescued");
    good(&[
        "export",
        path(&selection),
        &valid.to_string(),
        path(&rescued),
    ]);
    assert_eq!(fs::read(rescued).unwrap(), b"third");
    assert_eq!(files(&disk), before);
}

#[test]
fn authority_check_preserves_original_owner_and_identical_versions_on_a_moved_backup() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_preserves_original_owner_and_identical_versions_on_a_moved_backup",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (primary, source_selection) = initialize(temp.path(), "primary", Role::Primary);
    let first = fixture_file(&source_selection, b"same");
    let second = fixture_file(&source_selection, b"same");
    let empty = fixture_file(&source_selection, b"");
    assert_ne!(first, second);
    let source: DiskSelection =
        json_codec::from_slice(&fs::read(&source_selection).unwrap()).unwrap();
    let (backup, selection) = initialize(temp.path(), "backup", Role::Secondary);
    let mut selected: DiskSelection =
        json_codec::from_slice(&fs::read(&selection).unwrap()).unwrap();
    let disk = IndependentDisk::open(&selected).unwrap();
    let review = disk
        .review_backup(
            OperationId::generate().unwrap(),
            None,
            std::slice::from_ref(&source),
        )
        .unwrap();
    disk.execute(&review).unwrap();
    selected.known_checkpoint = disk.report().unwrap().checkpoint;
    drop(disk);
    fs::remove_dir_all(primary).unwrap();
    let moved = temp.path().join("moved-backup");
    fs::rename(&backup, &moved).unwrap();
    selected.path = moved.clone();
    fs::write(&selection, json_codec::to_vec(&selected).unwrap()).unwrap();
    let before = files(&moved);
    let report = good(&["check", path(&selection)]);
    assert_eq!(report["state"], "complete_success");
    assert_eq!(
        report["identity"],
        json_codec::to_value(&selected.identity).unwrap()
    );
    assert_eq!(report["copies"].as_array().unwrap().len(), 3);
    for commit in [first, second, empty] {
        let copy = checked_copy(&report, commit);
        assert_eq!(
            copy["owner"],
            json_codec::to_value(&source.identity).unwrap()
        );
        assert_eq!(copy["history_id"], "history");
        assert_eq!(copy["file_id"], "file");
        assert_eq!(copy["content"]["status"], "verified");
    }
    assert_eq!(checked_copy(&report, empty)["expected_bytes"], 0);
    assert_eq!(
        checked_copy(&report, first)["body"],
        checked_copy(&report, second)["body"]
    );
    assert_eq!(files(&moved), before);
}

#[test]
fn authority_check_does_not_merge_matching_names_or_bodies_from_different_primaries() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_does_not_merge_matching_names_or_bodies_from_different_primaries",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (left, left_selection) = initialize(temp.path(), "left", Role::Primary);
    let (right, right_selection) = initialize(temp.path(), "right", Role::Primary);
    let left_commit = fixture_file(&left_selection, b"same");
    let right_commit = fixture_file(&right_selection, b"same");
    assert_ne!(left_commit, right_commit);
    let sources: Vec<DiskSelection> = [&left_selection, &right_selection]
        .into_iter()
        .map(|path| json_codec::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect();
    let (backup, selection) = initialize(temp.path(), "backup", Role::Secondary);
    let selected: DiskSelection = json_codec::from_slice(&fs::read(&selection).unwrap()).unwrap();
    let disk = IndependentDisk::open(&selected).unwrap();
    let review = disk
        .review_backup(OperationId::generate().unwrap(), None, &sources)
        .unwrap();
    disk.execute(&review).unwrap();
    drop(disk);
    fs::remove_dir_all(left).unwrap();
    fs::remove_dir_all(right).unwrap();
    let before = files(&backup);
    let report = good(&["check", path(&selection)]);
    assert_eq!(report["state"], "complete_success");
    assert_eq!(report["copies"].as_array().unwrap().len(), 2);
    for (commit, owner) in [
        (left_commit, &sources[0].identity),
        (right_commit, &sources[1].identity),
    ] {
        let copy = checked_copy(&report, commit);
        assert_eq!(copy["owner"], json_codec::to_value(owner).unwrap());
        assert_eq!(copy["history_id"], "history");
        assert_eq!(copy["file_id"], "file");
        assert_eq!(copy["body"], Digest::body(b"same").to_string());
        assert_eq!(copy["content"]["status"], "verified");
    }
    assert_eq!(files(&backup), before);
}

#[test]
fn authority_check_rejects_identity_pin_and_metadata_failures_without_a_report() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_rejects_identity_pin_and_metadata_failures_without_a_report",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "primary", Role::Primary);
    let commit = fixture_file(&selection, b"body");
    let original_selection = fs::read(&selection).unwrap();
    let before = files(&disk);
    let mut selected: DiskSelection = json_codec::from_slice(&original_selection).unwrap();
    selected.identity.incarnation = Digest::body(b"different");
    fs::write(&selection, json_codec::to_vec(&selected).unwrap()).unwrap();
    rejected_check(&selection);
    assert_eq!(files(&disk), before);
    selected = json_codec::from_slice(&original_selection).unwrap();
    selected.known_checkpoint = Some(Digest::body(b"not-a-checkpoint"));
    fs::write(&selection, json_codec::to_vec(&selected).unwrap()).unwrap();
    rejected_check(&selection);
    assert_eq!(files(&disk), before);
    fs::write(&selection, original_selection).unwrap();
    let metadata = disk.join("objects").join(format!("{commit}.json"));
    fs::write(&metadata, b"corrupt metadata").unwrap();
    let corrupt = files(&disk);
    rejected_check(&selection);
    assert_eq!(files(&disk), corrupt);
    fs::remove_file(metadata).unwrap();
    let missing = files(&disk);
    rejected_check(&selection);
    assert_eq!(files(&disk), missing);
}

#[test]
fn authority_check_refuses_unsafe_and_oversized_body_reads_without_a_report() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_refuses_unsafe_and_oversized_body_reads_without_a_report",
    ) {
        return;
    }
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "primary", Role::Primary);
    let commit = fixture_file(&selection, b"body");
    let body = disk.join("bodies").join(format!("{commit}.bin"));
    let outside = temp.path().join("outside");
    fs::write(&outside, b"not selected").unwrap();
    fs::remove_file(&body).unwrap();
    symlink(&outside, &body).unwrap();
    rejected_check(&selection);
    assert!(
        fs::symlink_metadata(&body)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&outside).unwrap(), b"not selected");
    fs::remove_file(&body).unwrap();
    let oversized = vec![b'x'; (16 << 20) + 1];
    fs::write(&body, &oversized).unwrap();
    fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
    rejected_check(&selection);
    assert_eq!(fs::read(body).unwrap(), oversized);
}

#[test]
fn authority_check_enforces_the_total_metadata_and_content_read_budget() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_enforces_the_total_metadata_and_content_read_budget",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (disk, selection) = initialize(temp.path(), "primary", Role::Primary);
    let first = fixture_file(&selection, b"one");
    let second = fixture_file(&selection, b"two");
    let bytes = vec![b'x'; 8 << 20];
    for commit in [first, second] {
        fs::write(disk.join("bodies").join(format!("{commit}.bin")), &bytes).unwrap();
    }
    let output = rejected_check(&selection);
    assert!(String::from_utf8_lossy(&output.stderr).contains("inventory exceeded its limit"));
    for commit in [first, second] {
        assert_eq!(
            fs::read(disk.join("bodies").join(format!("{commit}.bin"))).unwrap(),
            bytes
        );
    }
    let selected: DiskSelection = json_codec::from_slice(&fs::read(&selection).unwrap()).unwrap();
    let source = IndependentDisk::open(&selected).unwrap();
    let mut snapshot = source.content_snapshot().unwrap();
    assert!(matches!(
        snapshot.read_content(first),
        Err(storage_engine::disk_authority::Error::Corrupt(_))
    ));
    assert!(matches!(
        snapshot.read_content(second),
        Err(storage_engine::disk_authority::Error::Budget)
    ));
    fs::write(disk.join("bodies").join(format!("{first}.bin")), b"one").unwrap();
    assert!(matches!(
        snapshot.read_content(first),
        Err(storage_engine::disk_authority::Error::Budget)
    ));
}

#[test]
fn authority_check_rejects_a_replaced_held_child_even_with_identical_bytes() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "authority_check_rejects_a_replaced_held_child_even_with_identical_bytes",
    ) {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let (path, selection) = initialize(temp.path(), "primary", Role::Primary);
    let commit = fixture_file(&selection, b"body");
    let selection: DiskSelection = json_codec::from_slice(&fs::read(selection).unwrap()).unwrap();
    let disk = IndependentDisk::open(&selection).unwrap();
    let mut snapshot = disk.content_snapshot().unwrap();
    let original = path.join("objects");
    let moved = path.join("original-objects");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(&moved).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), original.join(entry.file_name())).unwrap();
    }
    let before = files(&path);
    assert!(matches!(
        snapshot.read_content(commit),
        Err(storage_engine::disk_authority::Error::Filesystem(
            platform_io::fs_safety::Error::IdentityChanged(_)
        ))
    ));
    assert!(matches!(
        snapshot.recheck(),
        Err(storage_engine::disk_authority::Error::Filesystem(
            platform_io::fs_safety::Error::IdentityChanged(_)
        ))
    ));
    assert!(matches!(
        disk.content_snapshot(),
        Err(storage_engine::disk_authority::Error::Filesystem(
            platform_io::fs_safety::Error::IdentityChanged(_)
        ))
    ));
    assert_eq!(files(&path), before);
}
