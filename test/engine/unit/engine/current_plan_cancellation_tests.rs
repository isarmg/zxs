//! Actual native plans, D3 publication and permanent cancellation recovery.
//! These tests depend on local Ready plans staying in system/plans, not State.
use super::*;
use crate::publication_shape::{Table, unchanged_except};
use crate::*;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: op(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "plan cancellation source".into(),
        request_digest: None,
    }
}

fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine.commit_workspace(request(&workspace)).unwrap()
}

fn entry(engine: &Engine, branch: &str, path: &str) -> Entry {
    engine
        .namespace_tree(branch)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for child in fs::read_dir(directory).unwrap() {
            let path = child.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    old: Entry,
    current: Entry,
    source: OperationResult,
}

impl Fixture {
    fn new() -> Self {
        Self::with_retained_limit(None)
    }

    fn with_retained_limit(limit: Option<usize>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        if let Some(limit) = limit {
            config.max_retained_workspaces = limit;
            config.max_retained_workspaces_per_principal = limit;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("plan cancellation").unwrap(),
                "D04",
                1,
                "admin",
                op(),
            )
            .unwrap();
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &new_id("rule").unwrap(),
                repository_id: &repository.repository_id,
                path_prefix: "gone",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "gone",
            b"departed sole body",
        );
        let source = save(
            &mut engine,
            &repository.main_branch_id,
            "live",
            b"retained old body",
        );
        let old = entry(&engine, &repository.main_branch_id, "live");
        save(
            &mut engine,
            &repository.main_branch_id,
            "live",
            b"retained current body",
        );
        let current = entry(&engine, &repository.main_branch_id, "live");
        Self {
            temp,
            engine,
            config,
            repository,
            old,
            current,
            source,
        }
    }

    fn purge(&mut self, actor: &str) -> Plan {
        self.engine
            .create_purge_plan(
                &self.repository.history_tree_id,
                vec![self.old.version_id.clone()],
                actor,
            )
            .unwrap()
    }

    fn split(&mut self) -> Plan {
        self.engine
            .create_split_plan(
                &self.repository.history_tree_id,
                self.source.commit_id.as_deref().unwrap(),
                "admin",
            )
            .unwrap()
    }

    fn plan_path(&self, plan: &Plan) -> PathBuf {
        self.temp
            .path()
            .join("system/plans")
            .join(format!("{}.json", plan.plan_id))
    }

    fn exit(&mut self, target: usize, action: MembershipExitAction) {
        let disk = self.config.disks[target].clone();
        let resource = Arc::clone(self.engine.disk_resources.get(&disk).unwrap());
        self.engine
            .begin_disk_exit(&disk.disk_id, action, "admin", op())
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        assert!(
            self.engine
                .resume_membership_joint_decision()
                .unwrap()
                .joint_quorum_accepted()
        );
        let before = files(&disk.root);
        let result = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(result.result["target_io"], "closed");
        resource.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), before);
    }

    fn replace_stopped_root(&self, target: usize) -> (PathBuf, BTreeMap<PathBuf, Vec<u8>>) {
        let disk = &self.config.disks[target];
        let original = files(&disk.root);
        let saved = self.temp.path().join(format!("stopped-{target}"));
        fs::rename(&disk.root, &saved).unwrap();
        fs::write(&disk.root, b"stopped root is not a directory").unwrap();
        (saved, original)
    }
}

fn assert_same_scopes(before: &CurrentNamespaces, after: &CurrentNamespaces) {
    assert!(Arc::ptr_eq(&before.branches, &after.branches));
    assert!(Arc::ptr_eq(&before.directories, &after.directories));
    assert!(Arc::ptr_eq(&before.known_entries, &after.known_entries));
    assert!(Arc::ptr_eq(&before.known_files, &after.known_files));
    assert!(Arc::ptr_eq(
        &before.unavailable_versions,
        &after.unavailable_versions
    ));
    assert_eq!(before.registry, after.registry);
    assert_eq!(before.mask_hash, after.mask_hash);
}

