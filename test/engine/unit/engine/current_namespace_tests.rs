//! Real business consumers with an internally installed, semantically verified
//! namespace fixture. This does not simulate D3 durability, member retirement,
//! target I/O closure or production activation; those require lifecycle tests.
use super::super::published::occurrence::FrozenDetachSource;
use super::*;
use crate::*;
use std::{collections::BTreeMap, io::Read};

type Objects = BTreeMap<(String, String), Vec<u8>>;
fn read(objects: &Objects, kind: &str, hash: &str) -> Result<Vec<u8>> {
    objects
        .get(&(kind.into(), hash.into()))
        .cloned()
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "test object absent"))
}
fn verified(prepared: &crate::PreparedDiskRegistry) -> crate::VerifiedDiskRegistry {
    let objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    prepared
        .verify_with(|kind, hash| read(&objects, kind, hash))
        .unwrap()
}
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
        message: "namespace regression".into(),
        request_digest: None,
    }
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine.commit_workspace(request(&workspace)).unwrap();
}
struct Fixture {
    _temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    original_head: String,
    removed: Entry,
    retained: Entry,
}
impl Fixture {
    fn new(file_manager: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("data-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = if file_manager {
            engine.create_disk_root("D04", "admin", op()).unwrap()
        } else {
            engine
                .create_repository(
                    &RepositoryName::parse("namespace").unwrap(),
                    "D04",
                    1,
                    "admin",
                    op(),
                )
                .unwrap()
        };
        let rule = new_id("rule").unwrap();
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir/a",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(&mut engine, &repo.main_branch_id, "dir/a", b"only target");
        save(&mut engine, &repo.main_branch_id, "dir/b", b"retained");
        // Future writes are explicitly routed to a retained registered primary.
        // Deregistration itself never promotes a backup or rewrites placement.
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir/a",
                primary_slot_id: "D04",
                expected_generation: 1,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
        let removed = tree.get("dir/a").unwrap().clone();
        let retained = tree.get("dir/b").unwrap().clone();
        let original_head = engine.state.branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        Self {
            _temp: temp,
            engine,
            repo,
            original_head,
            removed,
            retained,
        }
    }
    fn project(&mut self, disable: bool) {
        let base = verified(&self.engine.prepare_disk_registry_base().unwrap());
        let target = base
            .historical_topology()
            .find(|disk| disk.identity.slot_id == "P01")
            .unwrap()
            .identity
            .clone();
        let source = FrozenDetachSource::capture(&self.engine, &base, &target).unwrap();
        let prepared_mask = source.prepare_mask().unwrap();
        let mask_objects = prepared_mask
            .objects()
            .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
            .collect();
        let mask = source
            .verify_mask(prepared_mask.hash(), |kind, hash| {
                read(&mask_objects, kind, hash)
            })
            .unwrap();
        let successor = verified(
            &base
                .prepare_exit(RegistryExitRequest {
                    target,
                    operation_id: op(),
                    principal: "admin".into(),
                    request_digest: "1".repeat(64),
                    intent_hash: "2".repeat(64),
                    effect: if disable {
                        RegistryExitEffect::Disable
                    } else {
                        RegistryExitEffect::Detach {
                            occurrence_mask_hash: prepared_mask.hash().into(),
                        }
                    },
                })
                .unwrap(),
        );
        let prepared = if disable {
            source.prepare_disabled_namespaces(&successor).unwrap()
        } else {
            mask.prepare_current_namespaces(&successor).unwrap()
        };
        // Internal source fixture, never a production installer or authority token.
        self.engine.named_namespaces.install_current(
            &mut self.engine.current_namespaces, Some(Arc::new(prepared.view)),
        );
        self.engine.workspace_namespace_baselines.clear();
    }
}

#[path = "current_namespace_batch_tests.rs"]
mod batches;
#[path = "current_namespace_directory_label_tests.rs"]
mod directory_labels;
#[path = "current_namespace_initialization_tests.rs"]
mod initialization;
#[path = "current_namespace_placement_tests.rs"]
mod placement;
#[path = "current_namespace_rename_tests.rs"]
mod rename;
#[path = "current_namespace_repository_tests.rs"]
mod repositories;
#[path = "current_namespace_workspace_overlap_tests.rs"]
mod workspace_overlap;

