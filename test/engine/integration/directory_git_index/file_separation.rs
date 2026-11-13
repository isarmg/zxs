//! Explicit file identity separation through the real batch/publication APIs.
//! These tests do not require or claim a flat physical-layout executor.
use super::*;
use std::os::unix::fs::MetadataExt;
use storage_contract::{
    BranchName, DirectoryBatchKind, DirectoryNamespaceBatch, DirectoryNamespaceItem,
    DirectoryNamespaceSource, DirectoryNamespaceTarget, Entry,
};

fn main_id(engine: &Engine, repo: &Repository, path: &str) -> String {
    let directory = status(engine, repo, path).directory_id;
    engine.state().directory_git_repositories[&directory]
        .main_branch_id
        .clone()
}

fn entry(engine: &Engine, branch: &str, path: &str) -> Entry {
    engine.state().directory_branches[branch]
        .entries
        .iter()
        .find(|entry| entry.path() == path)
        .unwrap_or_else(|| panic!("missing {path} in branch {branch}"))
        .clone()
}

fn branch(engine: &mut Engine, repo: &Repository, path: &str, name: &str) -> String {
    let path = (!path.is_empty()).then(|| name_policy::LookupPath::parse(path).unwrap());
    engine
        .create_directory_branches(
            &repo.repository_id,
            path.as_ref(),
            &BranchName::parse(name).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .into()
}

fn mapping(
    engine: &Engine,
    source: &str,
    source_path: &str,
    target: &str,
    target_path: &str,
) -> DirectoryNamespaceItem {
    DirectoryNamespaceItem {
        source: DirectoryNamespaceSource {
            directory_branch_id: source.into(),
            expected_tree_id: engine.state().directory_branches[source]
                .head_tree_id
                .clone(),
            path: source_path.into(),
            entry_id: entry(engine, source, source_path).entry_id,
        },
        target: Some(DirectoryNamespaceTarget {
            directory_branch_id: target.into(),
            expected_tree_id: engine.state().directory_branches[target]
                .head_tree_id
                .clone(),
            path: target_path.into(),
        }),
    }
}

fn apply(
    engine: &mut Engine,
    repo: &Repository,
    kind: DirectoryBatchKind,
    items: Vec<DirectoryNamespaceItem>,
) -> (DirectoryNamespaceBatch, OperationId, OperationResult) {
    let request = DirectoryNamespaceBatch { kind, items };
    let operation = OperationId::generate().unwrap();
    let result = engine
        .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
        .unwrap();
    (request, operation, result)
}

// Produce a real existing alias using the older Move behavior, without injecting
// State or picking an original by sort order. One frozen branch is retained at
// a different path so separation must not rewrite its file identity/history.
fn aliases(engine: &mut Engine, repo: &Repository) -> (String, String) {
    save(engine, repo, "a/report.txt", b"frozen secondary body");
    save(engine, repo, "b/keep", b"unrelated");
    initialize(engine, repo, "");
    commit(engine, repo, "");
    let main = main_id(engine, repo, "");
    let transfer = branch(engine, repo, "", "transfer");
    let other = branch(engine, repo, "", "other branch");
    let items = vec![mapping(
        engine,
        &other,
        "a/report.txt",
        &other,
        "a/renamed.txt",
    )];
    apply(engine, repo, DirectoryBatchKind::Rename, items);
    save(engine, repo, "a/report.txt", b"latest retained save");
    let items = vec![mapping(
        engine,
        &transfer,
        "a/report.txt",
        &main,
        "b/report.txt",
    )];
    apply(engine, repo, DirectoryBatchKind::Move, items);
    assert_eq!(
        entry(engine, &main, "a/report.txt").file_id,
        entry(engine, &main, "b/report.txt").file_id
    );
    (main, other)
}

fn assert_fresh(engine: &Engine, original: &Entry, separated: &Entry, body: &[u8]) {
    assert_ne!(original.entry_id, separated.entry_id);
    assert_ne!(original.file_id, separated.file_id);
    assert_ne!(original.version_id, separated.version_id);
    assert_eq!(version_bytes(engine, &separated.version_id), body);
    let history = engine.file_history(&separated.file_id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].version_id, separated.version_id);
    assert!(history[0].source_version_ids.is_empty());
    assert!(
        engine
            .file_node_parents(&separated.file_id, &separated.version_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .file_node_version(&separated.file_id, &original.version_id)
            .is_err()
    );
}

fn payload(engine: &Engine, repo: &Repository, entry: &Entry) -> std::path::PathBuf {
    let primary = engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap();
    primary
        .root
        .join("histories")
        .join(&repo.history_tree_id)
        .join("file-containers")
        .join(&entry.file_id)
        .join("namespace")
        .join(&engine.state().file_locations[&entry.file_id])
        .join("versions")
        .join(format!(
            "{}-{}",
            engine.state().versions[&entry.version_id].storage_id,
            std::path::Path::new(&engine.state().file_locations[&entry.file_id])
                .file_name().unwrap().to_str().unwrap(),
        ))
}

#[test]
fn same_main_separation_retains_original_history_and_uses_secondary_body_after_restart() {
    let (temp, mut engine, repo) = fixture();
    let (main, other) = aliases(&mut engine, &repo);
    let original = entry(&engine, &main, "a/report.txt");
    let secondary = entry(&engine, &main, "b/report.txt");
    assert_ne!(original.version_id, secondary.version_id);
    let history = engine.file_history(&original.file_id).unwrap();
    let other_before = engine.state().directory_branches[&other].clone();
    let git_before = engine.state().directory_git_repositories.clone();
    let items = vec![mapping(
        &engine,
        &main,
        "a/report.txt",
        &main,
        "b/report.txt",
    )];
    let (request, operation, receipt) =
        apply(&mut engine, &repo, DirectoryBatchKind::Separate, items);
    let fresh = entry(&engine, &main, "b/report.txt");
    assert_fresh(&engine, &secondary, &fresh, b"frozen secondary body");
    assert_eq!(
        receipt.result["copies"][0]["source_version_id"],
        secondary.version_id
    );
    let context = &engine.state().directory_branches[&main].file_contexts[fresh.lookup_key()];
    assert!(context.node_id.is_none());
    assert!(context.inherited_from.is_none());
    let original_stat = std::fs::metadata(payload(&engine, &repo, &original)).unwrap();
    let fresh_stat = std::fs::metadata(payload(&engine, &repo, &fresh)).unwrap();
    assert_ne!(
        (original_stat.dev(), original_stat.ino()),
        (fresh_stat.dev(), fresh_stat.ino())
    );
    assert_eq!(original_stat.nlink(), 1);
    assert_eq!(fresh_stat.nlink(), 1);
    save(
        &mut engine,
        &repo,
        "b/report.txt",
        b"independent later edit",
    );
    let edited = entry(&engine, &main, "b/report.txt");
    assert_eq!(edited.file_id, fresh.file_id);
    assert_eq!(engine.file_history(&fresh.file_id).unwrap().len(), 2);
    for _ in 0..2 {
        let before = storage_engine::canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
                .unwrap()
                .result,
            receipt.result
        );
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            before
        );
        assert_eq!(entry(&engine, &main, "a/report.txt"), original);
        assert_eq!(engine.file_history(&original.file_id).unwrap(), history);
        assert_eq!(engine.state().directory_branches[&other], other_before);
        assert_eq!(engine.state().directory_git_repositories, git_before);
        assert_eq!(
            bytes(&engine, &repo, "a/report.txt"),
            b"latest retained save"
        );
        assert_eq!(
            bytes(&engine, &repo, "b/report.txt"),
            b"independent later edit"
        );
        drop(engine);
        engine = Engine::open(&temp.path().join("system")).unwrap();
    }
}

