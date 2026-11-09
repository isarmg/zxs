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
                report: Box::new(report.clone())
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

fn identity_targets(
    f: &Fixture,
    operation: OperationId,
    lowers: &[PathBuf],
) -> (Engine, ForeignIdentityTarget) {
    let config = Config::development([f.temp.path().join("identity-current")]).unwrap();
    let current = Engine::initialize(&f.temp.path().join("identity-system"), config).unwrap();
    let mut proposed = current.config().clone();
    for (index, root) in lowers
        .iter()
        .cloned()
        .chain(std::iter::once(f.engine.config().disks[3].root.clone()))
        .enumerate()
    {
        proposed.disks.push(DiskConfig {
            disk_id: format!("disk_{:032x}", 500 + index),
            slot_id: format!("NEW{}", index + 1),
            root,
            level: index as u32 + 1,
            upstream_slot_id: index.checked_sub(1).map(|i| format!("NEW{}", i + 1)),
            metadata_member: true,
            binding_generation: 1,
            capacity_bytes: None,
        });
    }
    let target = current
        .capture_foreign_identity_target(ForeignIdentityTargetRequest {
            proposed_configuration: proposed,
            operation_id: operation,
            request_digest: "b".repeat(64).parse().unwrap(),
            chain_digest: "a".repeat(64).parse().unwrap(),
            registration_proposal_digest: "c".repeat(64).parse().unwrap(),
        })
        .unwrap();
    (current, target)
}
fn identity_capsules(
    f: &Fixture,
    operation: OperationId,
) -> (Vec<PathBuf>, Vec<ForeignEvidenceDigest>) {
    let source = inspect(&f.engine.config().disks[3].root);
    let mut roots = Vec::new();
    let mut digests = Vec::new();
    for level in [1, 2] {
        let root = chain_parent(f, &format!("identities-{level}"));
        let plan = chain_plan(&source, operation, level);
        let digest = *plan.digest();
        digests.push(digest);
        let mut owner = plan
            .begin_in(File::open(&root).unwrap(), &root.join("material"))
            .unwrap();
        owner.copy_source(&source).unwrap();
        owner.seal().unwrap();
        owner.publish(None).unwrap();
        let owner = published_owner(&root, operation, &digest);
        owner.prepare_original_databases().unwrap();
        roots.push(root);
    }
    (roots, digests)
}
#[test]
fn object_identity_mapping_is_independent_equal_and_keeps_foreign_receipts_out_of_live_state() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (current, target) = identity_targets(&f, operation, &roots);
    let before = json_codec::to_vec(current.state()).unwrap();
    let mut reports = Vec::new();
    for (root, digest) in roots.iter().zip(&digests) {
        let owner = published_owner(root, operation, digest);
        let count = fs::read_dir(root).unwrap().count();
        let (report, state) = owner.preview_identity_closure(&target).unwrap();
        assert_eq!(state, ForeignIdentityClosureStatus::NotStarted);
        assert_eq!(fs::read_dir(root).unwrap().count(), count);
        assert!(report.objects > 0);
        assert_eq!(
            report.archived_operations,
            f.engine.state().operations.len()
        );
        assert_eq!(report.archived_events, f.engine.state().events.len());
        assert_eq!(owner.prepare_identity_closure(&target).unwrap(), report);
        assert_eq!(
            owner.preview_identity_closure(&target).unwrap().1,
            ForeignIdentityClosureStatus::Verified {
                report: Box::new(report.clone())
            }
        );
        let map: json_codec::Value = json_codec::from_slice(
            &fs::read(root.join(format!(".zxs-foreign-identities-{operation}.json"))).unwrap(),
        )
        .unwrap();
        for mapped in map["archived_operations"].as_object().unwrap().values() {
            assert!(mapped.as_str().unwrap().starts_with("foreignop_"));
            assert!(mapped.as_str().unwrap().parse::<OperationId>().is_err());
        }
        assert!(map.get("control_auth").is_none());
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(json_codec::to_vec(current.state()).unwrap(), before);
    current.recheck_foreign_identity_target(&target).unwrap();
}
#[test]
fn finished_identity_mapping_loss_cannot_recreate_from_a_healthy_peer() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    for (root, digest) in roots.iter().zip(&digests) {
        published_owner(root, operation, digest)
            .prepare_identity_closure(&target)
            .unwrap();
    }
    let path = roots[0].join(format!(".zxs-foreign-identities-{operation}.json"));
    fs::remove_file(&path).unwrap();
    let owner = published_owner(&roots[0], operation, &digests[0]);
    assert!(owner.preview_identity_closure(&target).is_err());
    assert!(owner.prepare_identity_closure(&target).is_err());
    assert!(!path.exists());
    assert!(
        published_owner(&roots[1], operation, &digests[1])
            .preview_identity_closure(&target)
            .is_ok()
    );
}
#[test]
fn identity_preparation_binds_current_state_not_just_unchanged_configuration() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, _) = identity_capsules(&f, operation);
    let (mut current, target) = identity_targets(&f, operation, &roots);
    save(&mut current, "changed-current-state", "P01", 1);
    assert!(current.recheck_foreign_identity_target(&target).is_err());
}
#[test]
fn identity_closure_refuses_wrong_chain_or_missing_original_database() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    let owner = published_owner(&roots[0], operation, &digests[0]);
    fs::remove_file(roots[0].join("foreign-chain-databases/databases/system/index.sqlite"))
        .unwrap();
    assert!(owner.preview_identity_closure(&target).is_err());
    assert!(
        !roots[0]
            .join(format!(".zxs-foreign-identities-{operation}.json"))
            .exists()
    );
}