#[test]
fn membership_registry_current_listing_and_lookup_preserve_immutable_history() {
    let mut fixture = Fixture::new(true);
    let before = canonical_json(fixture.engine.state()).unwrap();
    fixture.project(false);
    let tree = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap();
    assert!(tree.get("dir/a").is_none());
    assert_eq!(tree.get("dir/b"), Some(&fixture.retained));
    assert!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "dir/a")
            .is_none()
    );
    assert!(
        fixture
            .engine
            .metadata_view()
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .is_none()
    );
    for branch in fixture.engine.state.directory_branches.values() {
        assert!(
            fixture
                .engine
                .current_directory_entries(&branch.directory_branch_id)
                .unwrap()
                .iter()
                .all(|entry| entry.path() != "dir/a")
        );
    }
    assert!(
        fixture
            .engine
            .namespace_tree_at(&fixture.repo.main_branch_id, &fixture.original_head)
            .unwrap()
            .get("dir/a")
            .is_some()
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    let mut bytes = Vec::new();
    fixture
        .engine
        .open_content(&fixture.removed.version_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(
        bytes, b"only target",
        "projection must not wipe target bytes"
    );
}

#[test]
fn membership_registry_current_same_name_upload_gets_fresh_identity_and_context() {
    let mut fixture = Fixture::new(true);
    let original = fixture.engine.state.commits[&fixture.original_head].clone();
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    assert!(
        workspace
            .selected_file_contexts
            .keys()
            .all(|key| !key.ends_with("/dir/a"))
    );
    let staged = fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/a", b"new identity")
        .unwrap();
    assert_eq!(staged.base_version_id, None);
    fixture
        .engine
        .commit_workspace(request(&workspace))
        .unwrap();
    let created = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("dir/a")
        .unwrap()
        .clone();
    assert_ne!(created.entry_id, fixture.removed.entry_id);
    assert_ne!(created.file_id, fixture.removed.file_id);
    assert!(
        fixture.engine.state.versions[&created.version_id]
            .source_version_ids
            .is_empty()
    );
    assert_eq!(
        fixture.engine.state.commits[&fixture.original_head],
        original
    );
    assert!(
        fixture
            .engine
            .state
            .versions
            .contains_key(&fixture.removed.version_id)
    );
    // The original removed path remains in the mask, but this new occurrence may
    // continue normally and its newly established context must not be stripped.
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"second write",
    );
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .unwrap()
            .file_id,
        created.file_id
    );
}

#[test]
fn membership_registry_current_unrelated_commit_and_selected_child_trees_do_not_resurrect() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/b",
        b"retained update",
    );
    let head = &fixture.engine.state.branches[&fixture.repo.main_branch_id].head_commit_id;
    assert!(
        fixture.engine.state.commits[head]
            .entries
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/b")
            .unwrap()
            .file_id,
        fixture.retained.file_id
    );
    for branch in fixture.engine.state.directory_selections.values() {
        let branch = &fixture.engine.state.directory_branches[branch];
        let tree = &fixture.engine.state.directory_trees[&branch.head_tree_id];
        assert!(tree.entries.iter().all(|entry| entry.path() != "dir/a"));
    }
}

#[test]
fn membership_registry_current_delete_tree_operates_only_on_retained_entries() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "dir", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    assert!(
        fixture
            .engine
            .list_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .is_empty()
    );
    let head = &fixture.engine.state.branches[&fixture.repo.main_branch_id].head_commit_id;
    let deletions = fixture.engine.state.commits[head]
        .namespace_deletions
        .as_ref()
        .unwrap();
    assert!(deletions.paths.iter().all(|entry| entry.path != "dir/a"));
    assert!(
        fixture
            .engine
            .state
            .versions
            .contains_key(&fixture.removed.version_id)
    );
}

