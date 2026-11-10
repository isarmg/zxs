//! Freeze the c7273397 projection oracle while replacing only its source storage.
use super::*;
use crate::directory_contents::DirectoryContents;

fn copied_project_branch(
    state: &State,
    source: &DirectoryBranch,
    entries: &[Entry],
    retained_children: &BTreeSet<&str>,
) -> Result<DirectoryBranch> {
    let mut branch = DirectoryBranch {
        directory_branch_id: source.directory_branch_id.clone(),
        repository_id: source.repository_id.clone(),
        directory_id: source.directory_id.clone(),
        path: source.path.clone(),
        name: source.name.clone(),
        parent_branch_id: source.parent_branch_id.clone(),
        head_tree_id: source.head_tree_id.clone(),
        commit_id: source.commit_id.clone(),
        entries: entries.to_vec(),
        file_contexts: BTreeMap::new(),
        file_branches: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        materialized: source.materialized,
        sequence: source.sequence,
    };
    for entry in entries {
        if at_key(&source.entries, entry.lookup_key()) != Some(entry) {
            return Err(invalid(
                "directory projection imports an occurrence outside its source",
            ));
        }
        match entry.kind {
            NamespaceEntryKind::File => {
                let use_as = if retained_children.contains(entry.entry_id.as_str()) {
                    ContextUse::RetainedChild
                } else {
                    ContextUse::Current
                };
                let context = source
                    .file_contexts
                    .get(entry.lookup_key())
                    .ok_or_else(stale)?;
                validate_context_for(state, &source.repository_id, entry, context, use_as)?;
                let file_branch = source.file_branches.get(&entry.file_id).ok_or_else(stale)?;
                let record = state.branches.get(file_branch).ok_or_else(stale)?;
                if record.file_id.as_deref() != Some(entry.file_id.as_str())
                    || (use_as == ContextUse::Current && record.state != BranchState::Active)
                    || record.history_tree_id
                        != state.repositories[&source.repository_id].history_tree_id
                {
                    return Err(invalid(
                        "directory source file branch differs from its occurrence",
                    ));
                }
                branch
                    .file_contexts
                    .insert(entry.lookup_key().into(), context.clone());
                branch
                    .file_branches
                    .insert(entry.file_id.clone(), file_branch.clone());
                if let Some(origin) = source.inherited_files.get(&entry.file_id) {
                    branch
                        .inherited_files
                        .insert(entry.file_id.clone(), origin.clone());
                }
            }
            NamespaceEntryKind::Directory => {
                if let Some(reference) = source.directory_refs.get(&entry.entry_id) {
                    branch
                        .directory_refs
                        .insert(entry.entry_id.clone(), reference.clone());
                }
            }
        }
    }
    Ok(branch)
}

fn frozen_snapshot(owner: &DirectoryBranch, tree: &DirectoryTree) -> DirectoryBranch {
    let mut snapshot = owner.clone();
    snapshot.commit_id = tree.commit_id.clone();
    snapshot.path = tree.path.clone();
    snapshot.entries = tree.entries.clone();
    snapshot.file_contexts = tree.file_contexts.clone();
    snapshot.file_branches = tree.file_branches.clone();
    snapshot.inherited_files = tree.inherited_files.clone();
    snapshot.directory_refs = tree.directory_refs.clone();
    snapshot
}

fn projection_result(
    result: Result<DirectoryBranch>,
) -> std::result::Result<Vec<u8>, (ErrorCode, String)> {
    result
        .map(|branch| canonical_json(&branch).unwrap())
        .map_err(|error| (error.code(), error.to_string()))
}

fn compare_source(state: &State, owner: &DirectoryBranch, tree: Option<&DirectoryTree>) -> usize {
    let snapshot = tree.map(|tree| frozen_snapshot(owner, tree));
    let copied = snapshot.as_ref().unwrap_or(owner);
    let mut compared = 0;
    for filter in 0..5 {
        let mut entries = copied
            .entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| match filter {
                0 | 4 => true,
                1 => false,
                2 => index % 2 == 0,
                3 => entry.kind == NamespaceEntryKind::File,
                _ => unreachable!(),
            })
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>();
        if filter == 4
            && let Some(entry) = entries.first_mut()
        {
            entry.size ^= 1;
        }
        for retain in [false, true] {
            let retained = copied
                .entries
                .iter()
                .filter(|entry| retain && entry.kind == NamespaceEntryKind::File)
                .map(|entry| entry.entry_id.as_str())
                .collect::<BTreeSet<_>>();
            let contents = match tree {
                Some(tree) => DirectoryContents::from(tree),
                None => DirectoryContents::from(owner),
            };
            assert_eq!(
                projection_result(project_branch(state, owner, contents, &entries, &retained)),
                projection_result(copied_project_branch(state, copied, &entries, &retained)),
                "branch={}, tree={:?}, filter={filter}, retain={retain}",
                owner.directory_branch_id,
                tree.map(|tree| &tree.tree_id),
            );
            compared += 1;
        }
    }
    compared
}

fn compare_state(state: &State) {
    let mut compared = 0;
    for owner in state.directory_branches.values() {
        compared += compare_source(state, owner, None);
    }
    for tree in state.directory_trees.values() {
        let owner = &state.directory_branches[&tree.directory_branch_id];
        compared += compare_source(state, owner, Some(tree));
    }
    assert!(compared > 0);
    let source = state
        .directory_trees
        .values()
        .find(|tree| {
            tree.entries
                .iter()
                .any(|entry| entry.kind == NamespaceEntryKind::File)
        })
        .unwrap();
    let owner = &state.directory_branches[&source.directory_branch_id];
    for field in 0..6 {
        let mut tree = source.clone();
        match field {
            0 => tree.entries[0].size += 1,
            1 => tree.file_contexts.clear(),
            2 => tree.file_branches.clear(),
            3 => {
                for context in tree.file_contexts.values_mut() {
                    context.branch_id = new_id("br").unwrap();
                }
            }
            4 => {
                for context in tree.file_contexts.values_mut() {
                    context.node_id = Some(new_id("commit").unwrap());
                }
            }
            5 => {
                for context in tree.file_contexts.values_mut() {
                    context.inherited_from = Some(new_id("dbr").unwrap());
                }
            }
            _ => unreachable!(),
        }
        compare_source(state, owner, Some(&tree));
    }
}

#[test]
fn borrowed_projection_matches_copied_snapshots_before_and_after_disk_exit() {
    let mut fixture = Fixture::new();
    let before = canonical_json(fixture.engine.state()).unwrap();
    compare_state(fixture.engine.state());
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture.exit(MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    compare_state(fixture.engine.state());
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}