#[test]
fn current_plan_cancellation_keeps_other_local_plans_and_cold_replays_permanently() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, stopped_bytes) = fixture.replace_stopped_root(0);
    let first = fixture.purge("admin");
    let second = fixture.split();
    let third = fixture.purge("other");
    let local = files(&fixture.temp.path().join("system/plans"));
    let before = fixture.engine.state.clone();
    assert!(
        [&first, &second, &third]
            .iter()
            .all(|plan| !before.plans.contains_key(&plan.plan_id))
    );
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let result = fixture
        .engine
        .cancel_plan(&first.plan_id, "admin", operation)
        .unwrap();
    let next = fixture.engine.state.clone();
    assert!(unchanged_except(&before, &next, &[Table::Plans]));
    assert_eq!(before.topology_generation, next.topology_generation);
    assert_eq!(before.control_auth, next.control_auth);
    assert_eq!(next.plans.len(), before.plans.len() + 1);
    assert_eq!(next.plans[&first.plan_id].state, PlanState::Cancelled);
    assert!(!next.plans.contains_key(&second.plan_id));
    assert!(!next.plans.contains_key(&third.plan_id));
    assert_eq!(fixture.engine.plan(&second.plan_id).unwrap(), second);
    assert_eq!(fixture.engine.plan(&third.plan_id).unwrap(), third);
    assert_eq!(next.file_names, before.file_names);
    assert_eq!(next.file_locations, before.file_locations);
    assert_eq!(next.commits, before.commits);
    assert_eq!(next.versions, before.versions);
    assert_eq!(next.placements, before.placements);
    assert_same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
    assert_eq!(
        result.request_digest,
        Engine::native_request_digest(KIND, "admin", &json!({"plan_id": first.plan_id})).unwrap()
    );
    assert_eq!(
        result.result["cancelled_plan_hash"],
        crate::object_hash("plan-cancellation", &next.plans[&first.plan_id]).unwrap()
    );
    assert_eq!(
        fixture
            .engine
            .cancel_plan(&first.plan_id, "admin", operation)
            .unwrap(),
        result
    );
    assert_eq!(
        fixture
            .engine
            .commit_plan(&first.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PlanClosed
    );
    assert_eq!(files(&fixture.temp.path().join("system/plans")), local);
    assert_eq!(files(&saved), stopped_bytes);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .cancel_plan(&first.plan_id, "admin", operation)
            .unwrap(),
        result
    );
    assert_eq!(
        engine.plan(&first.plan_id).unwrap().state,
        PlanState::Cancelled
    );
    assert_eq!(engine.plan(&second.plan_id).unwrap(), second);
    assert_eq!(engine.plan(&third.plan_id).unwrap(), third);
    assert_eq!(
        engine
            .commit_plan(&first.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PlanClosed
    );
    let second_operation = op();
    let second_result = engine
        .cancel_plan(&second.plan_id, "admin", second_operation)
        .unwrap();
    assert_eq!(engine.plan(&third.plan_id).unwrap(), third);
    assert_eq!(
        engine
            .cancel_plan(&third.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
    let third_operation = op();
    let third_result = engine
        .cancel_plan(&third.plan_id, "other", third_operation)
        .unwrap();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    for (plan, actor, operation, expected) in [
        (&first, "admin", operation, &result),
        (&second, "admin", second_operation, &second_result),
        (&third, "other", third_operation, &third_result),
    ] {
        assert_eq!(
            &engine.cancel_plan(&plan.plan_id, actor, operation).unwrap(),
            expected
        );
        assert_eq!(
            engine.plan(&plan.plan_id).unwrap().state,
            PlanState::Cancelled
        );
    }
    assert!(
        engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("gone")
            .is_none()
    );
    assert_eq!(files(&system.join("plans")), local);
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root is not a directory"
    );
    assert_eq!(files(&saved), stopped_bytes);
}

#[test]
fn current_plan_cancellation_on_disabled_and_detached_owner_never_reopens_storage() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        let plan = fixture.purge("admin");
        fixture.exit(3, action);
        let (saved, stopped_bytes) = fixture.replace_stopped_root(3);
        let before = fixture.engine.state.clone();
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let operation = op();
        let result = fixture
            .engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        assert!(unchanged_except(
            &before,
            fixture.engine.state(),
            &[Table::Plans],
        ));
        assert_eq!(
            before.topology_generation,
            fixture.engine.state().topology_generation
        );
        assert_eq!(before.control_auth, fixture.engine.state().control_auth);
        assert_same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
        assert_eq!(
            fixture
                .engine
                .create_workspace(&fixture.repository.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
        assert!(
            fixture
                .engine
                .open_content(&fixture.old.version_id)
                .is_err()
        );
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .cancel_plan(&plan.plan_id, "admin", operation)
                .unwrap(),
            result
        );
        assert_eq!(
            engine
                .create_workspace(&fixture.repository.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
        assert!(engine.open_content(&fixture.old.version_id).is_err());
        assert_eq!(
            fs::read(&fixture.config.disks[3].root).unwrap(),
            b"stopped root is not a directory"
        );
        assert_eq!(files(&saved), stopped_bytes);
    }
}

#[test]
fn current_plan_cancellation_busy_keeps_preparation_and_read_tails_owned() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, stopped_bytes) = fixture.replace_stopped_root(0);
    let plan = fixture.purge("admin");
    let other = fixture.split();
    let old_operation = op();
    let old_result = fixture
        .engine
        .cancel_plan(&other.plan_id, "admin", old_operation)
        .unwrap();
    let mut read = fixture
        .engine
        .open_content(&fixture.current.version_id)
        .unwrap();
    let counts = fixture
        .engine
        .reads
        .lock()
        .unwrap()
        .counts()
        .unwrap()
        .clone();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "live", b"prepared newer body")
        .unwrap();
    let preparation = fixture
        .engine
        .prepare_workspace_commit(request(&workspace), None)
        .unwrap();
    let state = fixture.engine.state.clone();
    let plan_bytes = fs::read(fixture.plan_path(&plan)).unwrap();
    let operation = op();
    for _ in 0..2 {
        assert_eq!(
            fixture
                .engine
                .cancel_plan(&plan.plan_id, "admin", operation)
                .unwrap_err()
                .code(),
            ErrorCode::CommitPreparationBusy
        );
        assert!(Arc::ptr_eq(&state, &fixture.engine.state));
        assert!(!fixture.engine.state.operations.contains_key(&operation));
        assert_eq!(fixture.engine.plan(&plan.plan_id).unwrap(), plan);
        assert_eq!(fs::read(fixture.plan_path(&plan)).unwrap(), plan_bytes);
        assert_eq!(
            fs::read(&staged.source_path).unwrap(),
            b"prepared newer body"
        );
        assert_eq!(
            *fixture.engine.reads.lock().unwrap().counts().unwrap(),
            counts
        );
        assert_eq!(
            fixture
                .engine
                .create_workspace(&fixture.repository.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::CommitPreparationBusy
        );
    }
    assert_eq!(
        fixture
            .engine
            .cancel_plan(&other.plan_id, "admin", old_operation)
            .unwrap(),
        old_result
    );
    read.file.check_running().unwrap();
    let mut bytes = Vec::new();
    read.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"retained current body");
    let prepared = preparation.execute().unwrap();
    fixture.engine.finish_workspace_commit(prepared).unwrap();
    assert_eq!(
        *fixture.engine.reads.lock().unwrap().counts().unwrap(),
        counts
    );
    fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(files(&saved), stopped_bytes);
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root is not a directory"
    );
    drop(read);
    assert!(
        fixture
            .engine
            .reads
            .lock()
            .unwrap()
            .counts()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn current_plan_cancellation_keeps_native_actor_id_digest_and_closed_state_rejections() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let plan = fixture.purge("admin");
    let foreign = fixture.purge("other");
    let before = canonical_json(fixture.engine.state()).unwrap();
    let local = files(&fixture.temp.path().join("system/plans"));
    for (id, actor, operation, code) in [
        (plan.plan_id.clone(), "", op(), ErrorCode::Unauthenticated),
        (
            plan.plan_id.clone(),
            "other",
            op(),
            ErrorCode::PermissionDenied,
        ),
        (
            foreign.plan_id.clone(),
            "admin",
            op(),
            ErrorCode::PermissionDenied,
        ),
        (
            new_id("plan").unwrap(),
            "admin",
            op(),
            ErrorCode::PlanNotFound,
        ),
        ("../invalid".into(), "admin", op(), ErrorCode::InvalidId),
        (
            plan.plan_id.clone(),
            "admin",
            fixture.source.operation_id,
            ErrorCode::IdempotencyKeyReused,
        ),
    ] {
        assert_eq!(
            fixture
                .engine
                .cancel_plan(&id, actor, operation)
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        assert_eq!(files(&fixture.temp.path().join("system/plans")), local);
    }
    let path = fixture.plan_path(&plan);
    for case in 0..6 {
        let mut changed = plan.clone();
        let expected = match case {
            0 => {
                changed.state = PlanState::Committed;
                ErrorCode::PlanClosed
            }
            1 => {
                changed.state = PlanState::Completed;
                ErrorCode::PlanClosed
            }
            2 => {
                changed.state = PlanState::Cancelled;
                ErrorCode::PlanClosed
            }
            3 => {
                changed.digest = "f".repeat(64);
                ErrorCode::PlanInvalid
            }
            4 => {
                changed.plan_id = new_id("plan").unwrap();
                ErrorCode::PlanInvalid
            }
            5 => {
                changed.input["history_tree_id"] = json!("modified local input");
                ErrorCode::PlanInvalid
            }
            _ => unreachable!(),
        };
        crate::disk::write_json(&path, &changed, true).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            fixture
                .engine
                .cancel_plan(&plan.plan_id, "admin", op())
                .unwrap_err()
                .code(),
            expected
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
    crate::disk::write_json(&path, &plan, true).unwrap();
    let operation = op();
    let result = fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    for (id, actor) in [(&plan.plan_id, "other"), (&foreign.plan_id, "admin")] {
        assert_eq!(
            fixture
                .engine
                .cancel_plan(id, actor, operation)
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
    }
    assert_eq!(
        fixture
            .engine
            .cancel_plan(&plan.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PlanClosed
    );
    let frozen = fixture.engine.state.clone();
    for change in 0..2 {
        let receipt = Arc::make_mut(&mut fixture.engine.state)
            .operations
            .get_mut(&operation)
            .unwrap();
        if change == 0 {
            receipt.request_digest = "e".repeat(64);
        } else {
            receipt.operation_id = op();
        }
        assert_eq!(
            fixture
                .engine
                .cancel_plan(&plan.plan_id, "admin", operation)
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
        fixture.engine.state = Arc::clone(&frozen);
    }
    assert_eq!(
        fixture
            .engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        result
    );
}

#[test]
fn current_plan_cancellation_fast_path_and_cold_replay_reject_closed_write_set_forgery() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let first = fixture.purge("admin");
    let first_operation = op();
    fixture
        .engine
        .cancel_plan(&first.plan_id, "admin", first_operation)
        .unwrap();
    let plan = fixture.split();
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    let next = fixture.engine.state.clone();
    assert!(next.branches.changes_since(&previous.branches).is_empty());
    assert!(
        next.directory_branches
            .changes_since(&previous.directory_branches)
            .is_empty()
    );
    assert_same_scopes(&view, &view.advance(&previous, &next).unwrap());
    let cold_previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    let cold_next: State = json_codec::from_slice(&canonical_json(next.as_ref()).unwrap()).unwrap();
    assert_same_scopes(&view, &view.advance(&cold_previous, &cold_next).unwrap());
    for attack in 0..42 {
        let mut forged = next.as_ref().clone();
        match attack {
            0 => {
                forged
                    .operation_principals
                    .insert(operation, "other".into());
            }
            1 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .request_digest = "a".repeat(64);
            }
            2 => {
                forged.operations.get_mut(&operation).unwrap().operation_id = op();
            }
            3 => {
                forged.operations.get_mut(&operation).unwrap().kind = "auth.fake".into();
            }
            4 => {
                forged.operations.remove(&operation);
            }
            5 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().state = PlanState::Ready;
            }
            6 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().state = PlanState::Completed;
            }
            7 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().plan_id = first.plan_id.clone();
            }
            8 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().principal = "other".into();
            }
            9 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().input["cut_commit_id"] =
                    json!("changed");
            }
            10 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().digest = "b".repeat(64);
            }
            11 => {
                forged.plans.get_mut(&plan.plan_id).unwrap().kind = "purge".into();
            }
            12 => {
                forged
                    .plans
                    .get_mut(&plan.plan_id)
                    .unwrap()
                    .expected_sequence += 1;
            }
            13 => {
                forged.plans.remove(&first.plan_id);
            }
            14 => {
                forged.plans.get_mut(&first.plan_id).unwrap().state = PlanState::Ready;
            }
            15 => {
                let mut extra = plan.clone();
                extra.plan_id = new_id("plan").unwrap();
                forged.plans.insert(extra.plan_id.clone(), extra);
            }
            16 => {
                forged.plans.remove(&plan.plan_id);
            }
            17 => {
                forged
                    .operations
                    .get_mut(&first_operation)
                    .unwrap()
                    .request_digest = "c".repeat(64);
            }
            18 => {
                forged
                    .operation_principals
                    .insert(first_operation, "other".into());
            }
            19 => {
                forged.operations.get_mut(&operation).unwrap().result["state"] = json!("ready");
            }
            20 => {
                forged.operations.get_mut(&operation).unwrap().result["unexpected"] = json!(true);
            }
            21 => {
                forged.operations.get_mut(&operation).unwrap().result["cancelled_plan_hash"] =
                    json!("d".repeat(64));
            }
            22 => {
                forged.operations.get_mut(&operation).unwrap().result["timestamp"] = json!(1);
            }
            23 => {
                forged.operations.get_mut(&operation).unwrap().result["membership_epoch"] =
                    Value::Null;
            }
            24 => {
                forged.operations.get_mut(&operation).unwrap().commit_id =
                    fixture.source.commit_id.clone();
            }
            25 => {
                forged.operations.get_mut(&operation).unwrap().branch_id =
                    Some(fixture.repository.main_branch_id.clone());
            }
            26 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .history_tree_id = Some(fixture.repository.history_tree_id.clone());
            }
            27 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .protection_at_publish = ProtectionAtPublish::TargetMet;
            }
            28 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .current_health = CurrentHealth::Degraded;
            }
            29 => {
                forged.operations.get_mut(&operation).unwrap().sequence += 1;
            }
            30 => {
                forged.business_generation += 1;
            }
            31 => {
                forged.topology_generation += 1;
            }
            32 => {
                forged.control_auth = Some(json!({"unrequested": true}));
            }
            33 => {
                forged.events.last_mut().unwrap().object_id = plan.plan_id.clone();
            }
            34 => {
                forged.events[0].kind = "rewritten audit".into();
            }
            35 => {
                forged
                    .branches
                    .get_mut(&fixture.repository.main_branch_id)
                    .unwrap()
                    .name = "renamed".into();
            }
            36 => {
                forged
                    .commits
                    .get_mut(fixture.source.commit_id.as_ref().unwrap())
                    .unwrap()
                    .message = "changed history".into();
            }
            37 => {
                forged
                    .file_names
                    .insert(fixture.current.file_id.clone(), "changed-container".into());
            }
            38 => {
                forged.file_locations.insert(
                    fixture.current.file_id.clone(),
                    "changed/physical/path".into(),
                );
            }
            39 => {
                forged
                    .placements
                    .get_mut(&fixture.old.version_id)
                    .unwrap()
                    .primary_slot_id = "P01".into();
            }
            40 => {
                forged
                    .content_states
                    .insert(fixture.old.version_id.clone(), ContentState::Purged);
            }
            41 => {
                forged.operations.get_mut(&operation).unwrap().commit_state = CommitState::InDoubt;
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &forged).is_err(),
            "tracked attack {attack}"
        );
        let restored: State = json_codec::from_slice(&canonical_json(&forged).unwrap()).unwrap();
        assert!(
            view.advance(&cold_previous, &restored).is_err(),
            "cold attack {attack}"
        );
    }
}

