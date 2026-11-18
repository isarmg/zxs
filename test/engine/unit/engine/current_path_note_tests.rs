//! Real Engine/D3 node-note consumers and exact ordinary-publication replay.
//! These temporary roots are not physical-media or power-loss acceptance.
use crate::{
    Branch, BranchName, BranchState, CommitRequest, Config, Engine, Entry, ErrorCode,
    FileBranchMergeRequest, FileHistoryMergeInput, FileHistoryMergeRequest,
    FileMergeInput, MembershipExitAction,
    NamespaceOperation, NamespaceOperationKind, OperationId, ProtectionPolicyUpdate,
    Repository, canonical_json, new_id,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use storage_contract::Note;

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn note(value: &str) -> Note {
    Note::parse(value.into()).unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(base).unwrap().into(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    visit(root, root, &mut found);
    found
}

fn save_body(engine: &mut Engine, repository: &Repository, body: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "original.txt", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "note source".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("original.txt")
        .unwrap()
        .clone()
}

fn merge_history(
    engine: &mut Engine,
    entry: &Entry,
    branch: &str,
    sources: [&str; 2],
    name: &str,
) -> (String, String) {
    let head = engine.file_branch_head(branch).unwrap();
    let inputs = sources.map(|source| FileHistoryMergeInput {
        branch_id: branch.into(),
        source_node_id: source.into(),
        expected_node_id: head.clone(),
    });
    let receipt = engine
        .merge_file_history(FileHistoryMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: &entry.version_id,
            name: &BranchName::parse(name).unwrap(),
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let node = receipt.result["node_id"].as_str().unwrap().to_owned();
    let branch = receipt.result["branch_id"].as_str().unwrap().to_owned();
    assert_eq!(node,entry.version_id);
    assert!(engine.state.file_metadata_nodes.is_empty());
    (node, branch)
}

fn rename_path(
    engine: &mut Engine,
    repository: &Repository,
    before: &str,
    after: &str,
) -> String {
    let content_branches = engine.state.branches.values()
        .filter(|branch| branch.file_id.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let receipt = engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, before, Some(after)).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let facts = crate::persist::path_changes::read(&receipt).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].before.path.as_str(), before);
    assert_eq!(facts[0].after.path.as_str(), after);
    assert_eq!(facts[0].before_version_id, facts[0].after_version_id);
    assert_eq!(
        engine.state.branches.values()
            .filter(|branch| branch.file_id.is_some())
            .cloned()
            .collect::<Vec<_>>(),
        content_branches
    );
    facts[0].event_id.to_string()
}

fn consume_alias(engine: &mut Engine, entry: &Entry, node: &str, survivor: &str) -> Branch {
    let alias_id = engine
        .create_file_branch_from_node(
            &entry.file_id,
            &entry.version_id,
            node,
            &BranchName::parse("unchanged alias").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let inputs = [survivor, alias_id.as_str()].map(|branch| FileMergeInput {
        branch_id: branch.into(),
        expected_node_id: node.into(),
    });
    engine
        .merge_file_branches(FileBranchMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: node,
            surviving_branch_id: survivor,
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let alias = engine.state.branches[&alias_id].clone();
    assert_eq!(alias.state, BranchState::Merged);
    assert_eq!(engine.state.file_branch_aliases[&alias_id], survivor);
    alias
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    file: String,
    version: String,
    merge_event: String,
    path_event: String,
    branch: String,
    merge_op: OperationId,
    alias: Branch,
}

impl Fixture {
    fn new(copies: u32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config =
            Config::development(crate::test_disks(
                (0..3).map(|n| temp.path().join(format!("disk-{n}"))),
            )).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        if copies != 1 {
            engine
                .update_protection_policy(ProtectionPolicyUpdate {
                    policy_id: &new_id("policy").unwrap(),
                    repository_id: &repository.repository_id,
                    path_prefix: "",
                    target_copies: copies,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let entry = save_body(&mut engine, &repository, b"immutable note body");
        let changed = save_body(&mut engine, &repository, b"distinct content merge source");
        assert_eq!(changed.file_id, entry.file_id);
        assert_ne!(changed.version_id, entry.version_id);
        let context = engine
            .file_context(&repository.repository_id, "original.txt")
            .unwrap();
        // Real content ancestry supplies two distinct inputs. Renaming no
        // longer manufactures a node that can stand in for changed content.
        let (node,survivor)=merge_history(&mut engine,&entry,&context.branch_id,
            [&entry.version_id,&changed.version_id],"only the adopted version");
        let event=engine.state.file_branch_merges.values().find(|e|e.surviving_branch_id==survivor).unwrap().clone();
        let alias=consume_alias(&mut engine,&entry,&node,&survivor);
        let path_event=rename_path(&mut engine,&repository,"original.txt","current.txt");
        Self {temp,engine,config,repository,file:entry.file_id,version:entry.version_id,
            merge_event:event.merge_id,path_event,branch:survivor,merge_op:event.operation_id,alias}
    }

    fn exit(&mut self, action: MembershipExitAction) -> (PathBuf, BTreeMap<PathBuf, Vec<u8>>) {
        let target = &self.config.disks[0];
        let held = Arc::clone(self.engine.disk_resources.get(target).unwrap());
        self.engine
            .begin_disk_exit(&target.disk_id, action, "admin", op())
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        let joint = self.engine.resume_membership_joint_decision().unwrap();
        assert_eq!((joint.original_members, joint.original_quorum), (3, 2));
        let before = files(&target.root);
        let receipt = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(receipt.result["target_io"], "closed");
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(target).is_none());
        assert_eq!(files(&target.root), before);
        let parked = self.temp.path().join("stopped-root");
        fs::rename(&target.root, &parked).unwrap();
        // Opening this original path as a directory now fails immediately.
        fs::write(&target.root, b"closed target must remain an ordinary file").unwrap();
        (parked, before)
    }
}

#[test]
fn membership_registry_current_adopted_version_and_real_events_never_gain_metadata_note_authority() {
    for action in [MembershipExitAction::PlannedOffline,MembershipExitAction::Deregister] {
        for copies in [1,2] {
            let mut f=Fixture::new(copies);
            let (parked,stopped)=f.exit(action);
            let before=canonical_json(f.engine.state()).unwrap();
            for node in [&f.version,&f.path_event,&f.merge_event,&new_id("fmeta").unwrap()] {
                let unused=op();
                assert_eq!(f.engine.set_file_node_note(&f.file,node,&note("cannot invent an annotation"),
                    "admin",unused).unwrap_err().code(), ErrorCode::FileNodeNotFound);
                assert!(!f.engine.state().operations.contains_key(&unused));
            }
            assert_eq!(f.engine.set_file_node_note(&f.file,&f.version,&note("another kind"),
                "admin",f.merge_op).unwrap_err().code(),ErrorCode::IdempotencyKeyReused);
            assert_eq!(canonical_json(f.engine.state()).unwrap(),before);
            let operation=op();
            let written=f.engine.set_file_note(&f.file,Some(&f.version),&note("the actual retained body"),
                "admin",operation).unwrap();
            assert_eq!(f.engine.state().version_notes[&f.version],"the actual retained body");
            assert!(f.engine.state().file_metadata_nodes.is_empty());
            assert!(f.engine.state().file_path_notes.is_empty());
            assert_eq!(f.engine.state().file_branch_aliases[&f.alias.branch_id],f.branch);
            assert_eq!(files(&parked),stopped);
            let system=f.temp.path().join("system");
            drop(f.engine);
            let mut reopened=Engine::open(&system).unwrap();
            assert_eq!(reopened.set_file_note(&f.file,Some(&f.version),&note("the actual retained body"),
                "admin",operation).unwrap().result,written.result);
            assert_eq!(reopened.state().file_branch_merges[&f.merge_event].operation_id,f.merge_op);
            assert_eq!(files(&parked),stopped);
        }
    }
}

#[test]
fn fabricated_metadata_or_a_rewritten_native_input_fails_before_current_snapshot_installation() {
    let mut f=Fixture::new(1);
    let (parked,stopped)=f.exit(MembershipExitAction::PlannedOffline);
    let original=f.engine.state().clone();
    let view=f.engine.current_namespaces.as_ref().unwrap();
    for corruption in ["record","edge","author","original_head","second_result"] {
        let mut after=original.clone();
        if corruption=="record" {
            let id=new_id("fmeta").unwrap();
            after.file_metadata_nodes.insert(id.clone(),crate::FileMetadataNode{node_id:id,
                history_tree_id:f.repository.history_tree_id.clone(),file_id:f.file.clone(),version_id:f.version.clone(),
                parent_node_ids:vec![f.version.clone()],branch_id:f.branch.clone(),kind:storage_contract::FileMetadataKind::Merge,
                operation_id:f.merge_op,author:"admin".into(),sequence:original.sequence,created_at:1});
        } else {
            let e=after.file_branch_merges.get_mut(&f.merge_event).unwrap();
            match corruption {
                "edge"=>e.parent_node_ids.push(e.chosen_node_id.clone()),
                "author"=>e.principal="not the original".parse().unwrap(),
                "original_head"=>e.original_inputs[0].expected_node_id=new_id("ver").unwrap(),
                _=>e.result_node_id=new_id("ver").unwrap(),
            }
        }
        assert!(crate::history_package::validate_prepared_state(&after).is_err());
        assert!(view.advance(&original,&after).is_err());
        assert_eq!(canonical_json(f.engine.state()).unwrap(),canonical_json(&original).unwrap());
        assert_eq!(files(&parked),stopped);
    }
}
