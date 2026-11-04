//! Real D3 publications and restart replay. These temporary directories may share
//! a filesystem; no physical device, deployment or backup restoration is implied.
use super::*;
use crate::{MembershipExitAction, PlacementRuleUpdate};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn note(value: &str) -> storage_contract::Note {
    storage_contract::Note::parse(value).unwrap()
}
fn label(value: &str) -> storage_contract::BranchName {
    storage_contract::BranchName::parse(value).unwrap()
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) {
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
            message: "retained metadata fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
fn files(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repo: Repository,
    hidden: Entry,
    live: Entry,
    old_live: Entry,
    hidden_branch: String,
    live_branch: String,
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
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("metadata").unwrap(),
                "D04",
                1,
                "admin",
                op(),
            )
            .unwrap();
        let rule = new_id("rule").unwrap();
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repo.main_branch_id,
            "dir/hidden.txt",
            b"exclusive old identity",
        );
        save(
            &mut engine,
            &repo.main_branch_id,
            "dir/live.txt",
            b"old body on exiting disk",
        );
        let old_live = engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("dir/live.txt")
            .unwrap()
            .clone();
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir",
                primary_slot_id: "D04",
                expected_generation: 1,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repo.main_branch_id,
            "dir/live.txt",
            b"current retained body",
        );
        let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
        let hidden = tree.get("dir/hidden.txt").unwrap().clone();
        let live = tree.get("dir/live.txt").unwrap().clone();
        assert_eq!(live.file_id, old_live.file_id);
        let hidden_branch = engine
            .file_context(&repo.repository_id, "dir/hidden.txt")
            .unwrap()
            .branch_id
            .clone();
        let live_branch = engine
            .file_context(&repo.repository_id, "dir/live.txt")
            .unwrap()
            .branch_id
            .clone();
        Self {
            temp,
            engine,
            config,
            repo,
            hidden,
            live,
            old_live,
            hidden_branch,
            live_branch,
        }
    }

    fn prepare_exit(&mut self, action: MembershipExitAction) {
        self.engine
            .begin_disk_exit(&self.config.disks[0].disk_id, action, "admin", op())
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        self.engine.resume_membership_joint_decision().unwrap();
    }

    fn exit(&mut self, action: MembershipExitAction) {
        let old = Arc::clone(
            self.engine
                .disk_resources
                .get(&self.config.disks[0])
                .unwrap(),
        );
        self.prepare_exit(action);
        let disk = &self.config.disks[0];
        let before = files(&disk.root);
        self.engine.activate_pending_disk_exit().unwrap();
        old.root.io_binding().unwrap().closed_proof().unwrap();
        assert_eq!(files(&disk.root), before);
        assert!(self.engine.disk_resources.available(disk).is_none());
    }

    fn park_target(&self) -> std::path::PathBuf {
        let parked = self.temp.path().join("retired-by-user");
        fs::rename(&self.config.disks[0].root, &parked).unwrap();
        fs::write(&self.config.disks[0].root, b"not a storage directory").unwrap();
        parked
    }
}

fn namespace_snapshot(engine: &Engine) -> BTreeMap<String, Vec<Entry>> {
    let mut snapshot = BTreeMap::new();
    for branch in engine.state.branches.values() {
        if branch.state != BranchState::Migrated {
            snapshot.insert(
                branch.branch_id.clone(),
                engine
                    .namespace_tree(&branch.branch_id)
                    .unwrap()
                    .entries()
                    .to_vec(),
            );
        }
    }
    for branch in engine.state.directory_branches.values() {
        snapshot.insert(
            branch.directory_branch_id.clone(),
            engine
                .current_directory_entries(&branch.directory_branch_id)
                .unwrap()
                .to_vec(),
        );
    }
    snapshot
}

fn immutable_content(engine: &Engine) -> json_codec::Value {
    let state = engine.state();
    let heads: BTreeMap<_, _> = state
        .branches
        .iter()
        .map(|(id, branch)| {
            (
                id,
                (
                    &branch.head_commit_id,
                    branch.generation,
                    &branch.file_node_id,
                ),
            )
        })
        .collect();
    json!({
        "versions": state.versions,
        "placements": state.placements,
        "commits": state.commits,
        "directory_trees": state.directory_trees,
        "directory_branches": state.directory_branches,
        "file_path_nodes": state.file_path_nodes,
        "file_metadata_nodes": state.file_metadata_nodes,
        "file_contexts": state.file_contexts,
        "file_selections": state.file_selections,
        "heads": heads,
    })
}

