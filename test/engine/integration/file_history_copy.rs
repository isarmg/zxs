//! Actual online publication on independent temporary roots, including current
//! registry activation and cold replay. This is not SMB transport certification.
use storage_engine as engine_api;
#[path = "../support/container_tags.rs"]
mod container_tags_fixture;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::unix::fs::MetadataExt,
};
use storage_contract::{
    history_window::HistoryWindowCancellation, shared_history::SharedHistoryRequest,
};
use storage_engine::*;

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
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
            message: "history copy fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
fn bytes(engine: &Engine, version: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine
        .open_content(version)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn entry(engine: &Engine, repo: &Repository, path: &str) -> Entry {
    engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}
fn request(engine: &Engine, source: &Repository, target: &Repository) -> FileHistoryCopyRequest {
    FileHistoryCopyRequest {
        source_repository_id: source.repository_id.clone(),
        source_path: "source.txt".into(),
        expected_source_head: engine.state().branches[&source.main_branch_id]
            .head_commit_id
            .clone(),
        target_repository_id: target.repository_id.clone(),
        target_name: "copied.txt".into(),
        expected_target_head: engine.state().branches[&target.main_branch_id]
            .head_commit_id
            .clone(),
    }
}
fn assert_independent_graph(engine: &Engine, repository: &Repository, file: &str) {
    let request = SharedHistoryRequest::parse(
        file,
        engine
            .metadata_view()
            .history_root(&repository.history_tree_id)
            .unwrap(),
    )
    .unwrap();
    let read = engine
        .capture_shared_history(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let graph = read.graph().unwrap();
    engine.recheck_shared_history(&read).unwrap();
    graph.validate(&request).unwrap();
    assert_eq!(graph.history_tree_id.as_str(), repository.history_tree_id);
    assert_eq!(graph.files.len(), 1);
    assert_eq!(graph.files[0].file_id.as_str(), file);
    assert_eq!(
        graph.versions.len(),
        engine.file_history(file).unwrap().len()
    );
    assert!(
        graph
            .versions
            .iter()
            .all(|version| version.file_id.as_str() == file)
    );
}
struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    source: Repository,
    target: Repository,
}
impl Fixture {
    fn new(current: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..5).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        for disk in config.disks.iter_mut().skip(3) {
            disk.level = 1;
            disk.upstream_slot_id = None;
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let source = engine.create_disk_root("P01", "admin", op()).unwrap();
        let target = engine.create_disk_root("D04", "admin", op()).unwrap();
        assert!(source.file_manager_root && target.file_manager_root);
        save(&mut engine, &source, "source.txt", b"old full body");
        save(&mut engine, &source, "source.txt", b"new full body");
        if current {
            engine
                .begin_disk_exit(
                    &config.disks[4].disk_id,
                    MembershipExitAction::Deregister,
                    "admin",
                    op(),
                )
                .unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                .unwrap();
            assert!(
                engine
                    .resume_membership_joint_decision()
                    .unwrap()
                    .all_certificates_synchronized()
            );
            engine.activate_pending_disk_exit().unwrap();
        }
        Self {
            temp,
            engine,
            source,
            target,
        }
    }
}

#[test]
fn history_copy_keeps_all_bodies_branches_labels_notes_and_source_after_cold_replay() {
    for current in [false, true] {
        let Fixture {
            temp,
            mut engine,
            source,
            target,
        } = Fixture::new(current);
        let original = entry(&engine, &source, "source.txt");
        let versions = engine.file_history(&original.file_id).unwrap();
        let pending = engine
            .create_file_branch(
                &original.file_id,
                &versions[0].version_id,
                &BranchName::parse("review").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        // The pending branch already exists when these independent path facts arise.
        let initial_context = engine.file_context(&source.repository_id, "source.txt").unwrap();
        let initial_head = engine.file_branch_head(&initial_context.branch_id).unwrap();
        let mut original_paths = Vec::new();
        for (old, new) in [("source.txt", "renamed.txt"), ("renamed.txt", "source.txt")] {
            let renamed = engine
                .file_namespace(
                    &source.repository_id,
                    NamespaceOperation::parse(NamespaceOperationKind::Rename, old, Some(new))
                        .unwrap(),
                    "admin",
                    op(),
                )
                .unwrap();
            let facts = renamed.result["path_changes"].as_array().unwrap();
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0]["operation_id"], renamed.operation_id.to_string());
            assert_eq!(facts[0]["before"]["path"], old);
            assert_eq!(facts[0]["after"]["path"], new);
            original_paths.push(facts[0].clone());
        }
        let context = engine
            .file_context(&source.repository_id, "source.txt")
            .unwrap();
        let head = engine.file_branch_head(&context.branch_id).unwrap();
        assert_eq!(context, initial_context);
        assert_eq!(head, initial_head);
        assert!(engine.state().file_path_nodes.is_empty());
        container_tags_fixture::edit(&mut engine, &original.file_id, &["keep-label".into()], &[], "admin", op())
            .unwrap();
        engine
            .set_file_note(
                &original.file_id,
                None,
                &Note::parse("whole file note".into()).unwrap(),
                "admin",
                op(),
            )
            .unwrap();
        engine
            .set_file_note(
                &original.file_id,
                Some(&versions[0].version_id),
                &Note::parse("old version note".into()).unwrap(),
                "admin",
                op(),
            )
            .unwrap();
        let before = engine.state().clone();
        let request = request(&engine, &source, &target);
        let operation = op();
        let receipt = engine
            .copy_file_history(&request, "admin", operation)
            .unwrap();
        assert_eq!(receipt.result["source_retained"], true);
        let copied = entry(&engine, &target, "copied.txt");
        assert_ne!(copied.file_id, original.file_id);
        assert_eq!(entry(&engine, &source, "source.txt"), original);
        assert_independent_graph(&engine, &source, &original.file_id);
        assert_independent_graph(&engine, &target, &copied.file_id);
        let copies = engine.file_history(&copied.file_id).unwrap();
        assert_eq!(original_paths.len(), 2);
        let retained_paths = receipt.result["retained_lifecycle"]["path_changes"]
            .as_array().unwrap();
        assert_eq!(retained_paths.len(), original_paths.len());
        assert!(engine.state().file_path_nodes.is_empty());
        for original_fact in &original_paths {
            let retained = retained_paths.iter()
                .find(|fact| fact["source"]["event_id"] == original_fact["event_id"])
                .unwrap();
            // Source ID, actor, original paths, time and sequence are not remapped.
            assert_eq!(&retained["source"], original_fact);
            assert_eq!(retained["file_id"], copied.file_id);
            assert_eq!(retained["history_tree_id"], target.history_tree_id);
            let branch = &engine.state().branches[retained["branch_id"].as_str().unwrap()];
            assert_eq!(branch.file_id.as_deref(), Some(copied.file_id.as_str()));
            assert_eq!(branch.history_tree_id, target.history_tree_id);
            for field in ["before_version_id", "after_version_id"] {
                assert_eq!(retained[field], copied.version_id);
                assert_eq!(
                    engine.state().versions[retained[field].as_str().unwrap()].file_id,
                    copied.file_id,
                );
            }
        }

        assert_eq!(copies.len(), versions.len());
        for old in &versions {
            let copy = copies
                .iter()
                .find(|copy| copy.storage_name == old.storage_name)
                .unwrap();
            assert_eq!(copy.created_at, old.created_at);
            assert_eq!(copy.version_sequence, old.version_sequence);
            assert_eq!(copy.content_hash, old.content_hash);
            assert_eq!(
                bytes(&engine, &copy.version_id),
                bytes(&engine, &old.version_id)
            );
            let old_inode = engine
                .open_content(&old.version_id)
                .unwrap()
                .file
                .metadata()
                .unwrap();
            let new_inode = engine
                .open_content(&copy.version_id)
                .unwrap()
                .file
                .metadata()
                .unwrap();
            assert_ne!(
                (old_inode.dev(), old_inode.ino()),
                (new_inode.dev(), new_inode.ino())
            );
            assert_eq!(
                engine.state().placements[&copy.version_id].primary_slot_id,
                "D04"
            );
        }
        for (id, branch) in before.branches.iter() {
            if branch.history_tree_id == source.history_tree_id {
                assert_eq!(engine.state().branches.get(id), Some(branch));
            }
        }
        for (id, node) in before.file_metadata_nodes.iter() {
            assert_eq!(engine.state().file_metadata_nodes.get(id), Some(node));
        }
        assert_eq!(
            engine.state().file_notes[&copied.file_id],
            "whole file note"
        );
        assert_eq!(engine.state().branches[&pending], before.branches[&pending]);
        let copied_branches = engine
            .state()
            .branches
            .values()
            .filter(|branch| branch.file_id.as_deref() == Some(&copied.file_id))
            .collect::<Vec<_>>();
        assert_eq!(
            copied_branches
                .iter()
                .map(|branch| branch.name.as_str())
                .collect::<BTreeSet<_>>(),
            ["main", "review"].into()
        );
        let pending_copy = copied_branches
            .iter()
            .find(|branch| branch.name == "review")
            .unwrap();
        assert!(
            pending_copy
                .pending_file_versions
                .contains_key(&copied.file_id)
        );
        let copied_context = engine
            .file_context(&target.repository_id, "copied.txt")
            .unwrap();
        assert_eq!(
            engine.file_branch_head(&copied_context.branch_id).unwrap(),
            copied.version_id,
        );
        assert!(
            container_tags_fixture::tags(&engine, &copied.file_id)
                .contains("keep-label")
        );
        let copied_id = copied.file_id.parse::<storage_contract::FileId>().unwrap();
        assert_eq!(
            engine.state().container_tags.get(&copied_id).unwrap().generation(),
            storage_contract::Generation::new(0),
        );
        container_tags_fixture::edit(
            &mut engine,
            &copied.file_id,
            &["copy-only".into()],
            &["keep-label".into()],
            "admin",
            op(),
        ).unwrap();
        assert_eq!(
            container_tags_fixture::tags(&engine, &original.file_id),
            BTreeSet::from(["keep-label".into()]),
        );
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(entry(&engine, &target, "copied.txt"), copied);
        assert_independent_graph(&engine, &target, &copied.file_id);
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", operation)
                .unwrap(),
            receipt
        );
        save(
            &mut engine,
            &target,
            "copied.txt",
            b"independent target edit",
        );
        assert_eq!(bytes(&engine, &original.version_id), b"new full body");
        save(&mut engine, &source, "source.txt", b"later source edit");
        let before_replay = engine.state().clone();
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", operation)
                .unwrap(),
            receipt
        );
        assert_eq!(*engine.state(), before_replay);
    }
}