#[test]
fn current_plan_cancellation_never_changes_an_existing_published_plan_source() {
    let mut fixture = Fixture::new();
    let plan = fixture.purge("admin");
    let before: TrackedMap<Plan> = [(plan.plan_id.clone(), plan.clone())].into();
    let mut cancelled = plan.clone();
    cancelled.state = PlanState::Cancelled;
    verify_cancelled_plan(&before, &plan.plan_id, "admin", &cancelled).unwrap();
    for attack in 0..5 {
        let mut changed = cancelled.clone();
        match attack {
            0 => {
                changed.kind = "split".into();
            }
            1 => {
                changed.expected_sequence += 1;
            }
            2 => {
                changed.input = json!({"different": true});
                changed.digest = crate::object_hash_value("plan", &changed.input).unwrap();
            }
            3 => {
                changed.principal = "other".into();
            }
            4 => {
                changed.plan_id = new_id("plan").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            verify_cancelled_plan(&before, &plan.plan_id, &changed.principal, &changed).is_err()
        );
    }
    for state in [
        PlanState::Cancelled,
        PlanState::Committed,
        PlanState::Completed,
    ] {
        let mut closed = plan.clone();
        closed.state = state;
        let closed: TrackedMap<Plan> = [(plan.plan_id.clone(), closed)].into();
        assert!(verify_cancelled_plan(&closed, &plan.plan_id, "admin", &cancelled).is_err());
    }
}

#[test]
fn current_plan_cancellation_old_receipts_survive_registry_without_a_new_publication() {
    let mut fixture = Fixture::new();
    let plan = fixture.purge("admin");
    let operation = op();
    let original = fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, stopped_bytes) = fixture.replace_stopped_root(0);
    let state = fixture.engine.state.clone();
    let local = fs::read(fixture.plan_path(&plan)).unwrap();
    assert_eq!(
        fixture
            .engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        original
    );
    assert!(Arc::ptr_eq(&state, &fixture.engine.state));
    assert_eq!(fs::read(fixture.plan_path(&plan)).unwrap(), local);
    assert_eq!(files(&saved), stopped_bytes);
}

