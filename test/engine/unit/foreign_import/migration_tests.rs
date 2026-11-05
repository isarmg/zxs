use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
}

fn save(engine: &mut Engine, name: &str, slot: &str, copies: u32) {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            slot,
            copies,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "document.txt", name.as_bytes())
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config =
        Config::development((0..4).map(|index| temp.path().join(format!("disk-{index}")))).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    config.disks[2].level = 2;
    config.disks[2].upstream_slot_id = Some("P01".into());
    config.disks[3].level = 3;
    config.disks[3].upstream_slot_id = Some(config.disks[2].slot_id.clone());
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    save(&mut engine, "first-origin", "P01", 3);
    save(&mut engine, "first-origin-again", "P01", 3);
    save(&mut engine, "second-origin", "B01", 1);
    Fixture { temp, engine }
}

fn inspect(path: &Path) -> ForeignImportPreview {
    ForeignImportPreview::inspect(
        File::open(path).unwrap(),
        path.into(),
        ForeignImportLimits::default(),
    )
    .unwrap()
}
fn plan(source: &ForeignImportPreview) -> ForeignMigrationPlan {
    ForeignMigrationPlan::primary(source, OperationId::generate().unwrap()).unwrap()
}
fn output(f: &Fixture) -> PathBuf {
    f.temp.path().join("capsule")
}
fn body(f: &Fixture) -> PathBuf {
    let version = f
        .engine
        .state()
        .versions
        .values()
        .find(|version| version.creation_primary_slot_id == "P01")
        .unwrap();
    let directory = version_dir(
        &f.engine.config().disks[2],
        &version.history_tree_id,
        &version.version_id,
    );
    crate::container_storage::content_path(&directory, version).unwrap()
}
fn target() -> (tempfile::TempDir, Engine, PortablePath) {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        Config::independent_roots(Mode::Development, [temp.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine
        .create_disk_root("D01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    let directory = PortablePath::parse("Restored").unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::Mkdir {
                path: directory.clone(),
            },
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    (temp, engine, directory)
}
fn names(source: &ForeignImportPreview) -> BTreeMap<ForeignPrimaryOrigin, String> {
    source
        .histories
        .iter()
        .map(|history| {
            (
                history.origin.clone(),
                format!("origin-{}", history.origin.slot_id),
            )
        })
        .collect()
}

#[test]
fn all_three_source_roles_can_produce_primary_plans_grouped_by_exact_origin() {
    let f = fixture();
    for index in [0, 2, 3] {
        let source = inspect(&f.engine.config().disks[index].root);
        let plan = plan(&source);
        assert_eq!(plan.origin_count(), 2);
        assert_eq!(
            plan.record
                .groups
                .iter()
                .map(|group| group.histories.len())
                .sum::<usize>(),
            3
        );
        assert!(
            plan.record
                .groups
                .iter()
                .any(|group| group.histories.len() == 2)
        );
        assert_eq!(plan.record.source_level, source.source_level());
    }
}

#[test]
fn published_capsule_preserves_graph_proofs_and_unavailable_bodies_without_source() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let seal = source.original_seal().to_vec();
    let plan = plan(&source);
    let digest = *plan.digest();
    let operation = *plan.operation_id();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    let receipt = transaction.publish(None).unwrap();
    assert_eq!(receipt.origin_count(), 2);
    assert_eq!(receipt.digest(), &digest);
    let copied = inspect(&receipt.path().join("source"));
    assert_eq!(copied.digest(), source.digest());
    assert_eq!(copied.original_seal(), seal);
    assert!(copied.histories.iter().any(|history| {
        history
            .availability
            .values()
            .any(|state| *state == ForeignVersionAvailability::TemporarilyUnavailable)
    }));
    assert_eq!(
        fs::read(source.root.path().join("disk.json")).unwrap(),
        seal
    );
    fs::rename(
        source.root.path(),
        f.temp.path().join("disconnected-source"),
    )
    .unwrap();
    drop(source);
    let retry = ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    assert_eq!(retry.publish(None).unwrap().digest(), &digest);
}

#[test]
fn rebuild_requires_secondary_empty_directory_and_complete_distinct_origin_names() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let (_target, engine, directory) = target();
    let names = names(&source);
    let plan = ForeignMigrationPlan::rebuild(
        &source,
        OperationId::generate().unwrap(),
        &engine,
        "D01",
        &directory,
        &names,
    )
    .unwrap();
    assert_eq!(plan.origin_count(), 2);
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(Some(&engine)).unwrap();
    let primary = inspect(&f.engine.config().disks[0].root);
    assert!(
        ForeignMigrationPlan::rebuild(
            &primary,
            OperationId::generate().unwrap(),
            &engine,
            "D01",
            &directory,
            &names
        )
        .is_err()
    );
    let mut missing = names.clone();
    missing.pop_first();
    assert!(
        ForeignMigrationPlan::rebuild(
            &source,
            OperationId::generate().unwrap(),
            &engine,
            "D01",
            &directory,
            &missing
        )
        .is_err()
    );
    let mut collision = names;
    for (index, name) in collision.values_mut().enumerate() {
        *name = if index == 0 { "Same" } else { "same" }.into();
    }
    assert!(
        ForeignMigrationPlan::rebuild(
            &source,
            OperationId::generate().unwrap(),
            &engine,
            "D01",
            &directory,
            &collision
        )
        .is_err()
    );
    assert!(
        ForeignMigrationPlan::rebuild(
            &source,
            OperationId::generate().unwrap(),
            &engine,
            "D01",
            &PortablePath::parse("Absent").unwrap(),
            &collision
        )
        .is_err()
    );
}