#[test]
fn history_copy_failure_boundaries_keep_source_and_recover_one_operation() {
    for (fault, decided) in [
        ("after_prepare_1", false),
        ("after_accept_1", true),
        ("after_accept_2", true),
        ("after_container_names", true),
    ] {
        let Fixture {
            temp,
            mut engine,
            source,
            target,
        } = Fixture::new(true);
        let original = entry(&engine, &source, "source.txt");
        let request = request(&engine, &source, &target);
        let operation = op();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", operation)
                .unwrap_err()
                .code(),
            if decided {
                ErrorCode::InDoubt
            } else {
                ErrorCode::InjectedFailure
            }
        );
        assert_eq!(entry(&engine, &source, "source.txt"), original);
        assert!(
            engine
                .namespace_tree(&target.main_branch_id)
                .unwrap()
                .entries()
                .is_empty()
        );
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(entry(&engine, &source, "source.txt"), original);
        assert_eq!(bytes(&engine, &original.version_id), b"new full body");
        assert_eq!(
            engine
                .namespace_tree(&target.main_branch_id)
                .unwrap()
                .get("copied.txt")
                .is_some(),
            decided
        );
        let receipt = engine
            .copy_file_history(&request, "admin", operation)
            .unwrap();
        let after = engine.state().clone();
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", operation)
                .unwrap(),
            receipt
        );
        assert_eq!(*engine.state(), after);
    }
}

