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
        .find(|c| {
            c.entries
                .iter()
                .any(|e| e.kind == NamespaceEntryKind::Directory)
        })
        .unwrap()
        .clone();
    let directory = commit
        .entries
        .iter()
        .find(|e| e.kind == NamespaceEntryKind::Directory)
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
    for object in graph {
        let (kind, object_hash) = (object.kind(), object.hash());
        for disk in &config.disks {
            fs::remove_file(object_path(disk, kind, object_hash)).unwrap();
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
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCorrupt);
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn single_history_projection_shares_complete_maps_with_identical_graph() {
    let state = fixture();
    assert_eq!(state.histories.len(), 1);
    assert!(!state.commits.is_empty());
    assert!(!state.directory_trees.is_empty());
    let id = state.histories.keys().next().unwrap();
    let projected = HistoryManifest::from_state(&state, id).unwrap();
    assert!(projected.commits.shares_storage_with(&state.commits));
    assert!(
        projected
            .directory_trees
            .shares_storage_with(&state.directory_trees)
    );
    // Independent materialization uses ordinary map collection, not subset.
    let mut materialized = projected.clone();
    materialized.commits = state
        .commits
        .iter()
        .filter(|(_, commit)| commit.history_tree_id == *id)
        .map(|(key, commit)| (key.clone(), commit.clone()))
        .collect();
    materialized.directory_trees = state
        .directory_trees
        .iter()
        .filter(|(_, tree)| {
            state.directory_branches[&tree.directory_branch_id].repository_id
                == projected.repository.repository_id
        })
        .map(|(key, tree)| (key.clone(), tree.clone()))
        .collect();
    assert!(!materialized.commits.shares_storage_with(&projected.commits));
    assert!(
        !materialized
            .directory_trees
            .shares_storage_with(&projected.directory_trees)
    );
    materialized.validate().unwrap();
    assert_eq!(
        canonical_json(&projected).unwrap(),
        canonical_json(&materialized).unwrap()
    );
    assert_eq!(
        graph::prepare(&projected, MAX_METADATA).unwrap().0,
        graph::prepare(&materialized, MAX_METADATA).unwrap().0
    );
}

#[test]
fn commit_entry_order_admission_matches_canonical_byte_oracle() {
    let state = fixture();
    let history = state.histories.keys().next().unwrap();
    let manifest = HistoryManifest::from_state(&state, history).unwrap();
    let commit = state
        .commits
        .values()
        .find(|commit| commit.entries.len() == 3)
        .unwrap();
    for permutation in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let entries: Vec<_> = permutation
            .into_iter()
            .map(|index| commit.entries[index].clone())
            .collect();
        let mut normalized = entries.clone();
        crate::business::validate_entries(&mut normalized, 1_000_000).unwrap();
        let expected = canonical_json(&normalized).unwrap() == canonical_json(&entries).unwrap();
        let mut changed_manifest = manifest.clone();
        changed_manifest
            .commits
            .get_mut(&commit.commit_id)
            .unwrap()
            .entries = entries.clone();
        assert_eq!(changed_manifest.validate().is_ok(), expected);
        let mut changed_state = state.clone();
        changed_state
            .commits
            .get_mut(&commit.commit_id)
            .unwrap()
            .entries = entries;
        let raw: RawState =
            json_codec::from_slice(&canonical_json(&changed_state).unwrap()).unwrap();
        assert_eq!(AdmittedState::try_from(raw).is_ok(), expected);
    }
}