#[test]
fn tertiary_to_secondary_binds_an_accepted_primary_and_rejects_stale_target() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[3].root);
    let (_target, mut engine, _directory) = target();
    assert!(
        ForeignMigrationPlan::secondary(
            &source,
            OperationId::generate().unwrap(),
            &engine,
            "unknown"
        )
        .is_err()
    );
    assert!(
        ForeignMigrationPlan::secondary(
            &inspect(&f.engine.config().disks[2].root),
            OperationId::generate().unwrap(),
            &engine,
            "D01"
        )
        .is_err()
    );
    assert!(
        ForeignMigrationPlan::secondary(
            &source,
            OperationId::generate().unwrap(),
            &f.engine,
            "C01"
        )
        .is_err()
    );
    let plan =
        ForeignMigrationPlan::secondary(&source, OperationId::generate().unwrap(), &engine, "D01")
            .unwrap();
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    assert!(transaction.publish(None).is_err());
    save(&mut engine, "checkpoint-moved", "D01", 1);
    let transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    assert_eq!(
        transaction.publish(Some(&engine)).err().unwrap().code(),
        ErrorCode::PlanStale
    );
    assert!(!output(&f).exists());
}

#[test]
fn interrupted_stages_resume_and_never_overwrite_conflicting_bytes() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    drop(plan.begin(&output(&f)).unwrap());
    let mut transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    transaction.copy_source(&source).unwrap();
    let relative = source
        .root
        .path()
        .strip_prefix(source.root.path())
        .unwrap()
        .join(body(&f).strip_prefix(source.root.path()).unwrap());
    let copied_body = transaction.capsule.path().join("source").join(relative);
    fs::set_permissions(&copied_body, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&copied_body, b"conflicting bytes").unwrap();
    drop(transaction);
    let mut transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    assert!(transaction.copy_source(&source).is_err());
    assert!(transaction.seal().is_err());
    assert_eq!(fs::read(&copied_body).unwrap(), b"conflicting bytes");
    assert!(!output(&f).exists());
}

#[test]
fn reservation_recovers_before_mkdir_and_after_empty_staging_creation() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let transaction = plan.begin(&output(&f)).unwrap();
    let stage = transaction.capsule.path().to_owned();
    drop(transaction);
    fs::remove_file(stage.join("binding.json")).unwrap();
    fs::remove_dir(&stage).unwrap();
    let transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    drop(transaction);
    fs::remove_file(stage.join("binding.json")).unwrap();
    let mut transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(None).unwrap();
}

#[test]
fn existing_destination_and_racing_destination_are_never_overwritten() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    fs::write(output(&f), b"keep").unwrap();
    assert!(plan(&source).begin(&output(&f)).is_err());
    assert_eq!(fs::read(output(&f)).unwrap(), b"keep");
    fs::remove_file(output(&f)).unwrap();
    let mut transaction = plan(&source).begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    fs::write(output(&f), b"racing data").unwrap();
    assert_eq!(
        transaction.publish(None).err().unwrap().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(output(&f)).unwrap(), b"racing data");
}

#[test]
fn source_change_after_review_or_copy_cannot_publish() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let path = body(&f);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, b"changed source body").unwrap();
    assert!(ForeignMigrationPlan::primary(&source, OperationId::generate().unwrap()).is_err());
    let mut transaction = plan.begin(&output(&f)).unwrap();
    assert!(transaction.copy_source(&source).is_err());
    assert!(!output(&f).exists());
    assert_eq!(fs::read(&path).unwrap(), b"changed source body");
}

