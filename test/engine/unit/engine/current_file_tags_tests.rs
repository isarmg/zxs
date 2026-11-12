//! Real temporary-root Engine/D3 publications, plus cold decoded-state proofs.
//! These are regression sources, not physical-device or HTTP acceptance evidence.
use super::*;
use crate::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn name(value: &str) -> BranchName {
    BranchName::parse(value).unwrap()
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> Entry {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "current file tags fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(branch)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for child in fs::read_dir(root).unwrap() {
        let path = child.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}
fn cold(state: &State) -> State {
    json_codec::from_slice(&canonical_json(state).unwrap()).unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    hidden: Entry,
    first: Entry,
    current: Entry,
    branch: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine.create_disk_root("D04", "admin", op()).unwrap();
        let hidden_rule = new_id("rule").unwrap();
        let live_rule = new_id("rule").unwrap();
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&live_rule, "dir/live")] {
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: rule,
                    repository_id: &repository.repository_id,
                    path_prefix: path,
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let hidden = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/gone",
            b"target only",
        );
        let first = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"historical source",
        );
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&live_rule, "dir/live")] {
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: rule,
                    repository_id: &repository.repository_id,
                    path_prefix: path,
                    primary_slot_id: "D04",
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let current = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"retained body",
        );
        let branch = engine
            .file_context(&repository.repository_id, "dir/live")
            .unwrap()
            .branch_id;
        Self {
            temp,
            engine,
            config,
            repository,
            hidden,
            first,
            current,
            branch,
        }
    }
    fn exit(&mut self, action: MembershipExitAction) -> BTreeMap<PathBuf, Vec<u8>> {
        let disk = self.config.disks[0].clone();
        let held = Arc::clone(self.engine.disk_resources.get(&disk).unwrap());
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
        self.engine.resume_membership_joint_decision().unwrap();
        let physical = files(&disk.root);
        self.engine.activate_pending_disk_exit().unwrap();
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), physical);
        physical
    }
}

fn assert_exit_visibility(
    engine: &Engine,
    repository: &Repository,
    hidden: &Entry,
    removed_disk: &DiskConfig,
    detached: bool,
) {
    assert!(engine.disk_resources.available(removed_disk).is_none());
    let namespace = engine.namespace_tree(&repository.main_branch_id).unwrap();
    let view = engine.current_namespaces.as_ref().unwrap();
    if detached {
        assert!(namespace.get("dir/gone").is_none());
        for scope in view.branches.values().chain(view.directories.values()) {
            assert!(
                scope
                    .entries
                    .iter()
                    .all(|entry| entry.file_id != hidden.file_id)
            );
        }
    } else {
        // PlannedOffline disables I/O without detaching registered occurrences.
        // It must retain identity, not accidentally apply the deregistration mask.
        let retained = namespace.get("dir/gone").unwrap();
        assert_eq!(retained.file_id, hidden.file_id);
        assert_eq!(retained.version_id, hidden.version_id);
        assert!(
            view.branches[&repository.main_branch_id]
                .entries
                .iter()
                .any(|entry| entry.file_id == hidden.file_id)
        );
    }
}

#[test]
fn current_container_tags_live_cold_replay_rejects_extra_writes_and_forged_requests() {
    for action in [
        MembershipExitAction::Deregister,
        MembershipExitAction::PlannedOffline,
    ] {
        let mut fixture = Fixture::new();
        let detached = action == MembershipExitAction::Deregister;
        fixture.exit(action);
        assert_exit_visibility(
            &fixture.engine,
            &fixture.repository,
            &fixture.hidden,
            &fixture.config.disks[0],
            detached,
        );
        let request = storage_contract::file_container_tags::ReplaceCommand {
            file_id: fixture.current.file_id.parse().unwrap(),
            context: storage_contract::file_container_tags::Context {
                repository_id: fixture.repository.repository_id.parse().unwrap(),
                path: name_policy::PortablePath::parse("dir/live").unwrap(),
            },
            expected_generation: storage_contract::Generation::new(0),
            tags: storage_contract::file_container_tags::TagSet::new([
                storage_contract::file_container_tags::Tag::parse("review".into()).unwrap(),
            ])
            .unwrap(),
        };
        let previous = cold(fixture.engine.state());
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let receipt = fixture
            .engine
            .replace_container_tags(&request, &"admin".parse().unwrap(), op())
            .unwrap();
        assert_exit_visibility(
            &fixture.engine,
            &fixture.repository,
            &fixture.hidden,
            &fixture.config.disks[0],
            detached,
        );
        let next = cold(fixture.engine.state());
        view.verify_new_file_tags(&previous, &next, &receipt).unwrap();
        assert_eq!(previous.branches, next.branches);
        assert_eq!(previous.directory_trees, next.directory_trees);
        assert_eq!(previous.file_metadata_nodes, next.file_metadata_nodes);
        for corruption in 0..5 {
            let mut bad = cold(&next);
            match corruption {
                0 => {
                    bad.file_notes
                        .insert(fixture.current.file_id.clone(), "forged".into());
                }
                1 => {
                    bad.container_tags.insert(
                        request.file_id,
                        storage_contract::file_container_tags::ContainerTags::empty(),
                    );
                }
                2 => {
                    bad.operations.get_mut(&receipt.operation_id).unwrap().result["request"]
                        ["expected_generation"] = json_codec::json!("1");
                }
                3 => {
                    bad.events.pop();
                }
                4 => {
                    bad.operation_principals
                        .insert(receipt.operation_id, "other".into());
                }
                _ => unreachable!(),
            }
            let supplied = bad.operations.get(&receipt.operation_id).unwrap();
            assert!(view.verify_new_file_tags(&previous, &bad, supplied).is_err());
        }
        drop(fixture.engine);
        let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
        assert_exit_visibility(
            &engine,
            &fixture.repository,
            &fixture.hidden,
            &fixture.config.disks[0],
            detached,
        );
        let snapshot = engine
            .file_container_tags(&storage_contract::file_container_tags::ReadRequest {
                file_id: request.file_id,
                context: request.context.clone(),
            })
            .unwrap();
        assert_eq!(snapshot.tags, request.tags);
        assert_eq!(snapshot.generation, storage_contract::Generation::new(1));
    }
}