#[test]
fn retained_metadata_real_d3_edits_preserve_namespaces_and_restart_without_target_io() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        fixture.exit(action);
        let parked = fixture.park_target();
        let target_bytes = files(&parked);
        let namespaces = namespace_snapshot(&fixture.engine);
        let immutable = immutable_content(&fixture.engine);
        let operation = op();
        let file_result = fixture
            .engine
            .set_file_note(
                &fixture.live.file_id,
                None,
                &note("current file"),
                "admin",
                operation,
            )
            .unwrap();
        let version_result = fixture
            .engine
            .set_file_note(
                &fixture.old_live.file_id,
                Some(&fixture.old_live.version_id),
                &note("historical version"),
                "admin",
                op(),
            )
            .unwrap();
        let rename = fixture
            .engine
            .rename_file_branch(
                &fixture.live.file_id,
                &fixture.live_branch,
                &label("retained public label"),
                "admin",
                op(),
            )
            .unwrap();
        assert!(file_result.commit_id.is_none());
        assert!(version_result.commit_id.is_none());
        assert!(rename.commit_id.is_none());
        assert_eq!(namespace_snapshot(&fixture.engine), namespaces);
        assert_eq!(immutable_content(&fixture.engine), immutable);
        assert_eq!(files(&parked), target_bytes);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"not a storage directory"
        );
        let sequence = fixture.engine.state.sequence;
        assert_eq!(
            fixture
                .engine
                .set_file_note(
                    &fixture.live.file_id,
                    None,
                    &note("current file"),
                    "admin",
                    operation,
                )
                .unwrap(),
            file_result
        );
        assert_eq!(fixture.engine.state.sequence, sequence);
        drop(fixture.engine);
        let mut reopened = Engine::open(&fixture.temp.path().join("system")).unwrap();
        assert_eq!(namespace_snapshot(&reopened), namespaces);
        assert_eq!(immutable_content(&reopened), immutable);
        assert_eq!(
            reopened.state.file_notes[&fixture.live.file_id],
            "current file"
        );
        assert_eq!(
            reopened.state.version_notes[&fixture.old_live.version_id],
            "historical version"
        );
        assert_eq!(
            reopened.state.branches[&fixture.live_branch].name,
            "retained public label"
        );
        assert_eq!(
            reopened
                .set_file_note(
                    &fixture.live.file_id,
                    None,
                    &note("current file"),
                    "admin",
                    operation,
                )
                .unwrap(),
            file_result
        );
        assert_eq!(files(&parked), target_bytes);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"not a storage directory"
        );
    }
}

#[test]
fn retained_metadata_hidden_identity_keeps_history_and_old_receipts_but_not_current_edits() {
    let mut fixture = Fixture::new();
    let note_operation = op();
    let rename_operation = op();
    let original_note = fixture
        .engine
        .set_file_note(
            &fixture.hidden.file_id,
            None,
            &note("old file identity"),
            "admin",
            note_operation,
        )
        .unwrap();
    let original_rename = fixture
        .engine
        .rename_file_branch(
            &fixture.hidden.file_id,
            &fixture.hidden_branch,
            &label("old label"),
            "admin",
            rename_operation,
        )
        .unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    let parked = fixture.park_target();
    let target_bytes = files(&parked);
    let sequence = fixture.engine.state.sequence;
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.hidden.file_id,
                None,
                &note("old file identity"),
                "admin",
                note_operation,
            )
            .unwrap(),
        original_note
    );
    assert_eq!(
        fixture
            .engine
            .rename_file_branch(
                &fixture.hidden.file_id,
                &fixture.hidden_branch,
                &label("old label"),
                "admin",
                rename_operation,
            )
            .unwrap(),
        original_rename
    );
    assert_eq!(fixture.engine.state.sequence, sequence);
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.hidden.file_id,
                None,
                &note("wrong new edit"),
                "admin",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNotFound
    );
    assert_eq!(
        fixture
            .engine
            .rename_file_branch(
                &fixture.hidden.file_id,
                &fixture.hidden_branch,
                &label("wrong new label"),
                "admin",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchNotFound
    );
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.hidden.file_id,
                None,
                &note("different request"),
                "admin",
                note_operation,
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    fixture
        .engine
        .set_file_note(
            &fixture.hidden.file_id,
            Some(&fixture.hidden.version_id),
            &note("explicit historical note"),
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/hidden.txt",
        b"new identity",
    );
    let replacement = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("dir/hidden.txt")
        .unwrap()
        .clone();
    assert_ne!(replacement.file_id, fixture.hidden.file_id);
    assert_ne!(replacement.entry_id, fixture.hidden.entry_id);
    assert!(
        !fixture
            .engine
            .state
            .file_notes
            .contains_key(&replacement.file_id)
    );
    fixture
        .engine
        .set_file_note(
            &replacement.file_id,
            None,
            &note("replacement only"),
            "admin",
            op(),
        )
        .unwrap();
    assert_eq!(
        fixture.engine.state.file_notes[&fixture.hidden.file_id],
        "old file identity"
    );
    assert_eq!(
        fixture.engine.state.file_notes[&replacement.file_id],
        "replacement only"
    );
    assert_eq!(
        fixture.engine.state.version_notes[&fixture.hidden.version_id],
        "explicit historical note"
    );
    let sequence = fixture.engine.state.sequence;
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.hidden.file_id,
                None,
                &note("old file identity"),
                "admin",
                note_operation,
            )
            .unwrap(),
        original_note
    );
    assert_eq!(fixture.engine.state.sequence, sequence);
    assert_eq!(
        fixture.engine.state.file_notes[&replacement.file_id],
        "replacement only"
    );
    assert_eq!(files(&parked), target_bytes);
    let namespaces = namespace_snapshot(&fixture.engine);
    drop(fixture.engine);
    let reopened = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(namespace_snapshot(&reopened), namespaces);
    assert_eq!(
        reopened.state.file_notes[&fixture.hidden.file_id],
        "old file identity"
    );
    assert_eq!(
        reopened.state.file_notes[&replacement.file_id],
        "replacement only"
    );
    assert_eq!(files(&parked), target_bytes);
}