#[test]
fn missing_body_is_preserved_but_symlink_and_hardlink_inputs_are_rejected() {
    let f = fixture();
    let path = body(&f);
    fs::remove_file(&path).unwrap();
    let source = inspect(&f.engine.config().disks[2].root);
    let mut transaction = plan(&source).begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(None).unwrap();
    let outside = f.temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    let junk = source.root.path().join("foreign-link");
    std::os::unix::fs::symlink(&outside, &junk).unwrap();
    assert!(ForeignMigrationPlan::primary(&source, OperationId::generate().unwrap()).is_err());
    fs::remove_file(&junk).unwrap();
    fs::hard_link(&outside, &junk).unwrap();
    assert!(ForeignMigrationPlan::primary(&source, OperationId::generate().unwrap()).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn replaced_stage_and_wrong_review_digest_are_rejected() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let transaction = plan.begin(&output(&f)).unwrap();
    let stage = transaction.capsule.path().to_owned();
    drop(transaction);
    let wrong = ForeignEvidenceDigest::from_digest(blake3::hash(b"wrong"));
    assert!(ForeignMigrationTransaction::resume(&output(&f), &operation, &wrong).is_err());
    fs::rename(&stage, f.temp.path().join("original-stage")).unwrap();
    fs::create_dir(&stage).unwrap();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    fs::copy(
        f.temp.path().join("original-stage/binding.json"),
        stage.join("binding.json"),
    )
    .unwrap();
    assert!(ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).is_err());
}

#[test]
fn source_descendant_and_unbound_unrelated_stage_are_rejected() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    assert!(
        plan(&source)
            .begin(&source.root.path().join("capsule"))
            .is_err()
    );
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let transaction = plan.begin(&output(&f)).unwrap();
    let stage = transaction.capsule.path().to_owned();
    drop(transaction);
    fs::remove_file(stage.join("binding.json")).unwrap();
    fs::write(stage.join("unrelated"), b"keep").unwrap();
    assert!(ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).is_err());
    assert_eq!(fs::read(stage.join("unrelated")).unwrap(), b"keep");
}

#[test]
fn atomic_copy_and_projection_residue_are_preserved_outside_the_proof_tree() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let mut transaction = plan(&source).begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    let payload_temp = ".zxs-payload-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tmp";
    fs::write(
        transaction.capsule.path().join("source").join(payload_temp),
        b"interrupted copy",
    )
    .unwrap();
    transaction.copy_source(&source).unwrap();
    assert_eq!(
        fs::read(
            transaction
                .capsule
                .path()
                .join("residue/source")
                .join(payload_temp)
        )
        .unwrap(),
        b"interrupted copy"
    );
    transaction.seal().unwrap();
    let group = transaction.intent.plan.groups[0].directory.clone();
    let atomic_temp = ".state-atomic-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tmp";
    fs::write(
        transaction
            .capsule
            .path()
            .join("origins")
            .join(&group)
            .join(atomic_temp),
        b"interrupted descriptor",
    )
    .unwrap();
    transaction.seal().unwrap();
    let receipt = transaction.publish(None).unwrap();
    assert_eq!(
        fs::read(
            receipt
                .path()
                .join("residue/origins")
                .join(group)
                .join(atomic_temp)
        )
        .unwrap(),
        b"interrupted descriptor"
    );
}

#[test]
fn a_renamed_sealed_stage_cannot_bypass_required_primary_acceptance() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[3].root);
    let (_target, engine, _directory) = target();
    let plan =
        ForeignMigrationPlan::secondary(&source, OperationId::generate().unwrap(), &engine, "D01")
            .unwrap();
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    let stage = transaction.capsule.path().to_owned();
    drop(transaction);
    fs::rename(stage, output(&f)).unwrap();
    assert!(ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).is_err());
    assert!(!output(&f).join("ready.json").exists());
}

