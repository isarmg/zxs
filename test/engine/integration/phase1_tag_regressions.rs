//! Real production Engine calls on disposable tmpfs roots; no save implementation
//! or phase-two model is substituted. HTTP/Samba are covered by other owners.
use std::{collections::BTreeSet, io::Read};
use storage_engine::*;
use storage_engine as engine_api;
use storage_contract::file_container_tags::{Context, ReadRequest, ReplaceCommand, Tag, TagSet};
#[path = "../support/container_tags.rs"]
mod container_tags_fixture;

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn save(e: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> Entry {
    let w = e.create_workspace(branch, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, bytes).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: op(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "phase-one independent tag regression".into(),
        request_digest: None,
    })
    .unwrap();
    e.namespace_tree(branch).unwrap().get(path).unwrap().clone()
}
fn command(e: &Engine, repo: &Repository, file: &str, path: &str, tags: &[&str]) -> ReplaceCommand {
    let read = ReadRequest {
        file_id: file.parse().unwrap(),
        context: Context {
            repository_id: repo.repository_id.parse().unwrap(),
            path: name_policy::PortablePath::parse(path).unwrap(),
        },
    };
    ReplaceCommand {
        expected_generation: e.file_container_tags(&read).unwrap().generation,
        file_id: read.file_id,
        context: read.context,
        tags: TagSet::new(tags.iter().map(|tag| Tag::parse((*tag).into()).unwrap())).unwrap(),
    }
}
fn edit(e: &mut Engine, request: &ReplaceCommand, operation: OperationId) -> Result<OperationResult> {
    e.replace_container_tags(request, &"admin".parse().unwrap(), operation)
}
fn bytes(e: &Engine, version: &str) -> Vec<u8> {
    let mut opened = e.open_content(version).unwrap();
    let mut result = Vec::new();
    opened.file.read_to_end(&mut result).unwrap();
    result
}
fn namespace(
    e: &mut Engine,
    repo: &str,
    kind: NamespaceOperationKind,
    path: &str,
    to: Option<&str>,
) {
    e.file_namespace(
        repo,
        NamespaceOperation::parse(kind, path, to).unwrap(),
        "admin",
        op(),
    )
    .unwrap();
}
fn selected_tree(e: &Engine, repo: &str) -> String {
    e.state()
        .directory_tree_selections
        .get(repo)
        .cloned()
        .unwrap_or_else(|| {
            let branch = &e.state().directory_selections[repo];
            e.state().directory_branches[branch].head_tree_id.clone()
        })
}

#[test]
fn container_replacement_keeps_content_graph_and_replays_exact_cas() {
    let temp = tempfile::tempdir().unwrap();
    let mut e = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repo = e.create_disk_root("P01", "admin", op()).unwrap();
    let first = save(&mut e, &repo.main_branch_id, "报告.txt", b"first body");
    let request = command(&e, &repo, &first.file_id, "报告.txt", &["审核"]);
    let operation = op();
    let before = e.state().clone();
    let receipt = edit(&mut e, &request, operation).unwrap();
    assert_eq!(e.state().versions, before.versions);
    assert_eq!(e.state().file_metadata_nodes, before.file_metadata_nodes);
    assert_eq!(e.state().branches, before.branches);
    assert_eq!(e.state().commits, before.commits);
    assert_eq!(container_tags_fixture::tags(&e, &first.file_id), BTreeSet::from(["审核".into()]));
    let second = save(&mut e, &repo.main_branch_id, "报告.txt", b"second body");
    assert_eq!(second.file_id, first.file_id);
    assert_eq!(
        e.file_node_parents(&first.file_id, &second.version_id).unwrap(),
        vec![first.version_id.clone()],
    );
    drop(e);
    let mut e = Engine::open(&temp.path().join("system")).unwrap();
    let before = e.state().clone();
    assert_eq!(edit(&mut e, &request, operation).unwrap(), receipt);
    assert_eq!(e.state(), &before);
    let mut changed = request.clone();
    changed.tags = TagSet::empty();
    assert_eq!(
        edit(&mut e, &changed, operation).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused,
    );
    assert_eq!(edit(&mut e, &request, op()).unwrap_err().code(), ErrorCode::HeadConflict);
    assert_eq!(e.state(), &before);
    assert_eq!(bytes(&e, &first.version_id), b"first body");
    assert_eq!(bytes(&e, &second.version_id), b"second body");
}

#[test]
fn reused_old_path_does_not_retarget_container_tags_or_break_exact_replay() {
    let temp = tempfile::tempdir().unwrap();
    let mut e = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repo = e.create_disk_root("P01", "admin", op()).unwrap();
    let a = save(&mut e, &repo.main_branch_id, "a.txt", b"original");
    let request = command(&e, &repo, &a.file_id, "a.txt", &["saved"]);
    let operation = op();
    let receipt = edit(&mut e, &request, operation).unwrap();
    namespace(
        &mut e,
        &repo.repository_id,
        NamespaceOperationKind::Rename,
        "a.txt",
        Some("moved.txt"),
    );
    let b = save(&mut e, &repo.main_branch_id, "a.txt", b"new independent file");
    assert_ne!(a.file_id, b.file_id);
    let before = e.state().clone();
    assert_eq!(edit(&mut e, &request, operation).unwrap(), receipt);
    assert_eq!(edit(&mut e, &request, op()).unwrap_err().code(), ErrorCode::FileNotFound);
    assert_eq!(e.state(), &before);
    assert!(container_tags_fixture::tags(&e, &b.file_id).is_empty());
    assert_eq!(container_tags_fixture::tags(&e, &a.file_id), BTreeSet::from(["saved".into()]));
    drop(e);
    let e = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        e.namespace_tree(&repo.main_branch_id).unwrap().get("a.txt").unwrap().file_id,
        b.file_id,
    );
    assert!(container_tags_fixture::tags(&e, &b.file_id).is_empty());
    assert_eq!(bytes(&e, &b.version_id), b"new independent file");
}