#[test]
fn current_plan_cancellation_keeps_historical_workspace_and_staged_source() {
    let mut fixture = Fixture::new();
    let source = fixture.source.commit_id.clone().unwrap();
    let workspace = fixture
        .engine
        .create_historical_workspace(&fixture.repository.main_branch_id, &source, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(
            &workspace.workspace_id,
            "live",
            b"retained historical staging",
        )
        .unwrap();
    let plan = fixture
        .engine
        .create_historical_save_plan(
            &fixture.repository.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source,
                name: "cancelled historical preparation".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let sealed = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(sealed.state, WorkspaceState::Sealed);
    let sealed_bytes = canonical_json(&sealed).unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, stopped_bytes) = fixture.replace_stopped_root(0);
    let operation = op();
    let result = fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(
        canonical_json(&fixture.engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        sealed_bytes
    );
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"retained historical staging"
    );
    assert_eq!(
        fixture
            .engine
            .commit_historical_save_plan(&plan.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PlanClosed
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        result
    );
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        sealed_bytes
    );
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"retained historical staging"
    );
    assert_eq!(
        engine
            .commit_plan(&plan.plan_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::PlanClosed
    );
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root is not a directory"
    );
    assert_eq!(files(&saved), stopped_bytes);
}

#[test]
fn current_plan_cancellation_existing_receipt_owner_and_tombstone_are_immutable_without_new_receipt()
 {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let plan = fixture.purge("admin");
    let operation = op();
    fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let cold_previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    for attack in 0..3 {
        let mut next = previous.as_ref().clone();
        next.begin_tracking();
        match attack {
            0 => {
                next.operation_principals.insert(operation, "other".into());
            }
            1 => {
                next.plans.get_mut(&plan.plan_id).unwrap().state = PlanState::Ready;
            }
            2 => {
                next.operations.remove(&operation);
            }
            _ => unreachable!(),
        }
        assert!(view.advance(&previous, &next).is_err());
        let cold_next: State = json_codec::from_slice(&canonical_json(&next).unwrap()).unwrap();
        assert!(view.advance(&cold_previous, &cold_next).is_err());
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn current_plan_cancellation_failure_keeps_local_evidence_and_recovers_a_decided_tombstone() {
    for fault in ["after_payloads", "after_finalize"] {
        let mut fixture = Fixture::new();
        fixture.exit(0, MembershipExitAction::Deregister);
        let (saved, stopped_bytes) = fixture.replace_stopped_root(0);
        let plan = fixture.purge("admin");
        let other = fixture.split();
        let local = files(&fixture.temp.path().join("system/plans"));
        let state = fixture.engine.state.clone();
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let read = fixture
            .engine
            .open_content(&fixture.current.version_id)
            .unwrap();
        let counts = fixture
            .engine
            .reads
            .lock()
            .unwrap()
            .counts()
            .unwrap()
            .clone();
        let operation = op();
        fixture.engine.inject_fault_once(fault);
        let error = fixture
            .engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap_err();
        assert!(Arc::ptr_eq(&state, &fixture.engine.state), "{fault}");
        assert!(
            Arc::ptr_eq(&view, fixture.engine.current_namespaces.as_ref().unwrap()),
            "{fault}"
        );
        assert_eq!(fixture.engine.plan(&plan.plan_id).unwrap(), plan);
        assert_eq!(fixture.engine.plan(&other.plan_id).unwrap(), other);
        assert_eq!(files(&fixture.temp.path().join("system/plans")), local);
        assert_eq!(
            *fixture.engine.reads.lock().unwrap().counts().unwrap(),
            counts
        );
        read.file.check_running().unwrap();
        if fault == "after_finalize" {
            assert_eq!(error.code(), ErrorCode::InDoubt);
            assert_eq!(
                fixture
                    .engine
                    .operation_record(operation)
                    .unwrap()
                    .result()
                    .commit_state,
                CommitState::InDoubt
            );
            assert_eq!(
                fixture
                    .engine
                    .cancel_plan(&plan.plan_id, "admin", operation)
                    .unwrap_err()
                    .code(),
                ErrorCode::RecoveryRequired
            );
        }
        assert_eq!(files(&saved), stopped_bytes);
        drop(read);
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        if fault == "after_finalize" {
            assert_eq!(
                engine.plan(&plan.plan_id).unwrap().state,
                PlanState::Cancelled
            );
        } else {
            assert_eq!(engine.plan(&plan.plan_id).unwrap(), plan);
        }
        let result = engine
            .cancel_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        assert_eq!(result.operation_id, operation);
        assert_eq!(result.commit_state, CommitState::Published);
        assert_eq!(
            result.request_digest,
            Engine::native_request_digest(KIND, "admin", &json!({"plan_id": plan.plan_id}))
                .unwrap()
        );
        assert_eq!(
            engine.plan(&plan.plan_id).unwrap().state,
            PlanState::Cancelled
        );
        assert_eq!(engine.plan(&other.plan_id).unwrap(), other);
        assert_eq!(files(&system.join("plans")), local);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"stopped root is not a directory"
        );
        assert_eq!(files(&saved), stopped_bytes);
    }
}

#[test]
fn current_plan_cancellation_requires_closed_proof_before_candidate_leaves_planning() {
    use crate::engine::MutationPlan;

    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let plan = fixture.purge("admin");
    let view = fixture.engine.current_namespaces.clone().unwrap();
    assert_eq!(
        view.require_operation(KIND).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    let previous = Arc::clone(&fixture.engine.state);
    let operation = op();
    let receipt = fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    let published = Arc::clone(&fixture.engine.state);
    let published_bytes = canonical_json(published.as_ref()).unwrap();
    let local_plans = files(&fixture.temp.path().join("system/plans"));
    let mut context = fixture.engine.planning_context();
    context.state = &previous;
    context.next_slot = published.sequence;

    let generic = context.plan_new_mutation(
        KIND,
        "admin",
        operation,
        receipt.request_digest.clone(),
        |_, _, _| panic!("generic kind admission must reject before preparing a cancellation"),
    );
    let Err(error) = generic else {
        panic!("plan cancellation must not become a generic allowed kind");
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        view.verify_new_plan_cancellation(&previous, &previous, &receipt)
            .is_err()
    );

    for attack in 0..8 {
        let digest = if attack == 1 {
            "f".repeat(64)
        } else {
            receipt.request_digest.clone()
        };
        let candidate =
            context.plan_cancellation_mutation("admin", operation, digest, |_, next, prepared| {
                if attack != 5 {
                    next.plans
                        .insert(plan.plan_id.clone(), published.plans[&plan.plan_id].clone());
                }
                let mut result = receipt.result.clone();
                if attack == 2 {
                    result["cancelled_plan_hash"] = json!("a".repeat(64));
                }
                if attack == 3 {
                    next.file_notes
                        .insert(fixture.current.file_id.clone(), "unrequested".into());
                }
                if attack == 4 {
                    prepared.push(crate::engine::Prepared {
                        version: previous.versions[&fixture.current.version_id].clone(),
                        placement: previous.placements[&fixture.current.version_id].clone(),
                        source: crate::engine::ReadSource::external(
                            fixture.temp.path().join("must-not-open-payload"),
                        ),
                    });
                }
                if attack == 6 {
                    result["state"] = json!("ready");
                }
                if attack == 7 {
                    next.business_generation += 1;
                }
                Ok(result)
            });
        if attack == 0 {
            let Ok(MutationPlan::Prepared {
                next,
                prepared,
                result,
            }) = candidate
            else {
                panic!("exact metadata-only cancellation must be admitted");
            };
            assert!(prepared.is_empty());
            view.verify_new_plan_cancellation(&previous, &next, &result)
                .unwrap();
            assert_same_scopes(&view, &view.advance(&previous, &next).unwrap());
            let mut different_result = result.clone();
            different_result.request_digest = "b".repeat(64);
            assert!(
                view.verify_new_plan_cancellation(&previous, &next, &different_result)
                    .is_err()
            );
        } else {
            let Err(error) = candidate else {
                panic!("unproven cancellation candidate {attack} left planning");
            };
            assert_eq!(
                error.code(),
                ErrorCode::PublicationInvalid,
                "attack {attack}"
            );
        }
    }

    context.current_namespaces = None;
    let pending = context.plan_cancellation_mutation(
        "admin",
        operation,
        receipt.request_digest,
        |_, _, _| panic!("missing registry admission must reject before preparation"),
    );
    let Err(error) = pending else {
        panic!("pending registry must not admit a cancellation");
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(Arc::ptr_eq(&fixture.engine.state, &published));
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        published_bytes
    );
    assert_eq!(
        files(&fixture.temp.path().join("system/plans")),
        local_plans
    );
}

#[test]
fn current_plan_cancellation_cannot_hide_its_transition_behind_another_kind_and_state() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let plan = fixture.purge("admin");
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    fixture
        .engine
        .cancel_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    let published = Arc::clone(&fixture.engine.state);
    let cold_previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    for state in [PlanState::Ready, PlanState::Committed, PlanState::Completed] {
        let mut forged = published.as_ref().clone();
        forged.plans.get_mut(&plan.plan_id).unwrap().state = state;
        let record = forged.operations.get_mut(&operation).unwrap();
        record.kind = "auth.unrelated".into();
        record.request_digest = Engine::native_request_digest(
            "auth.unrelated",
            "admin",
            &json!({"plan_id": plan.plan_id}),
        )
        .unwrap();
        record.result = json!({"unrelated": true});
        forged.events.last_mut().unwrap().kind = "auth.unrelated".into();
        assert_eq!(
            view.advance(&previous, &forged).err().unwrap().code(),
            ErrorCode::PublicationInvalid
        );
        let cold_next: State = json_codec::from_slice(&canonical_json(&forged).unwrap()).unwrap();
        assert_eq!(
            view.advance(&cold_previous, &cold_next)
                .err()
                .unwrap()
                .code(),
            ErrorCode::PublicationInvalid
        );
    }
    assert!(Arc::ptr_eq(&fixture.engine.state, &published));
}

// Real hash-bound journal decisions bypass live planning only in this fixture.
// Fixed membership, snapshot/history closure and receipts stay authenticated so
// cold recovery must reach the operation-specific proof before repairing them.
fn install_cancellation_decision(
    engine: &mut Engine,
    next: &State,
    result: &OperationResult,
    published: bool,
    versions: Vec<crate::journal::VersionEvidence>,
) -> crate::journal::Candidate {
    use crate::journal::*;

    let members = engine.members();
    let snapshot = prepare_snapshot(next, crate::engine::MAX_METADATA).unwrap();
    snapshot
        .store_new_objects_at(&engine.disk_resources, &members)
        .unwrap();
    let catalog = crate::history_package::store_history_manifests_bounded_at(
        &engine.disk_resources,
        &members,
        next,
        crate::engine::MAX_METADATA,
    )
    .unwrap();
    let generation = engine.config.generation().unwrap();
    let candidate = Candidate::new_with_registry(
        CandidateData {
            pool_id: engine.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: new_id("tx").unwrap(),
            operation_id: result.operation_id,
            request_digest: result.request_digest.clone(),
            previous_snapshot: engine.snapshot.clone(),
            snapshot_hash: snapshot.hash.clone(),
            versions,
            metadata_disk_ids: members.iter().map(|disk| disk.disk_id.clone()).collect(),
            history_roots: catalog.reference().clone(),
        },
        engine.config.generation_hash().unwrap(),
        generation.portable.exit_registry().unwrap(),
    );
    engine.verify_candidate_membership(&candidate).unwrap();
    let hash = object_hash("candidate", &candidate).unwrap();
    for disk in &members {
        store_object_at(
            &engine.disk_resources.get(disk).unwrap().root,
            disk,
            "candidate",
            &candidate,
        )
        .unwrap();
        engine
            .append_journal(
                disk,
                RecordType::Prepare,
                next.sequence,
                &hash,
                &candidate.snapshot_hash,
            )
            .unwrap();
    }
    let accepts = members
        .iter()
        .map(|disk| {
            engine
                .append_journal(disk, RecordType::Accept, next.sequence, &hash, &hash)
                .unwrap()
        })
        .collect();
    if published {
        let finalize = Finalize {
            candidate_hash: hash.clone(),
            accepts,
            recovery_id: None,
        };
        let finalize_hash = object_hash("finalize", &finalize).unwrap();
        for disk in &members {
            store_object_at(
                &engine.disk_resources.get(disk).unwrap().root,
                disk,
                "finalize",
                &finalize,
            )
            .unwrap();
        }
        let finalize_receipts = members
            .iter()
            .map(|disk| {
                engine
                    .append_journal(
                        disk,
                        RecordType::Finalize,
                        next.sequence,
                        &hash,
                        &finalize_hash,
                    )
                    .unwrap()
            })
            .collect();
        let publication = Publication {
            candidate_hash: hash.clone(),
            finalize,
            finalize_receipts,
            payload_receipts: Vec::new(),
            result: result.clone(),
            recovery: None,
        };
        let publication_hash = object_hash("publication", &publication).unwrap();
        for disk in &members {
            store_object_at(
                &engine.disk_resources.get(disk).unwrap().root,
                disk,
                "publication",
                &publication,
            )
            .unwrap();
        }
        let receipts = members
            .iter()
            .map(|disk| {
                engine
                    .append_journal(
                        disk,
                        RecordType::Publication,
                        next.sequence,
                        &hash,
                        &publication_hash,
                    )
                    .unwrap()
            })
            .collect();
        let seal = PublicationSeal {
            candidate_hash: hash,
            publication_hash,
            receipts,
        };
        verify_publication_seal(&candidate, &publication, &seal).unwrap();
        for disk in &members {
            engine
                .disk_resources
                .get(disk)
                .unwrap()
                .root
                .write_json(
                    &publication_seal_path(disk, &seal.candidate_hash),
                    &seal,
                    false,
                )
                .unwrap();
        }
    }
    candidate
}

#[test]
fn current_plan_cancellation_cold_proof_precedes_candidate_repairs_and_name_changes() {
    use crate::engine::MutationPlan;
    use crate::journal::object_path;

    for published in [false, true] {
        for attack in [
            "valid",
            "actor",
            "digest",
            "prior-owner",
            "location",
            "payload",
            "payload-kind",
            "payload-marker",
            "payload-noop",
            "payload-orphan-auth",
            "payload-orphan-workspace",
        ] {
            let mut fixture = Fixture::new();
            fixture.exit(0, MembershipExitAction::Deregister);
            let (parked, stopped_bytes) = fixture.replace_stopped_root(0);
            let prior = fixture.purge("admin");
            let prior_operation = op();
            fixture
                .engine
                .cancel_plan(&prior.plan_id, "admin", prior_operation)
                .unwrap();
            let plan = fixture.purge("admin");
            let operation = op();
            let mut cancelled = plan.clone();
            cancelled.state = PlanState::Cancelled;
            let cancelled_hash = object_hash("plan-cancellation", &cancelled).unwrap();
            let digest =
                Engine::native_request_digest(KIND, "admin", &json!({"plan_id": plan.plan_id}))
                    .unwrap();
            let prepared = fixture
                .engine
                .planning_context()
                .plan_cancellation_mutation("admin", operation, digest, |_, next, _| {
                    next.plans.insert(plan.plan_id.clone(), cancelled);
                    Ok(json!({
                        "plan_id": plan.plan_id,
                        "state": "cancelled",
                        "cancelled_plan_hash": cancelled_hash,
                    }))
                })
                .unwrap();
            let MutationPlan::Prepared {
                mut next,
                prepared,
                mut result,
            } = prepared
            else {
                panic!("fresh cancellation must prepare a native candidate")
            };
            assert!(prepared.is_empty());
            match attack {
                "valid" | "payload" => (),
                "payload-kind"
                | "payload-marker"
                | "payload-noop"
                | "payload-orphan-auth"
                | "payload-orphan-workspace" => {
                    result.kind = if attack == "payload-orphan-workspace" {
                        "workspace.commit"
                    } else {
                        "auth.unrelated"
                    }
                    .into();
                    next.events.last_mut().unwrap().kind = result.kind.clone();
                    if attack == "payload-marker" {
                        // Removing the Cancelled marker must not hide the real
                        // unauthorized Plan write from the pure State proof.
                        next.plans.get_mut(&plan.plan_id).unwrap().state = PlanState::Ready;
                        result.result["state"] = json!("ready");
                    }
                    if attack == "payload-noop" || attack.starts_with("payload-orphan") {
                        // Erase the entire business mutation and every marker.
                        // State-only advance legitimately preserves this view;
                        // only an exact payload delta can reject the old vector.
                        next.plans.remove(&plan.plan_id);
                        result.result = json!({
                            "timestamp": result.result["timestamp"],
                            "membership_epoch": result.result["membership_epoch"],
                            "version_protection": [],
                        });
                    }
                    next.operations.insert(operation, result.clone());
                }
                "actor" => {
                    next.operation_principals.insert(operation, "other".into());
                }
                "digest" => {
                    result.request_digest = "a".repeat(64);
                    next.operations.insert(operation, result.clone());
                }
                "prior-owner" => {
                    next.operation_principals
                        .insert(prior_operation, "other".into());
                }
                "location" => {
                    // Keep the structural basename relation valid so only the
                    // cancellation's closed write-set proof rejects the move.
                    let name = next.file_names[&fixture.current.file_id].clone();
                    next.file_locations.insert(
                        fixture.current.file_id.clone(),
                        format!("unrequested/{name}"),
                    );
                }
                _ => unreachable!(),
            }
            // Keep the original State, request and exact receipt valid, but
            // independently smuggle an existing live data-only payload into the
            // Candidate that drives repair. No planner may produce this pair.
            let versions = if attack.starts_with("payload-orphan") {
                let original = next.versions[&fixture.current.version_id].clone();
                let mut version = original.clone();
                version.version_id = new_id("ver").unwrap();
                version.version_sequence = next.sequence;
                let mut placement = next.placements[&fixture.current.version_id].clone();
                placement.version_id = version.version_id.clone();
                next.versions
                    .insert(version.version_id.clone(), version.clone());
                next.placements
                    .insert(version.version_id.clone(), placement.clone());
                next.content_states.insert(
                    version.version_id.clone(),
                    storage_contract::ContentState::Available,
                );
                let disk = fixture.engine.disk(&placement.route_slot_ids[0]).unwrap();
                let old =
                    crate::disk::version_dir(disk, &original.history_tree_id, &original.version_id);
                let orphan =
                    crate::disk::version_dir(disk, &version.history_tree_id, &version.version_id);
                fs::create_dir_all(&orphan).unwrap();
                for name in ["commit-proof.json", "publication-proof.json"] {
                    fs::copy(old.join(name), orphan.join(name)).unwrap();
                }
                vec![crate::journal::VersionEvidence { version, placement }]
            } else if attack.starts_with("payload") {
                vec![crate::journal::VersionEvidence {
                    version: next.versions[&fixture.current.version_id].clone(),
                    placement: next.placements[&fixture.current.version_id].clone(),
                }]
            } else {
                Vec::new()
            };
            if attack == "payload-noop" || attack.starts_with("payload-orphan") {
                fixture
                    .engine
                    .current_namespaces
                    .as_ref()
                    .unwrap()
                    .advance(&fixture.engine.state, &next)
                    .unwrap();
            }
            let candidate = install_cancellation_decision(
                &mut fixture.engine,
                &next,
                &result,
                published,
                versions,
            );
            let payload_path = candidate.versions.first().map(|evidence| {
                let disk = fixture
                    .engine
                    .disk(&evidence.placement.route_slot_ids[0])
                    .unwrap();
                assert!(
                    !disk.metadata_member,
                    "the attack must target a live data-only root"
                );
                crate::disk::version_dir(
                    disk,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                )
            });
            let members = fixture.engine.members();
            if !published && attack == "payload-noop" {
                let before = members
                    .iter()
                    .map(|disk| files(&disk.root))
                    .collect::<Vec<_>>();
                let before_system = files(&fixture.temp.path().join("system"));
                let watch = watch_candidate_payload(payload_path.as_ref().unwrap());
                let error = fixture
                    .engine
                    .prepare_degraded_recovery(
                        operation,
                        "admin",
                        "a forged receipt cannot authorize payload observation",
                    )
                    .unwrap_err();
                assert_eq!(error.code(), ErrorCode::PublicationInvalid);
                assert!(
                    error
                        .to_string()
                        .contains("exact fresh version and placement delta")
                );
                assert_no_candidate_payload_io(watch);
                for (disk, bytes) in members.iter().zip(before) {
                    assert_eq!(files(&disk.root), bytes);
                }
                assert_eq!(files(&fixture.temp.path().join("system")), before_system);
            }
            let missing = object_path(&members[0], "snapshot", &candidate.snapshot_hash);
            let snapshot_bytes = fs::read(&missing).unwrap();
            let system = fixture.temp.path().join("system");
            drop(fixture.engine);
            // A later proof would reject too late: this unique snapshot replica
            // would already be restored and name projection could have run.
            fs::remove_file(&missing).unwrap();
            let before_members = members
                .iter()
                .map(|disk| files(&disk.root))
                .collect::<Vec<_>>();
            let before_data = files(&fixture.config.disks[3].root);
            let before_plans = files(&system.join("plans"));
            // Directory watches observe opening/reading the real child proof,
            // so ACCEPT-only publication discovery cannot silently inspect it.
            let payload_watch = payload_path
                .as_ref()
                .map(|path| watch_candidate_payload(path));
            let opened = Engine::open(&system);
            if let Some(watch) = payload_watch {
                assert_no_candidate_payload_io(watch);
            }
            if attack == "valid" {
                let mut engine = opened.unwrap();
                assert_eq!(fs::read(&missing).unwrap(), snapshot_bytes);
                assert_eq!(
                    engine.plan(&plan.plan_id).unwrap().state,
                    PlanState::Cancelled
                );
                assert_eq!(
                    engine
                        .cancel_plan(&plan.plan_id, "admin", operation)
                        .unwrap(),
                    result
                );
                assert_eq!(engine.state.operation_principals[&prior_operation], "admin");
            } else {
                let error = opened
                    .err()
                    .expect("cold replay must reject the cancellation proof");
                assert_eq!(
                    error.code(),
                    ErrorCode::PublicationInvalid,
                    "{published} {attack}"
                );
                if attack == "payload" {
                    assert!(
                        error
                            .to_string()
                            .contains("metadata-only current-registry candidate"),
                        "{error}"
                    );
                } else if attack.starts_with("payload-orphan") {
                    assert!(
                        error
                            .to_string()
                            .contains("matching native content publication scope"),
                        "{error}"
                    );
                } else if attack == "payload-noop" {
                    assert!(
                        error
                            .to_string()
                            .contains("exact fresh version and placement delta"),
                        "{error}"
                    );
                } else {
                    assert!(
                        error.to_string().contains("plan cancellation differs"),
                        "{error}"
                    );
                }
                assert_eq!(
                    fs::symlink_metadata(&missing).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound
                );
                for (disk, bytes) in members.iter().zip(before_members) {
                    assert_eq!(
                        files(&disk.root),
                        bytes,
                        "member changed: {published} {attack}"
                    );
                }
                assert_eq!(files(&fixture.config.disks[3].root), before_data);
            }
            assert_eq!(files(&system.join("plans")), before_plans);
            assert_eq!(files(&parked), stopped_bytes);
            assert_eq!(
                fs::read(&fixture.config.disks[0].root).unwrap(),
                b"stopped root is not a directory"
            );
        }
    }
}

fn watch_candidate_payload(path: &Path) -> rustix::fd::OwnedFd {
    use rustix::fs::inotify;
    let watch =
        inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).unwrap();
    inotify::add_watch(
        &watch,
        path,
        inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
    )
    .unwrap();
    watch
}

