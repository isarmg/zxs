//! A selected file is not a selected version. Exercise a dirty private save,
//! an inherited sibling and later advancement of their actual FILE branch.
use super::*;
use crate::snapshot_worktree::selection::{HeldBranchFile,Pick};

fn selection(engine: &Engine,target: WorktreeHandle,path: &str) -> HeldBranchFile {
    let status = engine.directory_worktree_status(target).unwrap();
    HeldBranchFile::capture(engine, &Pick { worktree:target,visible:status.visible,revision:status.revision,
        path:&PortablePath::parse(path).unwrap() }).unwrap()
}
fn read_held(held: &mut HeldBranchFile) -> Vec<u8> {
    let mut data = Vec::new();
    held.reader().read_to_end(&mut data).unwrap();
    data
}
#[test]
fn selected_worktree_occurrence_reads_latest_head_of_its_branch_not_its_saved_or_inherited_body() {
    let (_fixture,mut engine,actor,request,sibling) = prepare();
    let mut dirty = selection(&engine,request.target,"first.txt");
    assert_eq!(bytes(&engine,request.target,"first.txt"), b"sealed current");
    assert_eq!(read_held(&mut dirty), FIRST_ORIGINAL, "a saved private edit is not a formal branch HEAD");
    let mut old = selection(&engine,sibling,"first.txt");
    let sibling_status = engine.directory_worktree_status(sibling).unwrap();
    let old_branch = old.facts().selected.branch;
    let old_node = old.facts().head.node;
    old.require_current(&engine).unwrap();
    engine.commit_directory_worktree(&request,&actor).unwrap();
    assert_eq!(engine.directory_worktree_status(sibling).unwrap(),sibling_status);
    assert_eq!(bytes(&engine,sibling,"first.txt"),FIRST_ORIGINAL);
    assert!(old.require_current(&engine).is_err(), "unchanged mount/revision does not keep an old FILE head current");
    assert_eq!(read_held(&mut old),FIRST_ORIGINAL,"the already held fd is independent of later source changes");
    let mut current = selection(&engine,sibling,"first.txt");
    assert_eq!(current.facts().selected.branch,old_branch);
    assert_ne!(current.facts().head.node,old_node);
    assert_eq!(engine.file_branch_head(&old_branch.to_string()).unwrap(),current.facts().head.node.to_string());
    assert_eq!(read_held(&mut current), b"sealed current", "the sibling did not create this newly eligible head");
    let wrong_visible = OperationId::generate().unwrap();
    assert!(HeldBranchFile::capture(&engine,&Pick {worktree:sibling,visible:wrong_visible,revision:sibling_status.revision,
        path:&PortablePath::parse("first.txt").unwrap()}).is_err());
    assert!(HeldBranchFile::capture(&engine,&Pick {worktree:sibling,visible:sibling_status.visible,revision:sibling_status.revision,
        path:&PortablePath::parse("Folder").unwrap()}).is_err());
}

#[test]
fn formal_merge_head_retains_its_node_while_selecting_its_older_body_not_the_newer_fork_or_private_edit() {
    let (fixture, mut engine) = Fixture::new();
    let principal: Principal = "admin".parse().unwrap();
    let root = engine.snapshot_root_context(fixture.targets[0].repository).unwrap();
    let allocate = || AllocateWorktree {
        source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation, operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic,
    };
    let picked = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let left = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let unrelated = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let original = selection(&engine, picked, "first.txt");
    let branch = original.facts().selected.branch.to_string();
    let file = original.facts().selected.file.to_string();
    let base = original.facts().head.version;
    let scratch = tempfile::tempdir().unwrap();
    save(&mut engine, left, &principal, scratch.path(), "first.txt", b"bound branch advance");
    save(&mut engine, unrelated, &principal, scratch.path(), "first.txt", b"globally newer, wrong branch");
    let command = |engine: &Engine, target| CommitWorktree {
        target, expected:engine.directory_worktree_status(target).unwrap().revision,
        operation_id:OperationId::generate().unwrap(), message:"real formal FILE publication".into(),
    };
    let left_change = command(&engine, left);
    let other_change = command(&engine, unrelated);
    engine.commit_directory_worktree(&left_change, &principal).unwrap();
    engine.commit_directory_worktree(&other_change, &principal).unwrap();
    let other = selection(&engine, unrelated, "first.txt");
    assert_ne!(other.facts().selected.branch.to_string(), branch, "the original retained operation routed its occurrence to a historical fork");
    let left_head = selection(&engine, left, "first.txt");
    assert!(other.facts().head.body_sequence > left_head.facts().head.body_sequence);
    let advanced = left_head.facts().head.node.to_string();
    let temporary = engine.create_file_branch_from_node(&file, &left_head.facts().head.version.to_string(), &advanced,
        &storage_contract::BranchName::parse("second merge input").unwrap(), "admin", OperationId::generate().unwrap())
        .unwrap().result["branch_id"].as_str().unwrap().to_owned();
    let inputs = [branch.clone(), temporary].map(|id| crate::FileMergeInput {
        expected_node_id: engine.file_branch_head(&id).unwrap(), branch_id:id,
    });
    let merge = OperationId::generate().unwrap();
    let merged = engine.merge_file_branches(crate::FileBranchMergeRequest {
        file_id:&file, inputs:&inputs, chosen_node_id:&base.to_string(), surviving_branch_id:&branch,
        location:None, principal:"admin", operation_id:merge,
    }).unwrap();
    let node: storage_contract::FileNodeId = merged.result["node_id"].as_str().unwrap().parse().unwrap();
    assert!(matches!(node, storage_contract::FileNodeId::Metadata(_)));
    save(&mut engine, picked, &principal, scratch.path(), "first.txt", b"latest dirty content must not win");
    let before = engine.state().events.len();
    let mut selected = selection(&engine, picked, "first.txt");
    assert_eq!(selected.facts().head.node, node);
    assert_ne!(selected.facts().head.node, original.facts().head.node, "same bytes must not erase the new formal HEAD identity");
    assert_eq!(selected.facts().head.commit_operation, merge);
    assert_eq!(selected.facts().head.version, base);
    assert_eq!(selected.facts().body, original.facts().body);
    assert_eq!(selected.facts().body_parent, original.facts().body_parent);
    assert_eq!(selected.facts().body_name, original.facts().body_name);
    assert_eq!(read_held(&mut selected), FIRST_ORIGINAL);
    assert_eq!(bytes(&engine, picked, "first.txt"), b"latest dirty content must not win");
    assert_eq!(engine.state().events.len(), before, "selection is a read, never a source publication");
}