#[test]
fn explicit_retained_occurrence_can_be_the_later_path_without_copying_history() {
    let (temp, mut engine, repo) = fixture();
    let (main, _) = aliases(&mut engine, &repo);
    let retained = entry(&engine, &main, "b/report.txt");
    let replaced = entry(&engine, &main, "a/report.txt");
    let history = engine.file_history(&retained.file_id).unwrap();
    let items = vec![mapping(
        &engine,
        &main,
        "b/report.txt",
        &main,
        "a/report.txt",
    )];
    apply(&mut engine, &repo, DirectoryBatchKind::Separate, items);
    let fresh = entry(&engine, &main, "a/report.txt");
    assert_fresh(&engine, &replaced, &fresh, b"latest retained save");
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(entry(&engine, &main, "b/report.txt"), retained);
    assert_eq!(engine.file_history(&retained.file_id).unwrap(), history);
    assert_fresh(
        &engine,
        &replaced,
        &entry(&engine, &main, "a/report.txt"),
        b"latest retained save",
    );
}

#[test]
fn two_child_main_repositories_separate_without_rewriting_parent_pins_or_reviewed_index() {
    let (temp, mut engine, repo) = fixture();
    aliases(&mut engine, &repo);
    initialize(&mut engine, &repo, "a");
    let left_head = commit(&mut engine, &repo, "a");
    initialize(&mut engine, &repo, "b");
    let right_head = commit(&mut engine, &repo, "b");
    stage_child(&mut engine, &repo, "", "a");
    stage_child(&mut engine, &repo, "", "b");
    let parent_head = commit(&mut engine, &repo, "");
    let parent = engine.state().directory_trees[&parent_head].clone();
    let left = main_id(&engine, &repo, "a");
    let right = main_id(&engine, &repo, "b");
    save(&mut engine, &repo, "a/report.txt", b"left uncommitted save");
    let original = entry(&engine, &left, "a/report.txt");
    let secondary = entry(&engine, &right, "b/report.txt");
    assert_eq!(original.file_id, secondary.file_id);
    let history = engine.file_history(&original.file_id).unwrap();
    let git = engine.state().directory_git_repositories.clone();
    let items = vec![mapping(
        &engine,
        &left,
        "a/report.txt",
        &right,
        "b/report.txt",
    )];
    let (request, operation, receipt) =
        apply(&mut engine, &repo, DirectoryBatchKind::Separate, items);
    for _ in 0..2 {
        assert_fresh(
            &engine,
            &secondary,
            &entry(&engine, &right, "b/report.txt"),
            b"frozen secondary body",
        );
        assert_eq!(entry(&engine, &left, "a/report.txt"), original);
        assert_eq!(engine.file_history(&original.file_id).unwrap(), history);
        assert_eq!(engine.state().directory_git_repositories, git);
        assert_eq!(engine.state().directory_trees[&parent_head], parent);
        assert_eq!(
            status(&engine, &repo, "a").head_tree_id.as_deref(),
            Some(left_head.as_str())
        );
        assert_eq!(
            status(&engine, &repo, "b").head_tree_id.as_deref(),
            Some(right_head.as_str())
        );
        assert_eq!(
            engine
                .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
                .unwrap()
                .result,
            receipt.result
        );
        drop(engine);
        engine = Engine::open(&temp.path().join("system")).unwrap();
    }
}