#[test]
fn current_tags_ignore_retired_directory_identity_and_keep_root_and_history() {
    for recreate in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let removed_disk = config.disks[0].disk_id.clone();
        let mut e = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = e.create_disk_root("D04", "admin", op()).unwrap();
        let file = save(&mut e, &repo.main_branch_id, "live.txt", b"retained bytes");
        namespace(
            &mut e,
            &repo.repository_id,
            NamespaceOperationKind::Mkdir,
            "retired",
            None,
        );
        let old_id = e
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("retired")
            .unwrap()
            .entry_id
            .clone();
        e.initialize_directory_scope(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("retired").unwrap()),
            "admin",
            op(),
        )
        .unwrap();
        let old_branch = e.state().directory_selections[&old_id].clone();
        let old_branch_record =
            json_codec::to_value(&e.state().directory_branches[&old_branch]).unwrap();
        let old_selection = e.state().directory_tree_selections.get(&old_id).cloned();
        let old_tree_id = old_selection.clone().unwrap_or_else(|| {
            e.state().directory_branches[&old_branch]
                .head_tree_id
                .clone()
        });
        let old_tree = e.state().directory_trees[&old_tree_id].clone();
        namespace(
            &mut e,
            &repo.repository_id,
            NamespaceOperationKind::Delete,
            "retired",
            None,
        );
        assert!(e.state().directory_selections.contains_key(&old_id));
        if recreate {
            namespace(
                &mut e,
                &repo.repository_id,
                NamespaceOperationKind::Mkdir,
                "retired",
                None,
            );
            let new_id = e
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get("retired")
                .unwrap()
                .entry_id
                .clone();
            assert_ne!(new_id, old_id);
            e.initialize_directory_scope(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("retired").unwrap()),
                "admin",
                op(),
            )
            .unwrap();
        }
        e.initialize_directory_scope(&repo.repository_id, None, "admin", op())
            .unwrap();
        e.begin_disk_exit(
            &removed_disk,
            MembershipExitAction::Deregister,
            "admin",
            op(),
        )
        .unwrap();
        let archive = e.build_managed_membership_control_archive().unwrap();
        e.enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        e.resume_membership_joint_decision().unwrap();
        e.activate_pending_disk_exit().unwrap();
        let root_before = selected_tree(&e, &repo.repository_id);
        let versions = e.state().versions.len();
        let metadata = e.state().file_metadata_nodes.len();
        let branch = e
            .file_context(&repo.repository_id, "live.txt")
            .unwrap()
            .branch_id;
        let head = e.file_branch_head(&branch).unwrap();
        let request = command(&e, &repo, &file.file_id, "live.txt", &["phase-one"]);
        let operation = op();
        let receipt = edit(&mut e, &request, operation).unwrap();
        assert_eq!(e.state().versions.len(), versions);
        assert_eq!(e.state().file_metadata_nodes.len(), metadata);
        let tagged = head;
        let expected_context = e.file_context(&repo.repository_id, "live.txt").unwrap().node_id;
        let expected_tree_context = e.state().directory_trees[&root_before]
            .file_contexts["live.txt"].node_id.clone();
        let assert_current = |engine: &Engine| {
            assert_eq!(engine.file_branch_head(&branch).unwrap(), tagged);
            assert_eq!(
                engine
                    .file_context(&repo.repository_id, "live.txt")
                    .unwrap()
                    .node_id
                    .as_deref(),
                expected_context.as_deref()
            );
            let selected = selected_tree(engine, &repo.repository_id);
            assert_eq!(
                engine.state().directory_trees[&selected].file_contexts["live.txt"]
                    .node_id
                    .as_deref(),
                expected_tree_context.as_deref()
            );
            assert_eq!(engine.state().directory_selections[&old_id], old_branch);
            assert_eq!(
                engine.state().directory_tree_selections.get(&old_id),
                old_selection.as_ref()
            );
            assert_eq!(engine.state().directory_trees[&old_tree_id], old_tree);
            selected
        };
        let root_after = assert_current(&e);
        assert_eq!(root_after, root_before);
        assert_eq!(
            json_codec::to_value(&e.state().directory_branches[&old_branch]).unwrap(),
            old_branch_record
        );
        assert!(e.state().directory_selections.contains_key(&old_id));
        assert_eq!(bytes(&e, &file.version_id), b"retained bytes");
        drop(e);
        let mut e = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(assert_current(&e), root_after);
        let before_retry = json_codec::to_value(e.state()).unwrap();
        let retry = edit(&mut e, &request, operation).unwrap();
        assert_eq!(retry, receipt);
        assert_eq!(json_codec::to_value(e.state()).unwrap(), before_retry);
        assert_eq!(assert_current(&e), root_after);
        assert_eq!(
            container_tags_fixture::tags(&e, &file.file_id),
            BTreeSet::from(["phase-one".into()])
        );
        assert_eq!(
            json_codec::to_value(&e.state().directory_branches[&old_branch]).unwrap(),
            old_branch_record
        );
    }
}