fn assert_no_candidate_payload_io(watch: rustix::fd::OwnedFd) {
    let mut bytes = [0u8; 4096];
    assert_eq!(
        rustix::io::read(&watch, &mut bytes),
        Err(rustix::io::Errno::WOULDBLOCK),
        "a metadata-only candidate must not open or read its smuggled local proof or payload"
    );
}

#[test]
fn current_payload_local_only_publication_preserves_cold_workspace_quota_before_replay() {
    use crate::engine::membership_transition::pending::activation::log_path;
    use crate::journal::{Candidate, JournalFrameReader, RecordType, load_object_at, object_path};

    let mut fixture = Fixture::with_retained_limit(Some(1));
    fixture.exit(0, MembershipExitAction::Deregister);
    let (parked, stopped_bytes) = fixture.replace_stopped_root(0);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open_with_workspace_limits(
        &system,
        Some(RetainedWorkspaceLimits {
            max_retained_workspaces: 2,
            max_retained_workspaces_per_principal: 2,
            max_retained_workspace_metadata_bytes: fixture
                .config
                .max_retained_workspace_metadata_bytes,
            max_retained_workspace_metadata_bytes_per_principal: fixture
                .config
                .max_retained_workspace_metadata_bytes_per_principal,
        }),
    )
    .unwrap();
    let waiting = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&waiting.workspace_id, "waiting", b"still open")
        .unwrap();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(
            &workspace.workspace_id,
            "fresh",
            b"new current registry body",
        )
        .unwrap();
    let request = request(&workspace);
    let result = fixture.engine.commit_workspace(request.clone()).unwrap();
    assert_eq!(
        fixture
            .engine
            .workspace_capacity("admin")
            .principal
            .records
            .used,
        1
    );
    let members = fixture.engine.members();
    let mut tails = Vec::new();
    for disk in &members {
        let path = log_path(&fixture.engine.config, disk);
        let bytes = fs::read(&path).unwrap();
        let mut reader = JournalFrameReader::new(bytes.as_slice());
        loop {
            let before = reader.valid_bytes;
            let (record, _) = reader
                .next_record()
                .unwrap()
                .expect("actual latest publication");
            if record.record_type == RecordType::Publication && record.sequence == result.sequence {
                assert!(
                    reader.next_record().unwrap().is_none(),
                    "truncate only the last frame"
                );
                let candidate: Candidate = load_object_at(
                    &fixture.engine.disk_resources.get(disk).unwrap().root,
                    disk,
                    "candidate",
                    &record.candidate_hash,
                )
                .unwrap();
                assert_eq!(candidate.operation_id, result.operation_id);
                assert_eq!(candidate.versions.len(), 1);
                let evidence = &candidate.versions[0];
                let target = fixture
                    .engine
                    .disk(&evidence.placement.route_slot_ids[0])
                    .unwrap();
                assert!(!target.metadata_member);
                let directory = crate::disk::version_dir(
                    target,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                assert!(directory.join("publication-proof.json").is_file());
                tails.push((
                    path,
                    bytes[..usize::try_from(before).unwrap()].to_vec(),
                    object_path(disk, "publication", &record.object_hash),
                ));
                break;
            }
        }
    }
    drop(fixture.engine);
    // Keep the genuine local publication and immutable complete seal. The last
    // slot has no metadata Publication frames/objects. The persisted limit is
    // one resident workspace: only discovery before quota admission can prove
    // the completed workspace cold while retaining the earlier open workspace.
    // Apply-time discovery alone would charge both records and reject too early.
    for (journal, prefix, publication) in tails {
        fs::write(journal, prefix).unwrap();
        fs::remove_file(publication).unwrap();
    }
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.operation(result.operation_id).unwrap(), result);
    assert_eq!(
        engine.workspace(&waiting.workspace_id).unwrap().state,
        WorkspaceState::Open
    );
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(
        engine.workspace_capacity("admin").principal.records.limit,
        1
    );
    assert_eq!(engine.commit_workspace(request).unwrap(), result);
    assert_eq!(files(&parked), stopped_bytes);
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root is not a directory"
    );
}

