//! Production stop consumers with real control files and the four store faults.
//! Source regressions only; these are not physical power-loss acceptance results.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{WriteFault, inject_next};
use crate::engine::membership_transition::pending::activation::plan_store::PLAN;
use std::{fs, os::unix::fs::PermissionsExt};

const FAULTS: [WriteFault; 4] = [
    WriteFault::BeforeRecord,
    WriteFault::AfterRecord,
    WriteFault::BeforeHead,
    WriteFault::AfterHead,
];
struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    target: DiskConfig,
    operation: OperationId,
    history: Vec<u8>,
    old_operation: OperationId,
    prior_target: Option<PathBuf>,
}
impl Fixture {
    fn prepared(relocate: bool, prior_stop: bool) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let count = if prior_stop { 4 } else { 3 };
        let config =
            Config::development((0..count).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let system = temp.path().join("system");
        let mut engine = Engine::initialize(&system, config.clone()).unwrap();
        let old_operation = OperationId::generate().unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("unchanged").unwrap(),
                "P01",
                1,
                "admin",
                old_operation,
            )
            .unwrap();
        // Relocate before creating absolute workspace staging paths. Genesis
        // has no authenticated historical aliases for an old staging address.
        if relocate {
            let next = engine.config.clone();
            drop(engine);
            let roots = next
                .disks
                .iter()
                .enumerate()
                .map(|(i, disk)| {
                    let path = temp.path().join(format!("relocated-{i}"));
                    fs::rename(&disk.root, &path).unwrap();
                    path
                })
                .collect();
            let next = next.with_disk_roots(roots).unwrap();
            write_json(&system.join("config.json"), &next, true).unwrap();
            engine = Engine::open(&system).unwrap();
        }
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "kept.txt",
                b"stop recovery never writes payload or history",
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
        let prior_target = if prior_stop {
            let first = engine.config.disks[0].clone();
            prepare(
                &mut engine,
                &first.disk_id,
                OperationId::generate().unwrap(),
            );
            engine.activate_pending_disk_exit().unwrap();
            Some(first.root)
        } else {
            None
        };
        let target = engine.config.disks[usize::from(prior_stop)].clone();
        let operation = OperationId::generate().unwrap();
        prepare(&mut engine, &target.disk_id, operation);
        engine.prepare_disk_exit_generation().unwrap();
        let history = canonical_json(&(
            &engine.state.versions,
            &engine.state.commits,
            &engine.state.operations[&old_operation],
        ))
        .unwrap();
        Self {
            temp,
            engine,
            target,
            operation,
            history,
            old_operation,
            prior_target,
        }
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn plan_path(&self) -> PathBuf {
        self.system()
            .join(DIRECTORY)
            .join(self.operation.to_string())
            .join(PLAN)
    }
    fn crash(&mut self, fault: WriteFault) {
        inject_next(fault);
        assert!(
            self.engine.activate_pending_disk_exit().is_err(),
            "real Exit must stop at {fault:?}"
        );
        assert!(!self.engine.state.operations.contains_key(&self.operation));
    }
    fn assert_history(&self) {
        assert_eq!(
            canonical_json(&(
                &self.engine.state.versions,
                &self.engine.state.commits,
                &self.engine.state.operations[&self.old_operation]
            ))
            .unwrap(),
            self.history
        );
    }
}
fn prepare(engine: &mut Engine, disk: &str, operation: OperationId) {
    engine
        .begin_disk_exit(
            disk,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation,
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, path: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, found);
            } else {
                found.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(root, root, &mut found);
    found
}

#[test]
fn exit_stop_all_four_real_barriers_resume_live_without_target_io() {
    for fault in FAULTS {
        for (relocate, second_publication) in [(false, false), (true, false), (true, true)] {
            let mut fixture = Fixture::prepared(relocate, false);
            let target = Arc::clone(fixture.engine.disk_resources.get(&fixture.target).unwrap());
            let before = files(&fixture.target.root);
            if second_publication {
                fixture.crash(WriteFault::AfterHead);
            }
            fixture.crash(fault);
            assert!(
                target.root.admit_io().is_err(),
                "uncertain publication must close ordinary admission"
            );
            let plan = fs::read(fixture.plan_path()).unwrap();
            let parked = fixture.temp.path().join("stopped-storage");
            fs::rename(&fixture.target.root, &parked).unwrap();
            let result = fixture.engine.activate_pending_disk_exit().unwrap();
            assert_eq!(result.operation_id, fixture.operation);
            assert_eq!(result.result["target_io"], "closed");
            assert_eq!(fs::read(fixture.plan_path()).unwrap(), plan);
            assert_eq!(files(&parked), before);
            fixture.assert_history();
            assert_eq!(fixture.engine.activate_pending_disk_exit().unwrap(), result);
        }
    }
}

