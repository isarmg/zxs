//! Real temporary native capsules and independently derived original databases.
//! No real system paths, active target installation or admission are exercised.
use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use storage_engine::*;

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
fn chain_parent(f: &Fixture, name: &str) -> PathBuf {
    let path = f.temp.path().join(name);
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn chain_plan(
    source: &ForeignImportPreview,
    operation: OperationId,
    level: u32,
) -> ForeignMigrationPlan {
    ForeignMigrationPlan::chain_material(source, operation, &"a".repeat(64).parse().unwrap(), level)
        .unwrap()
}
fn published_chain_capsule(
    f: &Fixture,
    name: &str,
    level: u32,
) -> (PathBuf, OperationId, ForeignEvidenceDigest) {
    let source = inspect(&f.engine.config().disks[3].root);
    let parent = chain_parent(f, name);
    let operation = OperationId::generate().unwrap();
    let plan = chain_plan(&source, operation, level);
    let digest = *plan.digest();
    let mut transaction = plan
        .begin_in(File::open(&parent).unwrap(), &parent.join("material"))
        .unwrap();
    transaction.copy_source(&source).unwrap();
    transaction.seal().unwrap();
    transaction.publish(None).unwrap();
    (parent, operation, digest)
}
fn published_owner(
    parent: &Path,
    operation: OperationId,
    digest: &ForeignEvidenceDigest,
) -> ForeignMigrationTransaction {
    ForeignMigrationTransaction::open_published_in(
        File::open(parent).unwrap(),
        &parent.join("material"),
        &operation,
        digest,
    )
    .unwrap()
}
#[test]
fn original_database_preview_recovers_complete_retained_state_without_writing() {
    let f = fixture();
    let (parent, operation, digest) = published_chain_capsule(&f, "scope-preview", 1);
    let before = fs::read_dir(&parent).unwrap().count();
    let owner = published_owner(&parent, operation, &digest);
    let (report, status) = owner.preview_original_databases().unwrap();
    assert_eq!(status, ForeignDatabaseStatus::NotStarted);
    assert_eq!(report.original.sequence, f.engine.state().sequence);
    assert_eq!(
        report.original.operations,
        f.engine.state().operations.len()
    );
    assert_eq!(
        report.original.principals,
        f.engine.state().operation_principals.len()
    );
    assert_eq!(report.original.events, f.engine.state().events.len());
    assert_eq!(report.original_disks, f.engine.config().disks.len());
    assert_eq!(
        report.original.registrations.len(),
        f.engine.config().disks.len()
    );
    for (registration, disk) in report
        .original
        .registrations
        .iter()
        .zip(&f.engine.config().disks)
    {
        assert_eq!(registration.pool_id, f.engine.config().pool_id);
        assert_eq!(registration.disk_id, disk.disk_id);
        assert_eq!(registration.binding_generation, disk.binding_generation);
    }
    assert!(report.file_databases > 0);
    assert_eq!(
        report.databases,
        1 + 2 * report.original_disks + report.file_databases + report.directory_databases
    );
    assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    assert!(!parent.join("foreign-chain-databases").exists());
}
#[test]
fn every_lower_has_equal_original_scopes_but_keeps_its_own_chain_target_binding() {
    let f = fixture();
    let mut reports = Vec::new();
    for level in [1, 2] {
        let (parent, operation, digest) =
            published_chain_capsule(&f, &format!("scopes-level-{level}"), level);
        let owner = published_owner(&parent, operation, &digest);
        let report = owner.prepare_original_databases().unwrap();
        assert_eq!(
            owner.original_database_status().unwrap(),
            ForeignDatabaseStatus::Verified {
                report: report.clone()
            }
        );
        assert_eq!(owner.prepare_original_databases().unwrap(), report);
        assert_eq!(report.target_level, level);
        assert_eq!(report.capsule_digest, digest.to_string());
        reports.push(report);
    }
    assert_eq!(reports[0].original, reports[1].original);
    assert_eq!(reports[0].manifest_hash, reports[1].manifest_hash);
    assert_ne!(reports[0].capsule_digest, reports[1].capsule_digest);
}
#[test]
fn completed_original_database_missing_image_is_not_reconstructed_on_retry() {
    let f = fixture();
    let (parent, operation, digest) = published_chain_capsule(&f, "scopes-missing", 1);
    let owner = published_owner(&parent, operation, &digest);
    owner.prepare_original_databases().unwrap();
    let system = parent.join("foreign-chain-databases/databases/system/index.sqlite");
    fs::remove_file(&system).unwrap();
    assert!(owner.original_database_status().is_err());
    assert!(owner.prepare_original_databases().is_err());
    assert!(!system.exists());
}
#[test]
fn original_scopes_cannot_be_elected_by_markers_after_native_source_material_loss() {
    let f = fixture();
    let (parent, operation, digest) = published_chain_capsule(&f, "scopes-native-loss", 1);
    let owner = published_owner(&parent, operation, &digest);
    owner.prepare_original_databases().unwrap();
    let original = parent.join("material/source/metadata/objects/state-control-auth");
    fs::remove_dir_all(&original).unwrap();
    assert!(owner.original_database_status().is_err());
    assert!(owner.prepare_original_databases().is_err());
    assert!(!original.exists());
    assert!(
        parent
            .join("foreign-chain-databases/complete.json")
            .is_file()
    );
}

#[test]
fn completed_original_database_root_loss_is_not_mistaken_for_a_partial_preparation() {
    let f = fixture();
    let (parent, operation, digest) = published_chain_capsule(&f, "scopes-root-loss", 1);
    let owner = published_owner(&parent, operation, &digest);
    owner.prepare_original_databases().unwrap();
    let root = parent.join("foreign-chain-databases");
    fs::remove_dir_all(&root).unwrap();
    assert!(owner.original_database_status().is_err());
    assert!(owner.prepare_original_databases().is_err());
    assert!(!root.exists());
    assert!(
        parent
            .join(format!(".zxs-foreign-databases-{operation}-complete.json"))
            .is_file()
    );
}