#[test]
fn interrupted_identity_final_marker_only_finishes_after_exact_material_readback() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    let owner = published_owner(&roots[0], operation, &digests[0]);
    let report = owner.prepare_identity_closure(&target).unwrap();
    let mapping = roots[0].join(format!(".zxs-foreign-identities-{operation}.json"));
    let before = fs::read(&mapping).unwrap();
    fs::remove_file(roots[0].join(format!(".zxs-foreign-identities-{operation}-complete.json")))
        .unwrap();
    assert_eq!(
        owner.preview_identity_closure(&target).unwrap().1,
        ForeignIdentityClosureStatus::Preparing
    );
    assert_eq!(owner.prepare_identity_closure(&target).unwrap(), report);
    assert_eq!(fs::read(mapping).unwrap(), before);
}
#[test]
fn changed_finished_identity_bytes_are_preserved_and_never_replaced() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    let owner = published_owner(&roots[0], operation, &digests[0]);
    owner.prepare_identity_closure(&target).unwrap();
    let mapping = roots[0].join(format!(".zxs-foreign-identities-{operation}.json"));
    fs::write(&mapping, b"changed original mapping").unwrap();
    assert!(owner.preview_identity_closure(&target).is_err());
    assert!(owner.prepare_identity_closure(&target).is_err());
    assert_eq!(fs::read(&mapping).unwrap(), b"changed original mapping");
}

#[test]
fn history_graph_requires_complete_original_identity_material_before_writes() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    let owner = published_owner(&roots[0], operation, &digests[0]);
    let before = fs::read_dir(&roots[0]).unwrap().count();
    assert!(owner.preview_history_graph(&target).is_err());
    assert!(owner.prepare_history_graph(&target).is_err());
    assert_eq!(fs::read_dir(&roots[0]).unwrap().count(), before);
}
#[test]
fn every_lower_derives_equal_mapped_graphs_without_turning_missing_bodies_into_ready() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (current, target) = identity_targets(&f, operation, &roots);
    let before = json_codec::to_vec(current.state()).unwrap();
    let mut reports = Vec::new();
    for (root, digest) in roots.iter().zip(&digests) {
        let owner = published_owner(root, operation, digest);
        owner.prepare_identity_closure(&target).unwrap();
        let count = fs::read_dir(root).unwrap().count();
        let (expected, status) = owner.preview_history_graph(&target).unwrap();
        assert_eq!(status, ForeignHistoryGraphStatus::NotStarted);
        assert_eq!(fs::read_dir(root).unwrap().count(), count);
        assert_eq!(expected.histories, f.engine.state().histories.len());
        assert!(expected.unavailable_versions > 0);
        let report = owner.prepare_history_graph(&target).unwrap();
        assert_eq!(report, expected);
        assert_eq!(
            owner.preview_history_graph(&target).unwrap().1,
            ForeignHistoryGraphStatus::Verified {
                report: Box::new(report.clone())
            }
        );
        let graph: json_codec::Value = json_codec::from_slice(
            &fs::read(root.join(format!(".zxs-foreign-graphs-{operation}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            graph["format"],
            "zxs-foreign-source-scoped-mapped-history-graphs"
        );
        for history in graph["histories"].as_array().unwrap() {
            assert_ne!(
                history["original_history_id"],
                history["graph"]["history"]["history_tree_id"]
            );
            assert_eq!(
                history["graph"]["versions"].as_object().unwrap().len(),
                history["original_version_availability"]
                    .as_object()
                    .unwrap()
                    .len()
            );
        }
        assert!(graph.get("operations").is_none());
        assert!(graph.get("control_auth").is_none());
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(json_codec::to_vec(current.state()).unwrap(), before);
    current.recheck_foreign_identity_target(&target).unwrap();
}
#[test]
fn completed_history_graph_loss_cannot_be_recreated_from_another_lower() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    for (root, digest) in roots.iter().zip(&digests) {
        let owner = published_owner(root, operation, digest);
        owner.prepare_identity_closure(&target).unwrap();
        owner.prepare_history_graph(&target).unwrap();
    }
    let path = roots[0].join(format!(".zxs-foreign-graphs-{operation}.json"));
    fs::remove_file(&path).unwrap();
    let owner = published_owner(&roots[0], operation, &digests[0]);
    assert!(owner.preview_history_graph(&target).is_err());
    assert!(owner.prepare_history_graph(&target).is_err());
    assert!(!path.exists());
    assert!(
        published_owner(&roots[1], operation, &digests[1])
            .preview_history_graph(&target)
            .is_ok()
    );
}
#[test]
fn interrupted_history_graph_completion_preserves_and_rechecks_original_bytes() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let (roots, digests) = identity_capsules(&f, operation);
    let (_, target) = identity_targets(&f, operation, &roots);
    let owner = published_owner(&roots[0], operation, &digests[0]);
    owner.prepare_identity_closure(&target).unwrap();
    let expected = owner.prepare_history_graph(&target).unwrap();
    let marker = roots[0].join(format!(".zxs-foreign-graphs-{operation}-complete.json"));
    let path = roots[0].join(format!(".zxs-foreign-graphs-{operation}.json"));
    let original = fs::read(&path).unwrap();
    fs::remove_file(&marker).unwrap();
    assert_eq!(
        owner.preview_history_graph(&target).unwrap().1,
        ForeignHistoryGraphStatus::Preparing
    );
    assert_eq!(owner.prepare_history_graph(&target).unwrap(), expected);
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::write(&path, b"changed graph").unwrap();
    assert!(owner.prepare_history_graph(&target).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"changed graph");
}