#[test]
fn exit_stop_all_four_real_barriers_resume_cold_with_only_retained_new_quorum() {
    for fault in FAULTS {
        for (relocate, second_publication) in [(false, false), (true, false), (true, true)] {
            let mut fixture = Fixture::prepared(relocate, false);
            if second_publication {
                fixture.crash(WriteFault::AfterHead);
            }
            let before = files(&fixture.target.root);
            fixture.crash(fault);
            let system = fixture.system();
            let operation = fixture.operation;
            let expected_history = fixture.history.clone();
            let old_operation = fixture.old_operation;
            let parked = fixture.temp.path().join("stopped-storage");
            drop(fixture.engine);
            fs::rename(&fixture.target.root, &parked).unwrap();
            assert_eq!(
                Engine::open(&system).err().unwrap().code(),
                ErrorCode::RecoveryRequired
            );
            let engine = Engine::resume_membership_activation(&system).unwrap();
            assert_eq!(engine.state.operations[&operation].operation_id, operation);
            assert!(engine.config.excludes_io_disk(&fixture.target.disk_id));
            assert_eq!(
                canonical_json(&(
                    &engine.state.versions,
                    &engine.state.commits,
                    &engine.state.operations[&old_operation]
                ))
                .unwrap(),
                expected_history
            );
            assert_eq!(files(&parked), before);
            drop(engine);
            assert!(Engine::open(&system).is_ok());
        }
    }
}

#[test]
fn exit_stop_partial_plan_preserves_old_stops_across_restart() {
    for fault in FAULTS {
        let mut fixture = Fixture::prepared(false, true);
        let prior = fixture.prior_target.as_ref().unwrap().clone();
        let prior_bytes = files(&prior);
        let target_bytes = files(&fixture.target.root);
        fixture.crash(fault);
        let system = fixture.system();
        let operation = fixture.operation;
        let prior_parked = fixture.temp.path().join("old-stopped");
        let target_parked = fixture.temp.path().join("new-stopped");
        drop(fixture.engine);
        fs::rename(&prior, &prior_parked).unwrap();
        fs::rename(&fixture.target.root, &target_parked).unwrap();
        let engine = Engine::resume_membership_activation(&system).unwrap();
        assert!(engine.state.operations.contains_key(&operation));
        assert_eq!(files(&prior_parked), prior_bytes);
        assert_eq!(files(&target_parked), target_bytes);
        let store = open_store(&system).unwrap();
        assert_eq!(
            store
                .reload(&engine.config.pool_id)
                .unwrap()
                .stop_hints()
                .count(),
            2
        );
    }
}

#[test]
fn exit_stop_uncertain_publication_keeps_busy_owner_until_real_closure() {
    let mut fixture = Fixture::prepared(false, false);
    let target = Arc::clone(fixture.engine.disk_resources.get(&fixture.target).unwrap());
    let tail = target.root.admit_io().unwrap();
    fixture.crash(WriteFault::AfterRecord);
    assert!(target.root.admit_io().is_err());
    assert!(target.root.io_binding().unwrap().closed_proof().is_err());
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    drop(tail);
    fixture.engine.activate_pending_disk_exit().unwrap();
    target.root.io_binding().unwrap().closed_proof().unwrap();
}

#[test]
fn exit_stop_refuses_missing_plan_unknown_staging_and_unverified_requests() {
    let mut fixture = Fixture::prepared(false, false);
    let target = Arc::clone(fixture.engine.disk_resources.get(&fixture.target).unwrap());
    let identity = DiskRegistrationIdentity {
        pool_id: fixture.engine.config.pool_id.clone(),
        disk_id: fixture.target.disk_id.clone(),
        slot_id: fixture.target.slot_id.clone(),
        binding_generation: fixture.target.binding_generation,
    };
    assert!(
        fixture
            .engine
            .synchronize_disk_exit_stop(
                &identity,
                OperationId::generate().unwrap(),
                &"b".repeat(64)
            )
            .is_err()
    );
    assert!(
        target.root.admit_io().is_ok(),
        "unverified preparation must not close the gate"
    );
    fixture.crash(WriteFault::AfterRecord);
    let plan = fs::read(fixture.plan_path()).unwrap();
    fs::remove_file(fixture.plan_path()).unwrap();
    let system = fixture.system();
    let projected = fixture.engine.config.clone();
    assert!(prepare_resume(&system, &projected).is_err());
    fs::write(fixture.plan_path(), &plan).unwrap();
    fs::set_permissions(fixture.plan_path(), fs::Permissions::from_mode(0o600)).unwrap();
    let unknown = system.join("bootstrap-discovery").join("unknown-staging");
    fs::write(&unknown, b"do not delete or adopt").unwrap();
    assert!(prepare_resume(&system, &projected).is_err());
    assert_eq!(fs::read(&unknown).unwrap(), b"do not delete or adopt");
}