#[test]
fn directory_branch_head_entry_comparison_preserves_boundary_rejections() {
    let state = fixture();
    let history = state.histories.keys().next().unwrap();
    let manifest = HistoryManifest::from_state(&state, history).unwrap();
    let branch = manifest
        .directory_branches
        .values()
        .find(|branch| branch.entries.len() == 2 && branch.materialized)
        .unwrap();
    let head = &manifest.directory_trees[&branch.head_tree_id];
    assert_eq!(branch.entries, head.entries);
    let mut cases = vec![branch.entries.clone()];
    let mut reordered = branch.entries.clone();
    reordered.reverse();
    cases.push(reordered);
    let mut entry_id = branch.entries.clone();
    entry_id[0].entry_id.push('x');
    cases.push(entry_id);
    let mut path = branch.entries.clone();
    path[0].path = name_policy::PortablePath::parse("folder/A.txt").unwrap();
    assert_eq!(path[0].lookup_key(), branch.entries[0].lookup_key());
    assert_ne!(path[0].path(), branch.entries[0].path());
    cases.push(path);
    let mut kind = branch.entries.clone();
    kind[0].kind = NamespaceEntryKind::Directory;
    cases.push(kind);
    let mut file_id = branch.entries.clone();
    file_id[0].file_id.push('x');
    cases.push(file_id);
    let mut version_id = branch.entries.clone();
    version_id[0].version_id.push('x');
    cases.push(version_id);
    let mut size = branch.entries.clone();
    size[0].size = u64::MAX;
    cases.push(size);
    for entries in cases {
        let expected = canonical_json(&entries).unwrap() == canonical_json(&head.entries).unwrap();
        let mut changed = manifest.clone();
        changed
            .directory_branches
            .get_mut(&branch.directory_branch_id)
            .unwrap()
            .entries = entries.clone();
        match changed.validate() {
            Ok(()) => assert!(expected),
            Err(error) => {
                assert!(!expected);
                assert_eq!(error.code(), ErrorCode::HistoryPackageInvalid);
                assert_eq!(
                    error.message(),
                    "directory branch projection differs from its HEAD tree"
                );
            }
        }
        let mut changed_state = state.clone();
        changed_state
            .directory_branches
            .get_mut(&branch.directory_branch_id)
            .unwrap()
            .entries = entries;
        let raw: RawState =
            json_codec::from_slice(&canonical_json(&changed_state).unwrap()).unwrap();
        assert_eq!(AdmittedState::try_from(raw).is_ok(), expected);
    }
    // A reordered persisted HEAD is independently valid but disagrees with the branch.
    let mut changed = manifest.clone();
    changed
        .directory_trees
        .get_mut(&branch.head_tree_id)
        .unwrap()
        .entries
        .reverse();
    let error = changed.validate().unwrap_err();
    assert_eq!(error.code(), ErrorCode::HistoryPackageInvalid);
    assert_eq!(
        error.message(),
        "directory branch projection differs from its HEAD tree"
    );
}

#[test]
fn branch_value_updates_preserve_full_graph_bytes_and_rebuild_unavailable_old_root() {
    for missing_old_root in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::development([
            temp.path().join("one"),
            temp.path().join("two"),
            temp.path().join("three"),
        ]);
        let before = fixture();
        let roots = store_history_manifests_bounded(&config.disks, &before, MAX_METADATA).unwrap();
        let branch = before
            .branches
            .values()
            .find(|branch| branch.file_id.is_some())
            .unwrap();
        let mut after = fork(&before);
        after.branches.get_mut(&branch.branch_id).unwrap().name = "precise branch update".into();
        if missing_old_root {
            for disk in &config.disks {
                fs::write(
                    object_path(disk, "history-manifest", &roots[&branch.history_tree_id]),
                    b"unreadable old root",
                )
                .unwrap();
            }
        }
        let (expected, expected_objects) = prepare_history_manifests(&after, MAX_METADATA).unwrap();
        let (actual, objects) = prepare_history_manifests_incremental(
            &before,
            &after,
            &roots,
            &config.disks,
            MAX_METADATA,
        )
        .unwrap();
        assert_eq!(actual, expected);
        let bytes = |objects: &[PreparedMetadataObject]| {
            objects
                .iter()
                .map(|object| {
                    (
                        (object.kind(), object.hash.clone()),
                        object.bytes().to_vec(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(bytes(&objects), bytes(&expected_objects));
        for object in objects {
            object.store(&config.disks).unwrap();
        }
        if missing_old_root {
            // The discarded old root is not a dependency of the new candidate;
            // the preparation must neither install it nor silently repair it.
            for disk in &config.disks {
                assert_eq!(
                    fs::read(object_path(
                        disk,
                        "history-manifest",
                        &roots[&branch.history_tree_id]
                    ))
                    .unwrap(),
                    b"unreadable old root"
                );
            }
        }
        bound_history_graph(&config.disks, &actual[&branch.history_tree_id]).unwrap();
    }
}

#[test]
fn precise_branch_updates_do_not_skip_complete_manifest_admission() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let before = fixture();
    let roots = store_history_manifests_bounded(&config.disks, &before, MAX_METADATA).unwrap();
    let branch = before
        .branches
        .values()
        .find(|branch| branch.file_id.is_some())
        .unwrap();
    let mut after = fork(&before);
    after
        .branches
        .get_mut(&branch.branch_id)
        .unwrap()
        .file_node_id = Some("fmeta_00000000000000000000000000000000".into());
    assert!(prepare_history_manifests(&after, MAX_METADATA).is_err());
    assert!(
        prepare_history_manifests_incremental(&before, &after, &roots, &config.disks, MAX_METADATA)
            .is_err()
    );
}
