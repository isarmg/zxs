use super::*;

fn id(prefix: &str, number: usize) -> String {
    format!("{prefix}_{number:032x}")
}

fn entry(file: usize, path: &str) -> Entry {
    Entry {
        entry_id: new_id("entry"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: id("file", file),
        version_id: id("ver", file),
        size: 1,
    }
}

// This pure fixture also exercises duplicate file IDs across repositories. That
// is outside RawState admission, but preserves the full helper's ordering oracle.
fn fixture() -> State {
    let mut state = State::default();
    for number in 1..=2 {
        let repository = id("repo", number);
        let history = id("history", number);
        let branch = id("br", number);
        let commit = id("commit", number);
        state.repositories.insert(
            repository.clone(),
            Repository {
                repository_id: repository.clone(),
                name: format!("repository-{number}"),
                history_tree_id: history.clone(),
                main_branch_id: branch.clone(),
                primary_slot_id: "P01".into(),
                target_copies: 1,
                name_policy: name_policy::NAME_POLICY.into(),
                unicode_version: name_policy::UNICODE_VERSION.into(),
                smb_enabled: true,
                smb_binding_generation: 1,
                file_manager_root: false,
            },
        );
        state.histories.insert(
            history.clone(),
            HistoryTree {
                history_tree_id: history.clone(),
                repository_id: repository,
                root_commit_id: commit.clone(),
                main_branch_id: Some(branch.clone()),
                generation: 1,
                state: HistoryState::Active,
                provenance: None,
            },
        );
        state.branches.insert(
            branch.clone(),
            Branch {
                branch_id: branch,
                history_tree_id: history.clone(),
                name: "main".into(),
                head_commit_id: commit.clone(),
                fork_commit_id: commit.clone(),
                branch_kind: BranchKind::Full,
                file_id: None,
                file_node_id: None,
                pending_file_versions: BTreeMap::new(),
                generation: 1,
                state: BranchState::Active,
            },
        );
        state.commits.insert(
            commit.clone(),
            Commit {
                commit_id: commit,
                history_tree_id: history,
                kind: "root".into(),
                parent_commit_ids: Vec::new(),
                tree_id: id("tree", number),
                sequence: 0,
                operation_id: OperationId::generate(),
                author: "alice".into(),
                message: String::new(),
                entries: vec![entry(number, &format!("repo-{number}/original.txt"))],
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            },
        );
    }
    reference_capture(&mut state);
    state
}

// Independent old full scan: do not call the production head selector, row
// capture helper, or candidate-repository computation from this oracle.
fn reference_capture(state: &mut State) {
    let mut names = state.file_names.clone();
    let mut locations = state.file_locations.clone();
    for repository in state.repositories.values() {
        let branch = &state.branches[&repository.main_branch_id];
        let commit = &state.commits[&branch.head_commit_id];
        for entry in &commit.entries {
            if entry.kind == NamespaceEntryKind::File {
                names
                    .entry(entry.file_id.clone())
                    .or_insert_with(|| entry.name().to_owned());
                locations
                    .entry(entry.file_id.clone())
                    .or_insert_with(|| entry.path().to_owned());
            }
        }
    }
    state.file_names = names;
    state.file_locations = locations;
}

fn candidate(previous: &State) -> State {
    let mut next = previous.clone();
    next.begin_tracking();
    next
}

fn compare(previous: &State, mut next: State) -> State {
    let mut expected = next.clone();
    reference_capture(&mut expected);
    capture_changed_names(previous, &mut next);
    assert_eq!(next, expected);
    next
}

fn add_head(state: &mut State, repository: usize, number: usize, entries: Vec<Entry>) {
    let branch = state.branches.get_mut(&id("br", repository)).unwrap();
    let previous = branch.head_commit_id.clone();
    branch.head_commit_id = id("commit", number);
    let mut commit = state.commits[&previous].clone();
    commit.parent_commit_ids = vec![previous];
    commit.commit_id = id("commit", number);
    commit.entries = entries;
    state.commits.insert(commit.commit_id.clone(), commit);
}

#[test]
fn metadata_and_non_main_branch_writes_visit_no_namespace() {
    let previous = fixture();
    let mut next = candidate(&previous);
    next.file_notes.insert(id("file", 1), "note".into());
    next.repositories.get_mut(&id("repo", 1)).unwrap().name = "renamed repository".into();
    next.branches.get_mut(&id("br", 1)).unwrap().generation += 1;
    let mut branch = next.branches[&id("br", 1)].clone();
    branch.branch_id = id("br", 9);
    branch.head_commit_id = id("commit", 9);
    next.branches.insert(branch.branch_id.clone(), branch);
    let mut commit = next.commits[&id("commit", 1)].clone();
    commit.commit_id = id("commit", 9);
    commit.entries = vec![entry(9, "unselected.txt")];
    next.commits.insert(commit.commit_id.clone(), commit);
    assert_eq!(
        changed_namespace_repositories(&previous, &next),
        Some(BTreeSet::new())
    );
    let next = compare(&previous, next);
    assert!(!next.file_names.contains_key(&id("file", 9)));
}

#[test]
fn changed_heads_preserve_first_alias_and_independent_existing_fields() {
    let previous = fixture();
    let mut next = candidate(&previous);
    add_head(
        &mut next,
        2,
        12,
        vec![
            entry(3, "later-repository.txt"),
            entry(1, "old-file-alias.txt"),
        ],
    );
    add_head(
        &mut next,
        1,
        11,
        vec![
            entry(3, "first.txt"),
            entry(3, "alias.txt"),
            entry(4, "new/name-only.txt"),
            entry(5, "new/location-only.txt"),
        ],
    );
    next.file_names
        .insert(id("file", 4), "retained-name.txt".into());
    next.file_locations
        .insert(id("file", 5), "retained/location.txt".into());
    assert_eq!(
        changed_namespace_repositories(&previous, &next),
        Some(BTreeSet::from([id("repo", 1), id("repo", 2)]))
    );
    let next = compare(&previous, next);
    assert_eq!(next.file_locations[&id("file", 3)], "first.txt");
    assert_eq!(next.file_names[&id("file", 4)], "retained-name.txt");
    assert_eq!(next.file_locations[&id("file", 4)], "new/name-only.txt");
    assert_eq!(next.file_names[&id("file", 5)], "location-only.txt");
    assert_eq!(next.file_locations[&id("file", 5)], "retained/location.txt");
    assert_eq!(next.file_locations[&id("file", 1)], "repo-1/original.txt");
}

#[test]
fn rebinding_an_existing_head_captures_its_previously_unselected_files() {
    let mut previous = fixture();
    let mut branch = previous.branches[&id("br", 1)].clone();
    branch.branch_id = id("br", 8);
    branch.head_commit_id = id("commit", 8);
    previous.branches.insert(branch.branch_id.clone(), branch);
    let mut commit = previous.commits[&id("commit", 1)].clone();
    commit.commit_id = id("commit", 8);
    commit.entries = vec![entry(8, "selected.txt"), entry(1, "old-alias.txt")];
    previous.commits.insert(commit.commit_id.clone(), commit);
    let mut next = candidate(&previous);
    next.repositories
        .get_mut(&id("repo", 1))
        .unwrap()
        .main_branch_id = id("br", 8);
    assert_eq!(
        changed_namespace_repositories(&previous, &next),
        Some(BTreeSet::from([id("repo", 1)]))
    );
    let next = compare(&previous, next);
    assert_eq!(next.file_locations[&id("file", 8)], "selected.txt");
    assert_eq!(next.file_locations[&id("file", 1)], "repo-1/original.txt");
}

#[test]
fn same_head_record_replacement_is_not_mistaken_for_an_unchanged_namespace() {
    let previous = fixture();
    let mut next = candidate(&previous);
    next.commits
        .get_mut(&id("commit", 1))
        .unwrap()
        .entries
        .push(entry(7, "added.txt"));
    assert_eq!(
        changed_namespace_repositories(&previous, &next),
        Some(BTreeSet::from([id("repo", 1)]))
    );
    let next = compare(&previous, next);
    assert_eq!(next.file_names[&id("file", 7)], "added.txt");
}

#[test]
fn namespace_removal_and_readdition_retain_the_last_physical_identity() {
    let previous = fixture();
    let mut next = candidate(&previous);
    add_head(&mut next, 1, 11, Vec::new());
    next.commits.remove_discard(&id("commit", 1));
    next.repositories.remove(&id("repo", 2));
    next.histories.remove(&id("history", 2));
    next.branches.remove(&id("br", 2));
    next.commits.remove_discard(&id("commit", 2));
    assert_eq!(
        changed_namespace_repositories(&previous, &next),
        Some(BTreeSet::from([id("repo", 1)]))
    );
    let previous = compare(&previous, next);
    let mut next = candidate(&previous);
    add_head(&mut next, 1, 12, vec![entry(1, "reappeared.txt")]);
    let next = compare(&previous, next);
    assert_eq!(next.file_locations[&id("file", 1)], "repo-1/original.txt");
    assert_eq!(next.file_locations[&id("file", 2)], "repo-2/original.txt");
}

#[test]
fn cold_pre_capture_wins_before_rebinding_and_remains_in_the_write_set() {
    let mut previous = fixture();
    previous.file_locations.remove(&id("file", 1));
    previous.file_names.remove(&id("file", 1));
    let mut next = candidate(&previous);
    capture_names(&mut next);
    add_head(&mut next, 1, 11, vec![entry(1, "later-alias.txt")]);
    let next = compare(&previous, next);
    assert_eq!(next.file_names[&id("file", 1)], "original.txt");
    assert_eq!(next.file_locations[&id("file", 1)], "repo-1/original.txt");
    for changes in [
        next.file_names.changes_since(&previous.file_names),
        next.file_locations.changes_since(&previous.file_locations),
    ] {
        let storage_contract::MapChanges::Keys(keys) = changes else {
            panic!("cold capture must retain precise transaction writes");
        };
        assert_eq!(keys.as_ref(), &BTreeSet::from([id("file", 1)]));
    }
}

#[test]
fn foreign_and_unknown_mutable_lineage_use_the_independent_full_result() {
    let previous = fixture();
    let mut next = candidate(&previous);
    add_head(&mut next, 1, 11, vec![entry(8, "foreign.txt")]);
    next.commits = next
        .commits
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert!(changed_namespace_repositories(&previous, &next).is_none());
    compare(&previous, next);

    let mut next = candidate(&previous);
    next.repositories.retain(|_, _| true);
    add_head(&mut next, 1, 12, vec![entry(9, "unknown.txt")]);
    assert!(changed_namespace_repositories(&previous, &next).is_none());
    compare(&previous, next);
}

#[test]
fn bounded_write_set_overflow_keeps_full_capture_semantics() {
    let previous = fixture();
    let mut next = candidate(&previous);
    for number in 10..16_395 {
        let mut commit = previous.commits[&id("commit", 1)].clone();
        commit.commit_id = id("commit", number);
        commit.entries.clear();
        next.commits.insert(commit.commit_id.clone(), commit);
    }
    add_head(&mut next, 1, 16_395, vec![entry(9, "overflow.txt")]);
    assert!(changed_namespace_repositories(&previous, &next).is_none());
    compare(&previous, next);
}
