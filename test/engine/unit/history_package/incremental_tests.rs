use super::*;

fn fixture() -> State {
    let fixture: json_codec::Value = json_codec::from_str(include_str!(
        "../journal/fixtures/canonical_publication.json"
    ))
    .unwrap();
    json_codec::from_str(fixture["state_bytes"].as_str().unwrap()).unwrap()
}
fn dirty(before: &State, after: &State) -> BTreeSet<String> {
    changes::HistoryChanges::between(before, after).histories
}
fn expected(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}
fn fork(state: &State) -> State {
    let mut next = state.clone();
    next.begin_tracking();
    next
}

#[test]
fn file_and_version_dependencies_resolve_both_owners_without_other_histories() {
    let mut before = fixture();
    let first = before.histories.keys().next().unwrap().clone();
    let mut other = before.histories[&first].clone();
    other.history_tree_id = "history_other".into();
    before
        .histories
        .insert(other.history_tree_id.clone(), other);
    let mut unrelated = before.histories[&first].clone();
    unrelated.history_tree_id = "history_unrelated".into();
    unrelated.repository_id = "repo_unrelated".into();
    before
        .histories
        .insert(unrelated.history_tree_id.clone(), unrelated);
    let (version_id, mut version) = before
        .versions
        .iter()
        .next()
        .map(|(id, v)| (id.clone(), v.clone()))
        .unwrap();
    let file = version.file_id.clone();
    version.version_id = "version_other".into();
    version.history_tree_id = "history_other".into();
    before.versions.insert(version.version_id.clone(), version);
    let mut next = fork(&before);
    next.file_notes.insert(file.clone(), "note".into());
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
    let mut next = fork(&before);
    next.version_notes.insert(version_id.clone(), "note".into());
    assert_eq!(dirty(&before, &next), expected(&[&first]));
    let mut next = fork(&before);
    next.versions.get_mut(&version_id).unwrap().history_tree_id = "history_other".into();
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
    let mut next = fork(&before);
    next.versions.remove(&version_id);
    assert_eq!(dirty(&before, &next), expected(&[&first]));
    let mut next = fork(&before);
    next.file_names.remove(&file);
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
}

#[test]
fn repository_context_policy_and_directory_dependencies_follow_projection_scope() {
    let mut before = fixture();
    let first = before.histories.keys().next().unwrap().clone();
    let repository = before.histories[&first].repository_id.clone();
    let mut other = before.histories[&first].clone();
    other.history_tree_id = "history_other".into();
    before
        .histories
        .insert(other.history_tree_id.clone(), other);
    let mut unrelated = before.histories[&first].clone();
    unrelated.history_tree_id = "history_unrelated".into();
    unrelated.repository_id = "repo_unrelated".into();
    before
        .histories
        .insert(unrelated.history_tree_id.clone(), unrelated);
    // Shared repository ownership is a synthetic closure case, not a valid
    // portable manifest; publication validation is deliberately not relaxed.
    let mut next = fork(&before);
    next.repositories
        .get_mut(&repository)
        .unwrap()
        .name
        .push('!');
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
    let context = before.file_contexts.keys().next().unwrap().clone();
    let mut next = fork(&before);
    next.file_contexts.remove(&context);
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
    let branch = before.directory_branches.keys().next().unwrap().clone();
    let mut next = fork(&before);
    next.directory_branches
        .get_mut(&branch)
        .unwrap()
        .repository_id = "repo_unrelated".into();
    assert_eq!(
        dirty(&before, &next),
        expected(&[&first, "history_other", "history_unrelated"])
    );
    let mut next = fork(&before);
    next.directory_selection_generations.insert(repository, 7);
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
}

#[test]
fn selection_retarget_alias_sources_and_shared_directory_membership_are_exact() {
    let mut before = fixture();
    let first = before.histories.keys().next().unwrap().clone();
    let mut other = before.histories[&first].clone();
    other.history_tree_id = "history_other".into();
    other.repository_id = "repo_other".into();
    before
        .histories
        .insert(other.history_tree_id.clone(), other);
    let original = before
        .branches
        .values()
        .find(|b| b.file_id.is_some())
        .unwrap()
        .clone();
    let file = original.file_id.as_ref().unwrap().clone();
    let mut branch = original.clone();
    branch.branch_id = "branch_other".into();
    branch.history_tree_id = "history_other".into();
    before.branches.insert(branch.branch_id.clone(), branch);
    before
        .file_selections
        .insert(file.clone(), original.branch_id.clone());
    let mut next = fork(&before);
    next.file_selections.insert(file, "branch_other".into());
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
    let mut next = fork(&before);
    next.file_branch_aliases
        .insert(original.branch_id.clone(), "branch_other".into());
    assert_eq!(dirty(&before, &next), expected(&[&first]));
    let mut commit = before
        .commits
        .values()
        .find(|c| c.entries.iter().any(|e| e.kind == "directory"))
        .unwrap()
        .clone();
    let directory = commit
        .entries
        .iter()
        .find(|e| e.kind == "directory")
        .unwrap()
        .entry_id
        .clone();
    commit.commit_id = "commit_other".into();
    commit.history_tree_id = "history_other".into();
    before.commits.insert(commit.commit_id.clone(), commit);
    let mut next = fork(&before);
    next.directory_tree_selections
        .insert(directory, "tree_other".into());
    assert_eq!(dirty(&before, &next), expected(&[&first, "history_other"]));
}