#[test]
fn retained_metadata_disabled_file_allows_metadata_without_enabling_payload_access() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::PlannedOffline);
    let parked = fixture.park_target();
    let target_bytes = files(&parked);
    let namespaces = namespace_snapshot(&fixture.engine);
    fixture
        .engine
        .set_file_note(
            &fixture.hidden.file_id,
            None,
            &note("offline file"),
            "admin",
            op(),
        )
        .unwrap();
    fixture
        .engine
        .rename_file_branch(
            &fixture.hidden.file_id,
            &fixture.hidden_branch,
            &label("offline label"),
            "admin",
            op(),
        )
        .unwrap();
    assert!(
        fixture
            .engine
            .open_content(&fixture.hidden.version_id)
            .is_err()
    );
    assert_eq!(namespace_snapshot(&fixture.engine), namespaces);
    assert_eq!(files(&parked), target_bytes);
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"not a storage directory"
    );
}

#[test]
fn retained_metadata_rejects_wrong_identity_actor_and_unrelated_missing_resources() {
    let mut fixture = Fixture::new();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.live.file_id,
                Some(&fixture.hidden.version_id),
                &note("wrong file"),
                "admin",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNotFound
    );
    assert_eq!(
        fixture
            .engine
            .set_file_note(&fixture.live.file_id, None, &note("no actor"), "", op(),)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    assert_eq!(
        fixture
            .engine
            .rename_file_branch(
                &fixture.live.file_id,
                &fixture.live_branch,
                &label("no actor"),
                "",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::Unauthenticated
    );
    // A missing admission resource is not a Disabled/Detached registration.
    fixture.engine.disk_resources = Arc::new(fixture.engine.disk_resources.excluding(
        &std::collections::BTreeSet::from([fixture.config.disks[3].disk_id.clone()]),
    ));
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.live.file_id,
                None,
                &note("missing retained body disk"),
                "admin",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::DiskUnavailable
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn retained_metadata_waits_for_real_target_io_closure() {
    let mut fixture = Fixture::new();
    let target = Arc::clone(
        fixture
            .engine
            .disk_resources
            .get(&fixture.config.disks[0])
            .unwrap(),
    );
    let tail = target.root.admit_io().unwrap();
    fixture.prepare_exit(MembershipExitAction::Deregister);
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::DiskUnavailable
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                &fixture.live.file_id,
                None,
                &note("not yet admitted"),
                "admin",
                op(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    drop(tail);
    fixture.engine.activate_pending_disk_exit().unwrap();
    target.root.io_binding().unwrap().closed_proof().unwrap();
    fixture
        .engine
        .set_file_note(
            &fixture.live.file_id,
            None,
            &note("after closure"),
            "admin",
            op(),
        )
        .unwrap();
}

#[cfg(feature = "fault-injection")]
#[test]
fn retained_metadata_decided_note_recovers_without_unmasking_or_opening_target() {
    for point in [
        "after_finalize",
        "after_container_names",
        "after_publication_1",
    ] {
        let mut fixture = Fixture::new();
        fixture.exit(MembershipExitAction::Deregister);
        let parked = fixture.park_target();
        let target_bytes = files(&parked);
        let namespaces = namespace_snapshot(&fixture.engine);
        let immutable = immutable_content(&fixture.engine);
        let operation = op();
        fixture.engine.inject_fault_once(point);
        assert_eq!(
            fixture
                .engine
                .set_file_note(
                    &fixture.hidden.file_id,
                    Some(&fixture.hidden.version_id),
                    &note("decided historical note"),
                    "admin",
                    operation,
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InDoubt,
            "{point}"
        );
        assert_eq!(namespace_snapshot(&fixture.engine), namespaces);
        drop(fixture.engine);
        let mut reopened = Engine::open(&fixture.temp.path().join("system")).unwrap();
        let sequence = reopened.state.sequence;
        let receipt = reopened
            .set_file_note(
                &fixture.hidden.file_id,
                Some(&fixture.hidden.version_id),
                &note("decided historical note"),
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(receipt.operation_id, operation);
        assert_eq!(reopened.state.sequence, sequence);
        assert_eq!(namespace_snapshot(&reopened), namespaces);
        assert_eq!(immutable_content(&reopened), immutable);
        assert_eq!(
            reopened.state.version_notes[&fixture.hidden.version_id],
            "decided historical note"
        );
        assert_eq!(files(&parked), target_bytes);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"not a storage directory"
        );
    }
}