#[test]
fn membership_registry_current_old_workspace_and_old_upload_identity_are_rejected() {
    let mut fixture = Fixture::new(false);
    let old = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture.project(false);
    assert_eq!(
        fixture
            .engine
            .stage_bytes(&old.workspace_id, "dir/a", b"old")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    let new = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let path = fixture._temp.path().join("upload");
    std::fs::write(&path, b"fresh").unwrap();
    assert_eq!(
        fixture
            .engine
            .stage_file_as(&new.workspace_id, &fixture.removed.entry_id, "dir/a", &path)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
}

#[test]
fn membership_registry_current_workspace_baseline_recovers_only_from_published_receipt() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture.engine.workspace_namespace_baselines.clear();
    assert!(
        fixture
            .engine
            .workspace_namespace_entries(&workspace)
            .is_err()
    );
    fixture
        .engine
        .restore_namespace_workspace_baselines()
        .unwrap();
    assert!(
        fixture
            .engine
            .workspace_namespace_entries(&workspace)
            .unwrap()
            .entries()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    let mut forged = workspace.clone();
    forged.fencing_token += 1;
    assert!(fixture.engine.workspace_namespace_entries(&forged).is_err());
    forged = workspace;
    forged.principal = "someone-else".into();
    assert!(fixture.engine.workspace_namespace_entries(&forged).is_err());
}

#[test]
fn membership_registry_current_historical_import_fails_before_mutation() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert!(
        fixture
            .engine
            .create_branch(
                &fixture.original_head,
                &BranchName::parse("restored").unwrap(),
                "admin",
                op()
            )
            .is_err()
    );
    assert!(
        fixture
            .engine
            .create_historical_workspace(
                &fixture.repo.main_branch_id,
                &fixture.original_head,
                "admin"
            )
            .is_err()
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
}

#[test]
fn membership_registry_current_historical_save_preview_preserves_plans_and_staged_bytes() {
    let mut fixture = Fixture::new(false);
    let plan = fixture
        .engine
        .create_historical_save_plan(
            &fixture.repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: fixture.original_head.clone(),
                name: "historical import".into(),
                workspace_id: None,
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    assert_eq!(plan.state, PlanState::Ready);
    let plan_path = fixture
        .engine
        .root
        .join("plans")
        .join(format!("{}.json", plan.plan_id));
    let plan_bytes = std::fs::read(&plan_path).unwrap();
    let workspace = fixture
        .engine
        .create_historical_workspace(
            &fixture.repo.main_branch_id,
            &fixture.original_head,
            "admin",
        )
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/b", b"retained staged body")
        .unwrap();
    let workspace = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    let manifests = [
        fixture
            .engine
            .workspace_data_directory(&workspace)
            .unwrap()
            .join("workspace.json"),
        fixture
            .engine
            .root
            .join("workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
    ];
    let manifest_bytes = manifests.map(|path| (path.clone(), std::fs::read(path).unwrap()));
    let workspace_bytes = canonical_json(&workspace).unwrap();
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    for workspace_id in [None, Some(workspace.workspace_id.clone())] {
        assert_eq!(
            fixture
                .engine
                .create_historical_save_plan(
                    &fixture.repo.history_tree_id,
                    HistoricalSaveOptions {
                        source_commit_id: fixture.original_head.clone(),
                        name: "unadmitted import".into(),
                        workspace_id,
                        primary_slot_id: None,
                        target_copies: None,
                    },
                    "admin",
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
    }
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(std::fs::read(&plan_path).unwrap(), plan_bytes);
    assert_eq!(
        std::fs::read_dir(plan_path.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        canonical_json(&fixture.engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        workspace_bytes
    );
    for (path, bytes) in manifest_bytes {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        std::fs::read(staged.source_path).unwrap(),
        b"retained staged body"
    );
}

#[test]
fn membership_registry_current_historical_save_replays_its_original_success() {
    let mut fixture = Fixture::new(false);
    let plan = fixture
        .engine
        .create_historical_save_plan(
            &fixture.repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: fixture.original_head.clone(),
                name: "completed historical import".into(),
                workspace_id: None,
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let operation = op();
    let result = fixture
        .engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        result
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_disable_keeps_namespace_and_history() {
    let mut fixture = Fixture::new(true);
    fixture.project(true);
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a"),
        Some(&fixture.removed)
    );
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/b"),
        Some(&fixture.retained)
    );
}

#[test]
fn membership_registry_current_registration_and_write_routes_use_the_admitted_view() {
    let mut fixture = Fixture::new(false);
    let target = fixture
        .engine
        .config
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap()
        .disk_id
        .clone();
    fixture.project(false);
    assert!(fixture.engine.registered_disk(&target).is_none());
    assert!(
        fixture
            .engine
            .registered_disks()
            .all(|disk| disk.slot_id != "P01")
    );
    assert!(fixture.engine.route("P01", 1).is_err());
    assert_eq!(fixture.engine.route("D04", 1).unwrap(), ["D04"]);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert!(
        fixture
            .engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &new_id("rule").unwrap(),
                repository_id: &fixture.repo.repository_id,
                path_prefix: "blocked",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .is_err()
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule").unwrap(),
            repository_id: &fixture.repo.repository_id,
            path_prefix: "new",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "new/file",
        b"retained route",
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("new/file")
            .is_some()
    );
}

#[test]
fn membership_registry_current_disabled_registration_is_visible_but_not_a_write_route() {
    let mut fixture = Fixture::new(false);
    let target = fixture
        .engine
        .config
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap()
        .disk_id
        .clone();
    fixture.project(true);
    assert_eq!(
        fixture
            .engine
            .registered_disk(&target)
            .unwrap()
            .registration_mode,
        RegistrationMode::Disabled
    );
    assert!(fixture.engine.route("P01", 1).is_err());
    assert_eq!(fixture.engine.route("D04", 1).unwrap(), ["D04"]);
}

#[test]
fn membership_registry_current_shared_version_survives_without_granting_removed_owner_writes() {
    let mut fixture = Fixture::new(false);
    let owned = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("target-owned").unwrap(),
            "P01",
            2,
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &owned.main_branch_id,
        "shared",
        b"two independent copies",
    );
    let shared = fixture
        .engine
        .namespace_tree(&owned.main_branch_id)
        .unwrap()
        .get("shared")
        .unwrap()
        .clone();
    let version = fixture.engine.state.versions[&shared.version_id].clone();
    fixture.project(false);
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&owned.main_branch_id)
            .unwrap()
            .get("shared"),
        Some(&shared)
    );
    assert_eq!(fixture.engine.state.versions[&shared.version_id], version);
    assert_eq!(
        fixture
            .engine
            .create_workspace(&owned.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
}

#[test]
fn membership_registry_current_cold_workspace_admission_survives_later_head_and_restart_index() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/b", b"published")
        .unwrap();
    fixture
        .engine
        .commit_workspace(request(&workspace))
        .unwrap();
    let completed = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(completed.state, WorkspaceState::Committed);
    // The durable receipt index must not depend on an in-memory manifest cache.
    fixture.engine.workspaces.remove(&workspace.workspace_id);
    fixture.engine.workspace_namespace_baselines.clear();
    fixture
        .engine
        .restore_namespace_workspace_baselines()
        .unwrap();
    let baseline = fixture
        .engine
        .workspace_namespace_entries(&completed)
        .unwrap();
    assert!(baseline.entries().iter().all(|entry| entry.path() != "dir/a"));
    assert!(baseline.entries().iter().any(|entry| entry == &fixture.retained));
}

#[test]
fn membership_registry_current_missing_admission_never_falls_back_in_metadata_snapshot() {
    let fixture = Fixture::new(false);
    let mut metadata = fixture.engine.metadata_view();
    metadata.current_namespaces = crate::engine::NamespaceReadView::AwaitingPublication;
    assert_eq!(
        metadata
            .namespace_tree(&fixture.repo.main_branch_id)
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn membership_registry_current_generic_imports_never_invoke_the_mutation_planner() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    for kind in ["branch.create", "merge-link.create"] {
        let invoked = std::cell::Cell::new(false);
        let result = fixture.engine.planning_context().plan_new_mutation(
            kind,
            "admin",
            op(),
            Engine::native_request_digest(kind, "admin", &json_codec::json!({})).unwrap(),
            |_, _, _| {
                invoked.set(true);
                Ok(json_codec::json!({}))
            },
        );
        assert!(result.is_err());
        assert!(!invoked.get());
    }
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

// Continuation fixtures model only verified metadata and owned projections. The
// authority author separately verifies genuine D3/archives and live I/O guards.
fn continued_source(fixture: &Fixture) -> (ValidatedConfig, State, String, String) {
    let mut config = fixture.engine.config().clone();
    let previous = fixture.engine.current_namespaces.as_ref().unwrap();
    for disk in &mut config.disks {
        disk.metadata_member = previous
            .registry_state
            .historical_topology()
            .find(|historical| historical.identity.disk_id == disk.disk_id)
            .map(|historical| {
                previous
                    .registry_state
                    .is_voter(&historical.identity)
                    .unwrap()
            })
            .unwrap();
    }
    let mut source = fixture.engine.state().clone();
    source.sequence += 1;
    source.topology_generation += 1;
    source.business_generation += 1;
    (
        config.try_into().unwrap(),
        source,
        "a".repeat(64),
        "b".repeat(64),
    )
}

fn continue_exit(fixture: &Fixture, slot: &str, disable: bool) -> CurrentNamespaces {
    use super::super::published::continuation::RegistryBoundary;
    let previous = fixture.engine.current_namespaces.as_ref().unwrap();
    let (config, state, snapshot, authority) = continued_source(fixture);
    let base = verified(
        &previous
            .registry_state
            .prepare_continuation(
                &config,
                &config,
                RegistryBoundary {
                    authority: &authority,
                    snapshot: &snapshot,
                    sequence: state.sequence,
                    topology_generation: state.topology_generation,
                },
            )
            .unwrap(),
    );
    let target = base
        .historical_topology()
        .find(|disk| disk.identity.slot_id == slot)
        .unwrap()
        .identity
        .clone();
    let source = FrozenDetachSource::capture_current(
        &state, &config, &snapshot, &authority, previous, &base, &target,
    )
    .unwrap();
    let mask = source.prepare_mask().unwrap();
    let objects: Objects = mask
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    let mask_proof = source
        .verify_mask(mask.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = verified(
        &base
            .prepare_exit(RegistryExitRequest {
                target,
                operation_id: op(),
                principal: "admin".into(),
                request_digest: "c".repeat(64),
                intent_hash: "d".repeat(64),
                effect: if disable {
                    RegistryExitEffect::Disable
                } else {
                    RegistryExitEffect::Detach {
                        occurrence_mask_hash: mask.hash().into(),
                    }
                },
            })
            .unwrap(),
    );
    if disable {
        source.prepare_disabled_namespaces(&next).unwrap().view
    } else {
        mask_proof.prepare_current_namespaces(&next).unwrap().view
    }
}

#[test]
fn membership_registry_current_second_detach_keeps_the_prior_projection() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let second = continue_exit(&fixture, "B01", false);
    let current = second
        .branch_entries(&fixture.repo.main_branch_id, &fixture.original_head)
        .unwrap();
    assert!(current.iter().all(|entry| entry.path() != "dir/a"));
    assert!(current.iter().any(|entry| entry == &fixture.retained));
    assert!(second.known_entries.contains(&fixture.removed.entry_id));
    assert!(
        second
            .unavailable_versions
            .contains(&fixture.removed.version_id)
    );
    assert_eq!(
        fixture.engine.state.commits[&fixture.original_head]
            .entries
            .iter()
            .find(|entry| entry.path() == "dir/a"),
        Some(&fixture.removed)
    );
}

#[test]
fn membership_registry_current_disabled_target_can_be_detached_without_raw_head_revival() {
    let mut fixture = Fixture::new(false);
    fixture.project(true);
    let detached = continue_exit(&fixture, "P01", false);
    assert!(
        detached
            .branch_entries(&fixture.repo.main_branch_id, &fixture.original_head)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    assert!(
        detached
            .registry_state
            .registered_definitions()
            .all(|(disk, _, _)| disk.identity.slot_id != "P01")
    );
}

#[test]
fn membership_registry_current_add_carries_hidden_entries_without_rebuilding_raw_heads() {
    use super::super::published::continuation::RegistryBoundary;
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let (source_config, source, snapshot, authority) = continued_source(&fixture);
    let mut target = (*source_config).clone();
    let mut added = target.disks[3].clone();
    added.disk_id = new_id("disk").unwrap();
    added.slot_id = "P02".into();
    added.root = fixture._temp.path().join("added");
    added.metadata_member = true;
    target.disks.push(added);
    let target = ValidatedConfig::try_from(target).unwrap();
    let previous = fixture.engine.current_namespaces.as_ref().unwrap();
    let next = verified(
        &previous
            .registry_state
            .prepare_continuation(
                &source_config,
                &target,
                RegistryBoundary {
                    authority: &authority,
                    snapshot: &snapshot,
                    sequence: source.sequence,
                    topology_generation: source.topology_generation,
                },
            )
            .unwrap(),
    );
    let carried = previous
        .prepare_continuation(&source, &snapshot, &next)
        .unwrap()
        .view;
    assert!(
        carried
            .branch_entries(&fixture.repo.main_branch_id, &fixture.original_head)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    assert_eq!(
        carried.registry_state.registered_definitions().count(),
        previous.registry_state.registered_definitions().count() + 1
    );
    assert!(
        carried
            .removed_paths(&fixture.repo.main_branch_id)
            .any(|path| path == "dir/a")
    );
}

#[test]
fn membership_registry_current_continuation_does_not_clear_recreated_path_contexts() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"new identity",
    );
    let (config, source, snapshot, authority) = continued_source(&fixture);
    let previous = fixture.engine.current_namespaces.as_ref().unwrap();
    let next = verified(
        &previous
            .registry_state
            .prepare_continuation(
                &config,
                &config,
                super::super::published::continuation::RegistryBoundary {
                    authority: &authority,
                    snapshot: &snapshot,
                    sequence: source.sequence,
                    topology_generation: source.topology_generation,
                },
            )
            .unwrap(),
    );
    let carried = previous
        .prepare_continuation(&source, &snapshot, &next)
        .unwrap()
        .view;
    assert!(
        !carried
            .removed_paths(&fixture.repo.main_branch_id)
            .any(|path| path == "dir/a")
    );
    let current = carried
        .branch_entries(
            &fixture.repo.main_branch_id,
            &source.branches[&fixture.repo.main_branch_id].head_commit_id,
        )
        .unwrap();
    let recreated = current
        .iter()
        .find(|entry| entry.path() == "dir/a")
        .unwrap();
    assert_ne!(recreated.file_id, fixture.removed.file_id);
    let mut changed = source;
    changed
        .branches
        .get_mut(&fixture.repo.main_branch_id)
        .unwrap()
        .head_commit_id = fixture.original_head.clone();
    assert!(
        previous
            .prepare_continuation(&changed, &snapshot, &next)
            .is_err()
    );
}

#[test]
fn membership_registry_current_snapshot_witness_changes_at_projection_not_auth_only_advance() {
    let mut fixture = Fixture::new(false);
    let initial = fixture
        .engine
        .namespace_snapshot(&fixture.repo.main_branch_id)
        .unwrap();
    let initial_witness = initial.witness();
    fixture.project(false);
    assert!(!fixture.engine.namespace_snapshot_matches(&initial_witness));
    assert_eq!(initial.head_commit_id(), fixture.original_head);
    assert!(initial.tree().get("dir/a").is_some());

    let projected = fixture
        .engine
        .namespace_snapshot(&fixture.repo.main_branch_id)
        .unwrap();
    let projected_witness = projected.witness();
    assert!(projected.tree().get("dir/a").is_none());
    let owned = match projected.into_entries() {
        crate::NamespaceEntries::Projected(entries) => entries,
        crate::NamespaceEntries::Commit(_) => {
            panic!("current namespace must own projected entries")
        }
    };
    let current = fixture.engine.current_namespaces.as_ref().unwrap();
    assert!(Arc::ptr_eq(
        &owned,
        &current.branches[&fixture.repo.main_branch_id].entries
    ));
    // An admitted publication with no changed namespace shares both the entries
    // allocation and registry identity. No live admission is manufactured here.
    let next = current
        .advance(fixture.engine.state(), fixture.engine.state())
        .unwrap();
    fixture.engine.named_namespaces.install_current(
        &mut fixture.engine.current_namespaces, Some(Arc::new(next)),
    );
    drop(owned);
    assert!(
        fixture
            .engine
            .namespace_snapshot_matches(&projected_witness)
    );
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/b",
        b"changed",
    );
    assert!(
        !fixture
            .engine
            .namespace_snapshot_matches(&projected_witness)
    );
}

#[test]
fn membership_registry_current_planning_inventory_rejects_stale_scope_heads() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    let branch = &fixture.repo.main_branch_id;
    let planning = fixture.engine.planning_context();
    assert!(
        planning
            .current_branch_entries(branch)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    assert!(
        view.branch_writable(fixture.engine.state(), branch)
            .unwrap()
    );
    let mut state = fixture.engine.state().clone();
    state.branches.get_mut(branch).unwrap().head_commit_id = "foreign".into();
    assert_eq!(
        view.branch_writable(&state, branch).unwrap_err().code(),
        ErrorCode::WorkspaceChanged
    );
    let directory = view.directories.keys().next().unwrap();
    assert!(
        view.directory_writable(fixture.engine.state(), directory)
            .unwrap()
    );
    state
        .directory_branches
        .get_mut(directory)
        .unwrap()
        .head_tree_id = "foreign".into();
    assert_eq!(
        view.directory_writable(&state, directory)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
}

#[test]
fn membership_registry_current_owned_directory_view_keeps_its_captured_projection() {
    let mut fixture = Fixture::new(true);
    let initial = fixture.engine.directory_namespace_view().unwrap();
    let root = fixture.engine.state.directory_selections[&fixture.repo.repository_id].clone();
    assert!(
        initial
            .entries(&root)
            .unwrap()
            .iter()
            .any(|entry| entry.path() == "dir/a")
    );
    fixture.project(false);
    let projected = fixture.engine.directory_namespace_view().unwrap();
    let cloned = projected.clone();
    let expected = fixture.engine.current_directory_entries(&root).unwrap();
    assert!(std::ptr::eq(projected.entries(&root).unwrap(), expected));
    assert!(std::ptr::eq(cloned.entries(&root).unwrap(), expected));
    assert!(
        projected
            .entries(&root)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"recreated",
    );
    assert!(
        fixture
            .engine
            .current_directory_entries(&root)
            .unwrap()
            .iter()
            .any(|entry| entry.path() == "dir/a")
    );
    assert!(
        projected
            .entries(&root)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    assert!(
        initial
            .entries(&root)
            .unwrap()
            .iter()
            .any(|entry| entry == &fixture.removed)
    );
    assert_eq!(
        projected.entries("absent").unwrap_err().code(),
        ErrorCode::DirectoryBranchNotFound
    );
}

#[test]
fn membership_registry_current_same_registry_activation_does_not_readmit_old_workspace() {
    let mut fixture = Fixture::new(false);
    fixture.project(true);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    assert!(
        fixture
            .engine
            .workspace_namespace_entries(&workspace)
            .is_ok()
    );
    let (_, source, snapshot, _) = continued_source(&fixture);
    let previous = fixture.engine.current_namespaces.as_ref().unwrap();
    let mut carried = previous
        .prepare_continuation(&source, &snapshot, &previous.registry_state)
        .unwrap()
        .view;
    assert_eq!(carried.registry, previous.registry);
    assert_eq!(carried.mask_hash, previous.mask_hash);
    // This private semantic fixture models the next admitted activation's
    // sequence. It is not a Live/Archived publication token or live I/O proof.
    let mut published = source;
    published.sequence += 1;
    carried.activation_sequence = published.sequence;
    fixture.engine.named_namespaces.install_state(&mut fixture.engine.state, Arc::new(published));
    fixture.engine.named_namespaces.install_current(
        &mut fixture.engine.current_namespaces, Some(Arc::new(carried)),
    );
    fixture.engine.workspace_namespace_baselines.clear();
    fixture
        .engine
        .restore_namespace_workspace_baselines()
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .workspace_namespace_entries(&workspace)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert!(fixture.engine.workspace(&workspace.workspace_id).is_ok());
}

#[path = "current_namespace_workspace_names_tests.rs"]
mod workspace_names;