#[test]
fn independent_fields_are_ignored_and_unknown_lineage_falls_back() {
    let before = fixture();
    let mut next = fork(&before);
    next.operations.clear();
    next.operation_principals.clear();
    next.plans.clear();
    next.events.clear();
    next.control_auth = Some(json!({"changed":true}));
    next.sequence += 1;
    next.business_generation += 1;
    next.topology_generation += 1;
    assert!(dirty(&before, &next).is_empty());
    let mut next = fork(&before);
    next.file_names = before
        .file_names
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(
        dirty(&before, &next),
        before.histories.keys().cloned().collect()
    );
    let mut next = fork(&before);
    next.file_names.values_mut().for_each(|name| name.push('!'));
    assert_eq!(
        dirty(&before, &next),
        before.histories.keys().cloned().collect()
    );
    let mut next = fork(&before);
    next.file_names.insert("unused".into(), "new".into());
    next.begin_tracking();
    assert_eq!(
        dirty(&before, &next),
        before.histories.keys().cloned().collect()
    );
}

#[test]
fn incremental_history_roots_equal_full_oracle_and_reuse_checks_existing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let before = fixture();
    let roots = store_history_manifests_bounded(&config.disks, &before, MAX_METADATA).unwrap();
    let mut next = fork(&before);
    next.sequence += 1;
    next.operations.clear();
    let (actual, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(
        actual,
        prepare_history_manifests(&next, MAX_METADATA).unwrap().0
    );
    assert!(dirty(&before, &next).is_empty());
    for object in &objects {
        object.store(&config.disks).unwrap();
    }
    let file = before.versions.values().next().unwrap().file_id.clone();
    next.file_notes.insert(file, "changed note".into());
    let (actual, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(
        actual,
        prepare_history_manifests(&next, MAX_METADATA).unwrap().0
    );
    assert_ne!(actual, roots);
    for object in objects {
        object.store(&config.disks).unwrap();
    }
    // A retained root is not permission to trust its on-disk dependencies.
    let hash = roots.values().next().unwrap();
    let path = object_path(&config.disks[0], "history-manifest", hash);
    fs::write(&path, b"corrupt").unwrap();
    let next = fork(&before);
    let (_, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert!(
        objects
            .iter()
            .any(|object| object.store(&config.disks).is_err())
    );
    assert_eq!(fs::read(path).unwrap(), b"corrupt");
    // Losing every replica must retain the full publisher's ability to
    // reconstruct from State, but only under the identical published root.
    let (_, graph) = bound_history_graph(&config.disks, hash).unwrap();
    for (kind, object_hash, _) in graph {
        for disk in &config.disks {
            fs::remove_file(object_path(disk, &kind, &object_hash)).unwrap();
        }
    }
    let (recreated, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(recreated, roots);
    for object in objects {
        object.store(&config.disks).unwrap();
    }
    for disk in &config.disks {
        bound_history_graph(std::slice::from_ref(disk), hash).unwrap();
    }
    let mut next = fork(&before);
    next.histories.clear();
    assert!(
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn reconstruction_never_overwrites_corruption_unsafe_targets_or_changes_bound_roots() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let before = fixture();
    let roots = store_history_manifests_bounded(&config.disks, &before, MAX_METADATA).unwrap();
    let hash = roots.values().next().unwrap();
    for disk in &config.disks {
        fs::write(object_path(disk, "history-manifest", hash), b"corrupt").unwrap();
    }
    let next = fork(&before);
    let (actual, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(actual, roots);
    assert!(
        objects
            .iter()
            .any(|object| object.store(&config.disks).is_err())
    );
    for disk in &config.disks {
        assert_eq!(
            fs::read(object_path(disk, "history-manifest", hash)).unwrap(),
            b"corrupt"
        );
    }
    let path = object_path(&config.disks[0], "history-manifest", hash);
    let outside = temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let (_, objects) =
        prepare_history_manifests_incremental(&before, &next, &roots, &config.disks, MAX_METADATA)
            .unwrap();
    assert!(
        objects
            .iter()
            .any(|object| object.store(&config.disks).is_err())
    );
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    let mut incorrect = roots;
    *incorrect.values_mut().next().unwrap() = "0".repeat(64);
    let error = prepare_history_manifests_incremental(
        &before,
        &next,
        &incorrect,
        &config.disks,
        MAX_METADATA,
    )
    .err()
    .unwrap();
    assert_eq!(error.code(), "OBJECT_CORRUPT");
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}