#[test]
fn history_copy_rejects_busy_changed_or_nonempty_destinations_without_publication() {
    let Fixture {
        mut engine,
        source,
        target,
        temp: _temp,
    } = Fixture::new(true);
    let request = request(&engine, &source, &target);
    let workspace = engine
        .create_workspace(&source.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "source.txt", b"unsaved")
        .unwrap();
    let before = engine.state().clone();
    assert_eq!(
        engine
            .copy_file_history(&request, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceBusy
    );
    assert_eq!(*engine.state(), before);
    // No request may silently overwrite a destination or redirect an old head.
    save(&mut engine, &target, "occupied", b"retained target");
    let before = engine.state().clone();
    assert_eq!(
        engine
            .copy_file_history(&request, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(*engine.state(), before);
    let mut fresh = request.clone();
    fresh.expected_target_head = engine.state().branches[&target.main_branch_id]
        .head_commit_id
        .clone();
    assert_eq!(
        engine
            .copy_file_history(&fresh, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(*engine.state(), before);
}

#[test]
fn history_copy_rejects_configured_placement_or_backup_without_changing_the_source() {
    for case in [
        "source-backup",
        "target-backup",
        "source-placement",
        "target-placement",
    ] {
        let Fixture {
            temp,
            mut engine,
            source,
            target,
        } = Fixture::new(true);
        let repository = if case.starts_with("source") {
            &source
        } else {
            &target
        };
        if case.ends_with("placement") {
            engine
                .configure_placement(PlacementConfigurationRequest {
                    repository_id: &repository.repository_id,
                    path: "",
                    primary_slot_id: &repository.primary_slot_id,
                    expected_scope_id: &repository.repository_id,
                    expected_rule_id: None,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        } else {
            engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &repository.repository_id,
                    path: "",
                    target_copies: 1,
                    backup_prefixes: &BTreeMap::new(),
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let request = request(&engine, &source, &target);
        let before = engine.state().clone();
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", op())
                .unwrap_err()
                .code(),
            ErrorCode::UnsupportedOperation,
            "{case}"
        );
        assert_eq!(*engine.state(), before);
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(*engine.state(), before);
        assert_eq!(
            engine
                .copy_file_history(&request, "admin", op())
                .unwrap_err()
                .code(),
            ErrorCode::UnsupportedOperation,
            "{case} after cold open"
        );
        assert_eq!(*engine.state(), before);
    }
}

#[test]
fn history_copy_does_not_admit_a_fanout_topology_as_a_selected_linear_route() {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    config.disks[2].level = 2;
    config.disks[2].upstream_slot_id = Some(config.disks[0].slot_id.clone());
    assert_eq!(
        Engine::initialize(&temp.path().join("system"), config)
            .err()
            .unwrap()
            .code(),
        ErrorCode::TopologyInvalid
    );
}

#[test]
fn history_copy_preserves_consumed_branch_aliases_and_merge_frontier() {
    let Fixture {
        temp,
        mut engine,
        source,
        target,
    } = Fixture::new(true);
    let original = entry(&engine, &source, "source.txt");
    let versions = engine.file_history(&original.file_id).unwrap();
    let topic = engine
        .create_file_branch(
            &original.file_id,
            &versions[0].version_id,
            &BranchName::parse("consumed").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let main = engine
        .file_context(&source.repository_id, "source.txt")
        .unwrap()
        .branch_id;
    let main_head = engine.file_branch_head(&main).unwrap();
    let link = engine
        .add_merge_link(
            &main,
            &engine.state().branches[&main].head_commit_id.clone(),
            &topic,
            &engine.state().branches[&topic].head_commit_id.clone(),
            "admin",
            op(),
        )
        .unwrap();

    engine
        .merge_file_branches(FileBranchMergeRequest {
            file_id: &original.file_id,
            inputs: &[
                FileMergeInput {
                    branch_id: main.clone(),
                    expected_node_id: main_head.clone(),
                },
                FileMergeInput {
                    branch_id: topic.clone(),
                    expected_node_id: engine.file_branch_head(&topic).unwrap(),
                },
            ],
            chosen_node_id: &main_head,
            surviving_branch_id: &main,
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let source_merges = engine
        .state()
        .file_branch_merges
        .values()
        .filter(|merge| merge.file_id == original.file_id)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(source_merges.len(), 1);
    let request = request(&engine, &source, &target);
    engine.copy_file_history(&request, "admin", op()).unwrap();
    let copied = entry(&engine, &target, "copied.txt");
    let copied_branches = engine
        .state()
        .branches
        .values()
        .filter(|branch| branch.file_id.as_deref() == Some(&copied.file_id))
        .collect::<Vec<_>>();
    let copied_main = copied_branches
        .iter()
        .find(|branch| branch.name == "main")
        .unwrap();
    let consumed = copied_branches
        .iter()
        .find(|branch| branch.name == "consumed")
        .unwrap();
    assert_eq!(
        engine.state().file_branch_aliases[&consumed.branch_id],
        copied_main.branch_id
    );
    let merges = engine
        .state()
        .file_branch_merges
        .values()
        .filter(|merge| merge.file_id == copied.file_id)
        .collect::<Vec<_>>();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0].sequence, source_merges[0].sequence);
    assert_eq!(merges[0].reused_node, source_merges[0].reused_node);
    assert_eq!(merges[0].surviving_branch_id, copied_main.branch_id);
    let links = engine
        .state()
        .merge_links
        .values()
        .filter(|link| link.history_tree_id == target.history_tree_id)
        .collect::<Vec<_>>();
    assert_eq!(links.len(), 1);
    assert_ne!(links[0].merge_link_id, link.merge_link_id);
    assert_eq!(links[0].source_branch_id, copied_main.branch_id);
    assert_eq!(links[0].target_branch_id, consumed.branch_id);
    assert_eq!(links[0].operation_id, link.operation_id);

    assert!(
        merges[0]
            .input_node_ids
            .iter()
            .all(|node| !source_merges[0].input_node_ids.contains(node))
    );
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(entry(&engine, &target, "copied.txt"), copied);
    assert_eq!(bytes(&engine, &copied.version_id), b"new full body");
}

#[path = "file_history_copy/file_databases.rs"]
mod file_databases;

#[path = "file_history_copy/move_capture.rs"]
mod move_capture;

#[path = "file_history_copy/move_recovery.rs"]
mod move_recovery;

#[path = "file_history_copy/move_target.rs"]
mod move_target;

#[path = "file_history_copy/move_decision.rs"]
mod move_decision;

#[path = "file_history_copy/move_native.rs"]
mod move_native;

#[path = "file_history_copy/move_scopes.rs"]
mod move_scopes;

#[path = "file_history_copy/move_choice.rs"]
mod move_choice;

#[path = "file_history_copy/move_candidate.rs"]
mod move_candidate;

#[path = "file_history_copy/move_ordinary.rs"]
mod move_ordinary;

#[path = "file_history_copy/move_execution.rs"]
mod move_execution;

#[path = "file_history_copy/lifecycle_page.rs"]
mod lifecycle_page;