#[test]
fn registry_activation_replay_requires_exact_base_and_actual_d3_before_tags() {
    use crate::journal::{Candidate, Publication, Receipt, RecordType, scan_configured_at};
    use crate::publication_evidence::{verified_object_at, verified_recovery_snapshot_at};

    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let engine = &mut fixture.engine;
    let generation = engine.config.generation().unwrap().clone();
    let online = engine
        .config
        .metadata_members()
        .cloned()
        .collect::<Vec<_>>();
    let mut receipts = Vec::new();
    for disk in &online {
        let root = &engine.disk_resources.get(disk).unwrap().root;
        let scan = scan_configured_at(root, disk, &engine.config).unwrap();
        let (record, frame_hash) = scan
            .records
            .into_iter()
            .find(|(record, _)| {
                record.record_type == RecordType::Publication
                    && record.sequence == generation.first_sequence().unwrap()
            })
            .unwrap();
        receipts.push(Receipt {
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            frame_hash,
            record,
        });
    }
    let candidate: Candidate = verified_object_at(
        &engine.disk_resources,
        &online,
        "candidate",
        &receipts[0].record.candidate_hash,
    )
    .unwrap();
    let publication: Publication = verified_object_at(
        &engine.disk_resources,
        &online,
        "publication",
        &receipts[0].record.object_hash,
    )
    .unwrap();
    let snapshot = verified_recovery_snapshot_at(
        &engine.disk_resources,
        &online,
        &candidate.snapshot_hash,
    )
    .unwrap();
    let original = snapshot.for_candidate(&candidate).unwrap();
    let live_state = Arc::clone(&engine.state);
    let live_snapshot = engine.snapshot.clone();
    let live_view = engine.current_namespaces.take();
    engine.state = Arc::clone(&generation.base);
    engine.snapshot = generation.snapshot();

    // Repair admission is pure and does not authorize the first Current view.
    engine
        .verify_replay_namespace_candidate(&candidate, original, Some(&publication))
        .unwrap();
    assert!(engine.current_namespaces.is_none());
    assert_eq!(engine.state, generation.base);
    assert!(
        engine.verify_current_recovery_candidate(&candidate, original.state()).is_err()
    );
    engine.snapshot = None;
    assert!(
        engine
            .verify_replay_namespace_candidate(
                &candidate,
                original,
                Some(&publication),
            )
            .is_err()
    );
    engine.snapshot = generation.snapshot();
    engine.state = Arc::clone(&live_state);
    assert!(
        engine
            .verify_replay_namespace_candidate(
                &candidate,
                original,
                Some(&publication),
            )
            .is_err()
    );

    engine.state = live_state;
    engine.snapshot = live_snapshot;
    assert!(live_view.is_some());
    assert!(engine.current_namespaces.is_none());
    // Final Current admission must read the real publication, actual D3 journal,
    // and full seal independently. Retaining the other two cannot replace one.
    let disk = &online[0];
    let paths = [
        crate::journal::object_path(
            disk,
            "publication",
            &receipts[0].record.object_hash,
        ),
        crate::engine::membership_transition::pending::activation::log_path(
            &engine.config,
            disk,
        ),
        crate::journal::publication_seal_path(disk, &receipts[0].record.candidate_hash),
    ];
    let intact = files(fixture.temp.path());
    for (index, path) in paths.iter().enumerate() {
        let parked = fixture.temp.path().join(format!("retained-d3-evidence-{index}"));
        fs::rename(path, &parked).unwrap();
        let missing = files(fixture.temp.path());
        assert!(engine.restore_current_disk_exit().is_err());
        assert!(engine.current_namespaces.is_none());
        assert!(engine.is_read_only());
        assert_eq!(files(fixture.temp.path()), missing);
        fs::rename(parked, path).unwrap();
        assert_eq!(files(fixture.temp.path()), intact);
    }
    engine.restore_current_disk_exit().unwrap();
    assert!(engine.current_namespaces.is_some());
    assert_eq!(files(fixture.temp.path()), intact);
    drop(snapshot);
    drop(fixture.engine);
    let reopened = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert!(reopened.current_namespaces.is_some());
    assert!(!reopened.is_read_only());
}