fn rejected_without_state_change(
    engine: &mut Engine,
    repo: &Repository,
    request: &DirectoryNamespaceBatch,
    code: ErrorCode,
) {
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .directory_namespace_batch(
                &repo.repository_id,
                request,
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        code
    );
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn ambiguous_or_stale_separation_rejects_without_publishing_a_partial_file() {
    let (temp, mut engine, repo) = fixture();
    let (main, other) = aliases(&mut engine, &repo);
    save(
        &mut engine,
        &repo,
        "independent.txt",
        b"frozen secondary body",
    );
    let baseline = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Separate,
        items: vec![mapping(
            &engine,
            &main,
            "a/report.txt",
            &main,
            "b/report.txt",
        )],
    };
    let mut invalid = baseline.clone();
    invalid.items.push(invalid.items[0].clone());
    rejected_without_state_change(&mut engine, &repo, &invalid, ErrorCode::InvalidRequest);
    for (source, source_path, target, target_path, code) in [
        (
            &main,
            "a/report.txt",
            &main,
            "a/report.txt",
            ErrorCode::BatchMappingConflict,
        ),
        (
            &main,
            "a/report.txt",
            &main,
            "independent.txt",
            ErrorCode::BatchMappingConflict,
        ),
        (
            &other,
            "a/renamed.txt",
            &main,
            "b/report.txt",
            ErrorCode::DirectoryBranchConflict,
        ),
        (
            &main,
            "a/report.txt",
            &main,
            "B/report.txt",
            ErrorCode::BatchMappingConflict,
        ),
    ] {
        let request = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Separate,
            items: vec![mapping(&engine, source, source_path, target, target_path)],
        };
        rejected_without_state_change(&mut engine, &repo, &request, code);
    }
    save(&mut engine, &repo, "b/keep", b"baseline changed");
    rejected_without_state_change(
        &mut engine,
        &repo,
        &baseline,
        ErrorCode::DirectoryBranchConflict,
    );
    let current = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Separate,
        items: vec![mapping(
            &engine,
            &main,
            "a/report.txt",
            &main,
            "b/report.txt",
        )],
    };
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "b/report.txt", b"unfinished save")
        .unwrap();
    rejected_without_state_change(&mut engine, &repo, &current, ErrorCode::WorkspaceBusy);
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
    assert_eq!(
        entry(&engine, &main, "a/report.txt").file_id,
        entry(&engine, &main, "b/report.txt").file_id
    );
    assert_eq!(
        bytes(&engine, &repo, "b/report.txt"),
        b"frozen secondary body"
    );
}

#[test]
fn current_registry_separation_admits_exact_replacement_and_cold_idempotent_retry() {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..4).map(|index| temp.path().join(format!("disk-{index}")))).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let exiting = config.disks[0].disk_id.clone();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_disk_root("D04", "admin", OperationId::generate().unwrap())
        .unwrap();
    let (main, _) = aliases(&mut engine, &repo);
    engine
        .begin_disk_exit(
            &exiting,
            storage_engine::MembershipExitAction::Deregister,
            "admin",
            OperationId::generate().unwrap(),
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
    assert_eq!(
        engine.disk_exit_status().unwrap().unwrap().phase,
        storage_engine::DiskExitPhase::Published
    );
    // Explicitly review the retained current composition after the registry
    // transition, just as the current-generation directory workflow requires.
    commit(&mut engine, &repo, "");
    let original = entry(&engine, &main, "a/report.txt");
    let secondary = entry(&engine, &main, "b/report.txt");
    let history = engine.file_history(&original.file_id).unwrap();
    let git = engine.state().directory_git_repositories.clone();
    let items = vec![mapping(
        &engine,
        &main,
        "a/report.txt",
        &main,
        "b/report.txt",
    )];
    let (request, operation, receipt) =
        apply(&mut engine, &repo, DirectoryBatchKind::Separate, items);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_fresh(
        &engine,
        &secondary,
        &entry(&engine, &main, "b/report.txt"),
        b"frozen secondary body",
    );
    assert_eq!(entry(&engine, &main, "a/report.txt"), original);
    assert_eq!(engine.file_history(&original.file_id).unwrap(), history);
    assert_eq!(engine.state().directory_git_repositories, git);
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
            .unwrap()
            .result,
        receipt.result
    );
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}