// This libtest entry also serves as an isolated process-crash driver. Exit
// deliberately skips all Rust destructors; the parent opens a new transaction.
#[test]
fn crash_child() {
    let Ok(phase) = std::env::var("ZXS_FOREIGN_CRASH_PHASE") else {
        return;
    };
    let source_path = PathBuf::from(std::env::var_os("ZXS_FOREIGN_CRASH_SOURCE").unwrap());
    let destination = PathBuf::from(std::env::var_os("ZXS_FOREIGN_CRASH_TARGET").unwrap());
    let operation: OperationId = std::env::var("ZXS_FOREIGN_CRASH_OPERATION")
        .unwrap()
        .parse()
        .unwrap();
    let source = inspect(&source_path);
    let plan = ForeignMigrationPlan::primary(&source, operation).unwrap();
    let mut transaction = plan.begin(&destination).unwrap();
    if phase == "prepared" {
        std::process::exit(92);
    }
    if phase == "mid_copy" {
        struct InterruptedCopy(bool);
        impl std::io::Read for InterruptedCopy {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    std::process::exit(92);
                }
                self.0 = true;
                output[..7].copy_from_slice(b"partial");
                Ok(7)
            }
        }
        let path = transaction.capsule.path().join("source");
        transaction.capsule.private_dir(&path).unwrap();
        let directory = transaction.capsule.private_directory(&path).unwrap();
        platform_io::payload::write_full(
            &directory,
            &EntryName::new("disk.json").unwrap(),
            &mut InterruptedCopy(false),
            platform_io::payload::Expectation {
                bytes: 1024,
                blake3: None,
            },
        )
        .unwrap();
        panic!("child should exit from inside the copy reader");
    }
    transaction.copy_source(&source).unwrap();
    if phase == "copied" {
        std::process::exit(92);
    }
    transaction.seal().unwrap();
    if phase == "sealed" {
        std::process::exit(92);
    }
    // Model the point after the no-replace rename and before its parent fsync.
    // The real publish method writes this marker after target checks; Primary
    // has no existing-target precondition to simulate here.
    let sealed = transaction
        .capsule
        .read_bytes(
            &transaction.capsule.path().join("sealed.json"),
            MAX_METADATA,
        )
        .unwrap();
    transaction::ensure_exact(
        &transaction.capsule,
        &transaction.capsule.path().join("ready.json"),
        &sealed,
    )
    .unwrap();
    let parent = transaction
        .parent
        .directory(transaction.parent.path())
        .unwrap();
    rustix::fs::renameat_with(
        &parent,
        transaction.capsule.path().file_name().unwrap(),
        &parent,
        destination.file_name().unwrap(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .unwrap();
    std::process::exit(92);
}

#[test]
fn process_exit_at_durable_boundaries_recovers_the_same_operation() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    for phase in ["prepared", "mid_copy", "copied", "sealed", "renamed"] {
        let plan = plan(&source);
        let operation = *plan.operation_id();
        let digest = *plan.digest();
        let destination = f.temp.path().join(format!("capsule-{phase}"));
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "foreign_import::migration::tests::crash_child",
                "--nocapture",
            ])
            .env("ZXS_FOREIGN_CRASH_PHASE", phase)
            .env("ZXS_FOREIGN_CRASH_SOURCE", source.root.path())
            .env("ZXS_FOREIGN_CRASH_TARGET", &destination)
            .env("ZXS_FOREIGN_CRASH_OPERATION", operation.to_string())
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(92),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut transaction =
            ForeignMigrationTransaction::resume(&destination, &operation, &digest).unwrap();
        if !transaction.published {
            transaction.copy_source(&source).unwrap();
            transaction.seal().unwrap();
        } else {
            assert!(transaction.seal().is_err());
        }
        assert_eq!(transaction.publish(None).unwrap().digest(), &digest);
    }
}

fn copy_capsule(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_capsule(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn capsule_copy_is_independently_verifiable_without_source_or_parent_reservation() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let digest = *plan.digest();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    let receipt = transaction.publish(None).unwrap();
    let transported = tempfile::tempdir().unwrap();
    let copy = transported.path().join("portable-capsule");
    copy_capsule(receipt.path(), &copy);
    drop(source);
    drop(f);
    assert_eq!(
        ForeignMigrationReceipt::verify_capsule(&copy, &digest)
            .unwrap()
            .origin_count(),
        2
    );
    fs::write(copy.join("source/disk.json"), b"corrupt original proof").unwrap();
    assert!(ForeignMigrationReceipt::verify_capsule(&copy, &digest).is_err());
    assert_eq!(
        fs::read(copy.join("source/disk.json")).unwrap(),
        b"corrupt original proof"
    );
}

#[test]
fn logical_source_copy_cannot_replace_the_reviewed_physical_selection() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let mut transaction = plan(&source).begin(&output(&f)).unwrap();
    let copied_path = f.temp.path().join("other-source");
    copy_capsule(source.root.path(), &copied_path);
    let copied = inspect(&copied_path);
    assert_eq!(copied.digest(), source.digest());
    assert_ne!(copied.local_binding(), source.local_binding());
    assert_eq!(
        transaction.copy_source(&copied).unwrap_err().code(),
        ErrorCode::ObjectCollision
    );
    assert!(!transaction.capsule.path().join("source").exists());
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(None).unwrap();
    assert_eq!(inspect(&copied_path).digest(), source.digest());
}