#[test]
fn current_payload_scope_binds_real_content_actor_and_operation_before_local_proofs() {
    use crate::engine::MutationPlan;
    use crate::journal::{VersionEvidence, object_path};

    for published in [false, true] {
        for retag in [false, true] {
            let mut fixture = Fixture::new();
            fixture.exit(0, MembershipExitAction::Deregister);
            let (parked, stopped_bytes) = fixture.replace_stopped_root(0);
            let workspace = fixture
                .engine
                .create_workspace(&fixture.repository.main_branch_id, "admin")
                .unwrap();
            fixture
                .engine
                .stage_bytes(&workspace.workspace_id, "fresh", b"real staged body")
                .unwrap();
            let request = request(&workspace);
            let plan = fixture
                .engine
                .plan_workspace_commit_inner(request, None)
                .unwrap();
            let MutationPlan::Prepared {
                mut next,
                prepared,
                mut result,
            } = plan.mutation
            else {
                panic!("new content must have a real payload plan")
            };
            assert_eq!(prepared.len(), 1);
            if retag {
                result.kind = "auth.unrelated".into();
                next.operations.insert(result.operation_id, result.clone());
                next.events.last_mut().unwrap().kind = result.kind.clone();
            } else {
                next.operation_principals
                    .insert(result.operation_id, "another-actor".into());
            }
            fixture
                .engine
                .current_namespaces
                .as_ref()
                .unwrap()
                .advance(&fixture.engine.state, &next)
                .unwrap();
            let versions = prepared
                .iter()
                .map(|copy| VersionEvidence {
                    version: copy.version.clone(),
                    placement: copy.placement.clone(),
                })
                .collect::<Vec<_>>();
            let target = fixture
                .engine
                .disk(&versions[0].placement.route_slot_ids[0])
                .unwrap();
            assert!(!target.metadata_member);
            let directory = crate::disk::version_dir(
                target,
                &versions[0].version.history_tree_id,
                &versions[0].version.version_id,
            );
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("commit-proof.json"), b"{}").unwrap();
            let candidate = install_cancellation_decision(
                &mut fixture.engine,
                &next,
                &result,
                published,
                versions,
            );
            let members = fixture.engine.members();
            let missing = object_path(&members[0], "snapshot", &candidate.snapshot_hash);
            let system = fixture.temp.path().join("system");
            drop(prepared);
            drop(fixture.engine);
            fs::remove_file(&missing).unwrap();
            let metadata = members
                .iter()
                .map(|disk| files(&disk.root))
                .collect::<Vec<_>>();
            let data = files(&fixture.config.disks[3].root);
            let watch = watch_candidate_payload(&directory);
            let error = Engine::open(&system)
                .err()
                .expect("content scope must match its original actor and native operation");
            assert_eq!(
                error.code(),
                ErrorCode::PublicationInvalid,
                "{published} {retag}: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains("matching native content publication scope"),
                "{error}"
            );
            assert_no_candidate_payload_io(watch);
            assert!(!missing.exists());
            for (disk, bytes) in members.iter().zip(metadata) {
                assert_eq!(files(&disk.root), bytes);
            }
            assert_eq!(files(&fixture.config.disks[3].root), data);
            assert_eq!(files(&parked), stopped_bytes);
        }
    }
}