#[test]
fn exit_stop_rejects_changed_authority_actor_operation_and_retained_identity() {
    for change in ["actor", "operation", "authority", "root"] {
        let mut fixture = Fixture::prepared(false, false);
        fixture.crash(WriteFault::AfterRecord);
        let mut value: json_codec::Value =
            decode_strict(&fs::read(fixture.plan_path()).unwrap()).unwrap();
        match change {
            "actor" => {
                fn change_actor(value: &mut json_codec::Value) -> bool {
                    match value {
                        json_codec::Value::Object(map) => {
                            if let Some(actor) = map.get_mut("principal") {
                                *actor = "intruder".into();
                                return true;
                            }
                            map.values_mut().any(change_actor)
                        }
                        json_codec::Value::Array(values) => values.iter_mut().any(change_actor),
                        _ => false,
                    }
                }
                assert!(change_actor(&mut value["authority"]));
            }
            "operation" => {
                value["stop"]["operation_id"] = OperationId::generate().unwrap().to_string().into();
            }
            "authority" => {
                value["authority"] = json_codec::Value::Null;
            }
            "root" => {
                value["stop"]["stopped"]["roots"][1]["inode"] = "1".into();
            }
            _ => unreachable!(),
        }
        fs::write(fixture.plan_path(), canonical_json(&value).unwrap()).unwrap();
        let system = fixture.system();
        let projected = fixture.engine.config.clone();
        assert!(
            prepare_resume(&system, &projected).is_err(),
            "must reject {change}"
        );
    }
}

#[test]
fn exit_stop_saved_plan_cannot_replace_actual_retained_quorum_or_archive() {
    for missing in ["member", "archive"] {
        let mut fixture = Fixture::prepared(false, false);
        let generation = Arc::clone(
            &fixture
                .engine
                .pending_disk_exit
                .as_ref()
                .unwrap()
                .generation,
        );
        let retained = generation.intent.target.metadata_members().next().unwrap();
        assert_eq!(generation.intent.target.metadata_members().len(), 2);
        assert_eq!(generation.intent.target.metadata_quorum(), 2);
        let source = if missing == "member" {
            retained.root.clone()
        } else {
            let transition = object_hash(INTENT_KIND, &generation.intent).unwrap();
            member_directory(retained, &transition).join(CONTROL_ARCHIVE_DIRECTORY)
        };
        fixture.crash(WriteFault::AfterRecord);
        let system = fixture.system();
        let plan_path = fixture.plan_path();
        let plan = fs::read(&plan_path).unwrap();
        let operation = fixture.operation;
        let target_before = files(&fixture.target.root);
        drop(fixture.engine);
        let parked = source.with_extension("retained-for-retry");
        fs::rename(&source, &parked).unwrap();
        let before = files(fixture.temp.path());
        let error = Engine::resume_membership_activation(&system).err().unwrap();
        assert_eq!(
            error.code(),
            ErrorCode::MetadataQuorumUnavailable,
            "{missing}"
        );
        assert_eq!(files(fixture.temp.path()), before);
        assert_eq!(fs::read(&plan_path).unwrap(), plan);
        fs::rename(&parked, &source).unwrap();
        let engine = Engine::resume_membership_activation(&system).unwrap();
        assert!(engine.state.operations.contains_key(&operation));
        assert_eq!(files(&fixture.target.root), target_before);
        assert_eq!(fs::read(&plan_path).unwrap(), plan);
    }
}

#[test]
fn exit_stop_does_not_replace_a_changed_genesis_control_setting() {
    let mut fixture = Fixture::prepared(false, false);
    let path = fixture.engine.config.disks[1]
        .root
        .join("metadata/pool-membership/config.json");
    let original = fs::read(&path).unwrap();
    let mut changed: Config = decode_strict(&original).unwrap();
    changed.workspace_ttl_seconds += 1;
    let changed = canonical_json(&changed).unwrap();
    fs::write(&path, &changed).unwrap();
    let target = files(&fixture.target.root);
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
    assert_eq!(fs::read(&path).unwrap(), changed);
    assert_eq!(files(&fixture.target.root), target);
}