#[test]
fn a_moved_parent_cannot_resume_inside_the_protected_source() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let parent = f.temp.path().join("original-parent");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let destination = parent.join("capsule");
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let mut transaction = plan.begin(&destination).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    let stage_name = transaction.capsule.path().file_name().unwrap().to_owned();
    let sealed = fs::read(transaction.capsule.path().join("sealed.json")).unwrap();
    let moved = source.root.path().join("moved-parent");
    fs::rename(&parent, &moved).unwrap();
    assert!(transaction.publish(None).is_err());
    assert!(
        ForeignMigrationTransaction::resume(&moved.join("capsule"), &operation, &digest).is_err()
    );
    assert_eq!(
        fs::read(moved.join(&stage_name).join("sealed.json")).unwrap(),
        sealed
    );
    assert!(!moved.join("capsule").exists());
    // Restoring the exact reviewed name permits the same operation to finish.
    fs::rename(&moved, &parent).unwrap();
    let transaction =
        ForeignMigrationTransaction::resume(&destination, &operation, &digest).unwrap();
    transaction.publish(None).unwrap();
    assert_eq!(inspect(source.root.path()).digest(), source.digest());
}

#[test]
fn repeated_resumes_preserve_each_interrupted_copy_outside_the_proof_tree() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let plan = plan(&source);
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    let temporary_names = [
        ".zxs-payload-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tmp",
        ".zxs-payload-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tmp",
    ];
    for name in temporary_names {
        fs::write(transaction.capsule.path().join("source").join(name), name).unwrap();
        drop(transaction);
        transaction =
            ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
        transaction.copy_source(&source).unwrap();
    }
    transaction.seal().unwrap();
    let receipt = transaction.publish(None).unwrap();
    for name in temporary_names {
        assert_eq!(
            fs::read(receipt.path().join("residue/source").join(name)).unwrap(),
            name.as_bytes()
        );
        assert!(!receipt.path().join("source").join(name).exists());
    }
    ForeignMigrationReceipt::verify_capsule(receipt.path(), &digest).unwrap();
}

#[test]
fn accepted_tertiary_to_secondary_plan_publishes_and_retries_historically() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[3].root);
    let (_target, mut engine, _directory) = target();
    let plan =
        ForeignMigrationPlan::secondary(&source, OperationId::generate().unwrap(), &engine, "D01")
            .unwrap();
    let operation = *plan.operation_id();
    let digest = *plan.digest();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(Some(&engine)).unwrap();
    save(&mut engine, "later-checkpoint", "D01", 1);
    let transaction =
        ForeignMigrationTransaction::resume(&output(&f), &operation, &digest).unwrap();
    transaction.publish(None).unwrap();
    let capsule = ForeignMigrationReceipt::verify_capsule(&output(&f), &digest).unwrap();
    assert_eq!(capsule.origin_count(), 2);
}

#[test]
fn reconstruction_rechecks_target_after_review_and_inventory_limits_unknown_bytes() {
    let f = fixture();
    let source = inspect(&f.engine.config().disks[2].root);
    let (_target, mut engine, directory) = target();
    let plan = ForeignMigrationPlan::rebuild(
        &source,
        OperationId::generate().unwrap(),
        &engine,
        "D01",
        &directory,
        &names(&source),
    )
    .unwrap();
    let mut transaction = plan.begin(&output(&f)).unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    save(&mut engine, "new-target-state", "D01", 1);
    assert_eq!(
        transaction.publish(Some(&engine)).err().unwrap().code(),
        ErrorCode::PlanStale
    );
    assert!(!output(&f).exists());
    let source = ForeignImportPreview::inspect(
        source.root.directory(source.root.path()).unwrap(),
        source.root.path().into(),
        ForeignImportLimits::new(8 << 20, 8 << 20, 100, 1024).unwrap(),
    )
    .unwrap();
    let extra = source.root.path().join("unreferenced-large-file");
    File::create(&extra).unwrap().set_len(17 << 20).unwrap();
    assert_eq!(
        ForeignMigrationPlan::primary(&source, OperationId::generate().unwrap())
            .err()
            .unwrap()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(fs::metadata(extra).unwrap().len(), 17 << 20);
}