fn current_copy_request(
    fixture: &Fixture,
    source_path: &str,
    target_path: &str,
) -> DirectoryNamespaceBatch {
    let branch = &fixture.engine.state.directory_selections[&fixture.repository.repository_id];
    DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: branch.clone(),
                expected_tree_id: fixture.engine.state.directory_branches[branch]
                    .head_tree_id
                    .clone(),
                path: source_path.into(),
                entry_id: entry(
                    &fixture.engine,
                    &fixture.repository.main_branch_id,
                    source_path,
                )
                .entry_id,
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: branch.clone(),
                expected_tree_id: fixture.engine.state.directory_branches[branch]
                    .head_tree_id
                    .clone(),
                path: target_path.into(),
            }),
        }],
    }
}

fn published_candidate(engine: &Engine, sequence: u64) -> crate::journal::Candidate {
    use crate::engine::membership_transition::pending::activation::log_path;
    use crate::journal::{JournalFrameReader, RecordType, load_object_at};

    let member = &engine.members()[0];
    let bytes = fs::read(log_path(&engine.config, member)).unwrap();
    let mut reader = JournalFrameReader::new(bytes.as_slice());
    loop {
        let (record, _) = reader
            .next_record()
            .unwrap()
            .expect("published sequence exists");
        if record.sequence == sequence && record.record_type == RecordType::Publication {
            return load_object_at(
                &engine.disk_resources.get(member).unwrap().root,
                member,
                "candidate",
                &record.candidate_hash,
            )
            .unwrap();
        }
    }
}

#[test]
fn current_copy_preflight_rebuilds_empty_targets_and_fresh_identities_before_body_io() {
    let mut fixture = Fixture::new();
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    fixture
        .engine
        .initialize_directory_scope(&fixture.repository.repository_id, None, "admin", op())
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (parked, stopped_bytes) = fixture.replace_stopped_root(0);
    let previous = Arc::clone(&fixture.engine.state);
    let previous_snapshot = fixture.engine.snapshot.clone();
    let previous_view = fixture.engine.current_namespaces.clone();
    let request = current_copy_request(&fixture, "live", "copied");
    let result = fixture
        .engine
        .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", op())
        .unwrap();
    let candidate = published_candidate(&fixture.engine, result.sequence);
    assert_eq!(candidate.versions.len(), 1);
    let copied = entry(
        &fixture.engine,
        &fixture.repository.main_branch_id,
        "copied",
    );
    let published = Arc::clone(&fixture.engine.state);
    let published_snapshot = fixture.engine.snapshot.clone();
    let published_view = fixture.engine.current_namespaces.clone();
    let target = fixture
        .engine
        .disk(&candidate.versions[0].placement.route_slot_ids[0])
        .unwrap();
    let directory = crate::disk::version_dir(
        target,
        &candidate.versions[0].version.history_tree_id,
        &candidate.versions[0].version.version_id,
    );
    fixture.engine.state = Arc::clone(&previous);
    fixture.engine.snapshot = previous_snapshot;
    fixture.engine.current_namespaces = previous_view;
    assert!(
        fixture
            .engine
            .current_namespaces
            .as_ref()
            .unwrap()
            .copy_identity_is_fresh(&copied)
    );
    assert!(
        !fixture
            .engine
            .current_namespaces
            .as_ref()
            .unwrap()
            .copy_identity_is_fresh(&fixture.current)
    );
    // A canonical round trip forces every tracked map through its cold path.
    let cold: State = json_codec::from_slice(&canonical_json(published.as_ref()).unwrap()).unwrap();
    fixture
        .engine
        .verify_current_recovery_candidate(&candidate, &cold)
        .unwrap();
    for destination in ["live", "missing/copied"] {
        let mut forged = cold.clone();
        let mut request = request.clone();
        request.items[0].target.as_mut().unwrap().path = destination.into();
        let receipt = forged.operations.get_mut(&result.operation_id).unwrap();
        receipt.result["transfers"][0]["target"]["path"] = json!(destination);
        receipt.request_digest = Engine::native_request_digest(
            "directory.namespace.batch",
            "admin",
            &json!({"repository_id": fixture.repository.repository_id, "request": request}),
        )
        .unwrap();
        let mut forged_candidate = candidate.clone();
        forged_candidate.request_digest = receipt.request_digest.clone();
        let before_members = fixture
            .engine
            .members()
            .iter()
            .map(|disk| (disk.root.clone(), files(&disk.root)))
            .collect::<Vec<_>>();
        let before_data = files(&fixture.config.disks[3].root);
        let watch = watch_candidate_payload(&directory);
        let error = fixture
            .engine
            .verify_current_recovery_candidate(&forged_candidate, &forged)
            .expect_err("copy destinations must be empty and have an admitted parent");
        assert_eq!(error.code(), ErrorCode::PublicationInvalid);
        assert!(
            error
                .to_string()
                .contains("exact empty destination and fresh identity projection"),
            "{error}"
        );
        assert_no_candidate_payload_io(watch);
        for (root, bytes) in before_members {
            assert_eq!(files(&root), bytes);
        }
        assert_eq!(files(&fixture.config.disks[3].root), before_data);
    }
    fixture.engine.state = published;
    fixture.engine.snapshot = published_snapshot;
    fixture.engine.current_namespaces = published_view;
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .directory_namespace_batch(
                &fixture.repository.repository_id,
                &request,
                "admin",
                result.operation_id,
            )
            .unwrap(),
        result
    );
    assert_eq!(
        entry(&engine, &fixture.repository.main_branch_id, "copied"),
        copied
    );
    assert_eq!(files(&parked), stopped_bytes);
}

#[test]
fn current_copy_preflight_accepts_a_parent_copied_earlier_in_the_same_batch() {
    let mut fixture = Fixture::new();
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "source/nested",
        b"nested body",
    );
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    fixture
        .engine
        .initialize_directory_scope(&fixture.repository.repository_id, None, "admin", op())
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let mut request = current_copy_request(&fixture, "source", "parent-copy");
    request
        .items
        .extend(current_copy_request(&fixture, "live", "parent-copy/second").items);
    let result = fixture
        .engine
        .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", op())
        .unwrap();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .directory_namespace_batch(
                &fixture.repository.repository_id,
                &request,
                "admin",
                result.operation_id,
            )
            .unwrap(),
        result
    );
    assert_ne!(
        entry(
            &engine,
            &fixture.repository.main_branch_id,
            "parent-copy/nested"
        )
        .file_id,
        entry(&engine, &fixture.repository.main_branch_id, "source/nested").file_id
    );
    assert_ne!(
        entry(
            &engine,
            &fixture.repository.main_branch_id,
            "parent-copy/second"
        )
        .file_id,
        fixture.current.file_id
    );
}
