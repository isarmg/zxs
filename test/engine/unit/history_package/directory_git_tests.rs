use super::*;
use storage_contract::directory_git::{
    DirectoryGitBranch, DirectoryGitChildReference, DirectoryGitRepository,
};

struct Fixture {
    manifest: HistoryManifest,
    root: String,
    child: String,
    root_branch: String,
    child_branch: String,
    root_tree: String,
    child_tree: String,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ])
    .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "child/file", b"retained history")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    for path in [Some(name_policy::LookupPath::parse("child").unwrap()), None] {
        engine
            .initialize_directory_scope(
                &repository.repository_id,
                path.as_ref(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let mut manifest =
        HistoryManifest::from_state(engine.state(), &repository.history_tree_id).unwrap();
    let root_branch = manifest
        .directory_branches
        .values()
        .find(|branch| branch.path.is_empty())
        .unwrap()
        .clone();
    let child_branch = manifest
        .directory_branches
        .values()
        .find(|branch| branch.path == "child")
        .unwrap()
        .clone();
    for branch in [&root_branch, &child_branch] {
        manifest.directory_git_repositories.insert(
            branch.directory_id.clone(),
            DirectoryGitRepository {
                deletions: std::collections::BTreeMap::new(),
                repository_id: repository.repository_id.clone(),
                directory_id: branch.directory_id.clone(),
                initialized_sequence: 1,
                main_branch_id: branch.directory_branch_id.clone(),
                current_branch_id: branch.directory_branch_id.clone(),
                branches: BTreeMap::from([(
                    branch.directory_branch_id.clone(),
                    DirectoryGitBranch {
                        head_tree_id: None,
                        index_tree_id: branch.head_tree_id.clone(),
                        index_generation: 1,
                        index_boundaries: BTreeMap::new(),
                        index_children: BTreeMap::new(),
                    },
                )]),
            },
        );
    }
    let child_tree = committed_tree(&mut manifest, &child_branch.head_tree_id, 100);
    let root_tree = committed_tree(&mut manifest, &root_branch.head_tree_id, 200);
    let mut tree = manifest.directory_trees[&root_tree].clone();
    tree.directory_refs = BTreeMap::from([(
        child_branch.directory_id.clone(),
        DirectoryReference {
            directory_branch_id: child_branch.directory_branch_id.clone(),
            tree_id: child_tree.clone(),
        },
    )]);
    let child_snapshot = &manifest.directory_trees[&child_tree];
    tree.file_contexts
        .extend(child_snapshot.file_contexts.clone());
    tree.file_branches
        .extend(child_snapshot.file_branches.clone());
    tree.inherited_files
        .extend(child_snapshot.inherited_files.clone());
    manifest.directory_trees.insert(root_tree.clone(), tree);
    for (branch, tree) in [(&child_branch, &child_tree), (&root_branch, &root_tree)] {
        manifest
            .directory_git_repositories
            .get_mut(&branch.directory_id)
            .unwrap()
            .branches
            .get_mut(&branch.directory_branch_id)
            .unwrap()
            .head_tree_id = Some(tree.clone());
    }
    let index = manifest
        .directory_git_repositories
        .get_mut(&root_branch.directory_id)
        .unwrap()
        .branches
        .get_mut(&root_branch.directory_branch_id)
        .unwrap();
    index
        .index_boundaries
        .insert(child_branch.directory_id.clone(), "child".into());
    index.index_children.insert(
        child_branch.directory_id.clone(),
        DirectoryGitChildReference {
            directory_branch_id: child_branch.directory_branch_id.clone(),
            tree_id: Some(child_tree.clone()),
        },
    );
    manifest.validate().unwrap();
    Fixture {
        manifest,
        root: repository.repository_id,
        child: child_branch.directory_id,
        root_branch: root_branch.directory_branch_id,
        child_branch: child_branch.directory_branch_id,
        root_tree,
        child_tree,
    }
}

fn committed_tree(manifest: &mut HistoryManifest, working: &str, sequence: u64) -> String {
    let mut tree = manifest.directory_trees[working].clone();
    let mut commit = manifest.commits[&tree.commit_id].clone();
    let tree_id = new_id("dtree").unwrap();
    let commit_id = new_id("commit").unwrap();
    commit.commit_id = commit_id.clone();
    commit.kind = "directory_commit".into();
    commit.parent_commit_ids.clear();
    commit.sequence = sequence;
    commit.provenance = Some(tree_id.clone());
    commit.tree_id = new_id("tree").unwrap();
    tree.tree_id = tree_id.clone();
    tree.commit_id = commit_id.clone();
    tree.sequence = sequence;
    tree.parent_tree_id = None;
    tree.directory_refs.clear();
    manifest.commits.insert(commit_id, commit);
    manifest.directory_trees.insert(tree_id.clone(), tree);
    tree_id
}

fn index<'a>(
    manifest: &'a mut HistoryManifest,
    directory: &str,
    branch: &str,
) -> &'a mut DirectoryGitBranch {
    manifest
        .directory_git_repositories
        .get_mut(directory)
        .unwrap()
        .branches
        .get_mut(branch)
        .unwrap()
}

#[test]
fn directory_git_history_preserves_exact_heads_indexes_and_child_snapshots() {
    let f = fixture();
    let manifest = f.manifest;
    let (hash, objects) = graph::prepare(&manifest, MAX_METADATA).unwrap();
    let objects = objects
        .into_iter()
        .map(|object| ((object.kind(), object.hash.clone()), object))
        .collect::<BTreeMap<_, _>>();
    let decoded = read_bound_foreign_manifest(&hash, |kind, hash| {
        let object = &objects[&(kind, hash.to_owned())];
        AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(object.bytes())?)
    })
    .unwrap();
    assert_eq!(
        decoded.directory_git_repositories,
        manifest.directory_git_repositories
    );
    assert_eq!(
        decoded.directory_trees[&f.root_tree],
        manifest.directory_trees[&f.root_tree]
    );
    assert_ne!(
        decoded.directory_git_repositories[&f.root].branches[&f.root_branch].index_tree_id,
        f.root_tree
    );
}

#[test]
fn directory_git_history_rejects_identity_scope_and_missing_index_edges() {
    let f = fixture();
    for attack in 0..12 {
        let mut manifest = f.manifest.clone();
        match attack {
            0 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .directory_id = f.child.clone();
            }
            1 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .repository_id = new_id("repo").unwrap();
            }
            2 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .initialized_sequence = 0;
            }
            3 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .current_branch_id = new_id("dbr").unwrap();
            }
            4 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .branches
                    .clear();
            }
            5 => {
                index(&mut manifest, &f.root, &f.root_branch).index_generation = 0;
            }
            6 => {
                index(&mut manifest, &f.root, &f.root_branch).index_tree_id =
                    new_id("dtree").unwrap();
            }
            7 => {
                index(&mut manifest, &f.root, &f.root_branch).index_tree_id = f.child_tree.clone();
            }
            8 => {
                index(&mut manifest, &f.root, &f.root_branch).head_tree_id =
                    Some(new_id("dtree").unwrap());
            }
            9 => {
                index(&mut manifest, &f.root, &f.root_branch).head_tree_id =
                    Some(f.child_tree.clone());
            }
            10 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .main_branch_id = new_id("dbr").unwrap();
            }
            11 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .main_branch_id = f.child_branch.clone();
            }
            _ => unreachable!(),
        }
        assert!(manifest.validate().is_err(), "attack {attack}");
    }
}

#[test]
fn directory_git_history_rejects_uncommitted_heads_and_forged_commit_markers() {
    let f = fixture();
    for attack in 0..8 {
        let mut manifest = f.manifest.clone();
        let mut tree = manifest.directory_trees[&f.root_tree].clone();
        let mut commit = manifest.commits[&tree.commit_id].clone();
        match attack {
            0 => {
                index(&mut manifest, &f.root, &f.root_branch).head_tree_id = Some(
                    manifest.directory_branches[&f.root_branch]
                        .head_tree_id
                        .clone(),
                );
            }
            1 => {
                commit.provenance = None;
            }
            2 => {
                commit.provenance = Some(f.child_tree.clone());
            }
            3 => {
                commit.sequence += 1;
            }
            4 => {
                tree.parent_tree_id = Some(
                    manifest.directory_branches[&f.root_branch]
                        .head_tree_id
                        .clone(),
                );
            }
            5 => {
                commit.parent_commit_ids =
                    vec![manifest.directory_trees[&f.child_tree].commit_id.clone()];
            }
            6 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .initialized_sequence = tree.sequence + 1;
            }
            7 => {
                manifest
                    .directory_git_repositories
                    .get_mut(&f.root)
                    .unwrap()
                    .initialized_sequence = tree.sequence;
            }
            _ => unreachable!(),
        }
        manifest.directory_trees.insert(tree.tree_id.clone(), tree);
        manifest.commits.insert(commit.commit_id.clone(), commit);
        assert!(manifest.validate().is_err(), "attack {attack}");
    }
}

#[test]
fn directory_git_history_pins_child_commits_instead_of_following_current_heads() {
    let mut f = fixture();
    let old_child = f.child_tree.clone();
    let next = committed_tree(&mut f.manifest, &old_child, 300);
    let mut tree = f.manifest.directory_trees[&next].clone();
    tree.parent_tree_id = Some(old_child.clone());
    let mut commit = f.manifest.commits[&tree.commit_id].clone();
    commit.parent_commit_ids = vec![f.manifest.directory_trees[&old_child].commit_id.clone()];
    f.manifest.commits.insert(commit.commit_id.clone(), commit);
    f.manifest.directory_trees.insert(next.clone(), tree);
    index(&mut f.manifest, &f.child, &f.child_branch).head_tree_id = Some(next.clone());
    f.manifest.validate().unwrap();
    assert_eq!(
        f.manifest.directory_trees[&f.root_tree].directory_refs[&f.child].tree_id,
        old_child
    );
    assert_ne!(old_child, next);
    // Stale indexed child references may remain after stage-owned boundaries change.
    index(&mut f.manifest, &f.root, &f.root_branch)
        .index_boundaries
        .clear();
    f.manifest.validate().unwrap();
}

#[test]
fn directory_git_user_commit_dependencies_must_precede_their_referrer() {
    let f = fixture();
    let parent_sequence = f.manifest.directory_trees[&f.root_tree].sequence;
    for sequence in [parent_sequence, parent_sequence + 1] {
        let mut manifest = f.manifest.clone();
        let mut child = manifest.directory_trees[&f.child_tree].clone();
        let mut marker = manifest.commits[&child.commit_id].clone();
        child.sequence = sequence;
        marker.sequence = sequence;
        manifest
            .directory_trees
            .insert(child.tree_id.clone(), child);
        manifest.commits.insert(marker.commit_id.clone(), marker);
        assert!(
            manifest.validate().is_err(),
            "a parent cannot pin a same-sequence or future child commit"
        );
    }

    let previous = &f.manifest.directory_trees[&f.child_tree];
    for sequence in [previous.sequence, previous.sequence - 1] {
        let mut manifest = f.manifest.clone();
        let next = committed_tree(&mut manifest, &f.child_tree, sequence);
        let mut child = manifest.directory_trees[&next].clone();
        let mut marker = manifest.commits[&child.commit_id].clone();
        child.parent_tree_id = Some(f.child_tree.clone());
        marker.parent_commit_ids = vec![previous.commit_id.clone()];
        manifest.directory_trees.insert(next, child);
        manifest.commits.insert(marker.commit_id.clone(), marker);
        assert!(
            manifest.validate().is_err(),
            "a new user commit cannot precede or share its parent's sequence"
        );
    }
}

#[test]
fn directory_git_history_rejects_bad_child_references_and_staged_paths() {
    let f = fixture();
    for attack in 0..7 {
        let mut manifest = f.manifest.clone();
        match attack {
            0 => {
                manifest.directory_git_repositories.remove(&f.child);
            }
            1 => {
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_children
                    .get_mut(&f.child)
                    .unwrap()
                    .tree_id = Some(new_id("dtree").unwrap());
            }
            2 => {
                let raw = manifest.directory_branches[&f.child_branch]
                    .head_tree_id
                    .clone();
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_children
                    .get_mut(&f.child)
                    .unwrap()
                    .tree_id = Some(raw);
            }
            3 => {
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_children
                    .get_mut(&f.child)
                    .unwrap()
                    .directory_branch_id = f.root_branch.clone();
            }
            4 => {
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_boundaries
                    .insert(f.child.clone(), "../escape".into());
            }
            5 => {
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_boundaries
                    .insert(f.child.clone(), "unrelated".into());
            }
            6 => {
                let reference = DirectoryGitChildReference {
                    directory_branch_id: f.root_branch.clone(),
                    tree_id: Some(f.root_tree.clone()),
                };
                index(&mut manifest, &f.root, &f.root_branch)
                    .index_children
                    .insert(f.root.clone(), reference);
            }
            _ => unreachable!(),
        }
        assert!(manifest.validate().is_err(), "attack {attack}");
    }
}

#[test]
fn directory_git_boundary_detection_does_not_rely_on_adjacent_lexical_names() {
    assert!(
        reject_nested_boundaries(&BTreeSet::from(["a".into(), "a-b".into(), "a/b".into()]))
            .is_err()
    );
    assert!(
        reject_nested_boundaries(&BTreeSet::from(["a".into(), "a-b".into(), "b/a".into()])).is_ok()
    );
}

#[test]
fn directory_git_history_allows_inherited_head_and_index_in_the_same_directory() {
    let mut f = fixture();
    let mut branch = f.manifest.directory_branches[&f.root_branch].clone();
    let id = new_id("dbr").unwrap();
    branch.directory_branch_id = id.clone();
    branch.name = "inherited".into();
    branch.parent_branch_id = Some(f.root_branch.clone());
    branch.materialized = false;
    let inherited = f.manifest.directory_git_repositories[&f.root].branches[&f.root_branch].clone();
    f.manifest.directory_branches.insert(id.clone(), branch);
    let git = f
        .manifest
        .directory_git_repositories
        .get_mut(&f.root)
        .unwrap();
    git.branches.insert(id.clone(), inherited);
    git.current_branch_id = id.clone();
    assert!(
        f.manifest.validate().is_err(),
        "another valid Git branch cannot disagree with the live working selection"
    );
    f.manifest.directory_selections.insert(f.root.clone(), id);
    f.manifest.validate().unwrap();
    f.manifest.directory_selections.remove(&f.root);
    assert!(
        f.manifest.validate().is_err(),
        "a live initialized repository must retain its working selection"
    );
}

#[test]
fn directory_git_retained_deleted_repository_needs_no_live_selection() {
    let mut f = fixture();
    f.manifest.directory_selections.remove(&f.child);
    f.manifest.directory_tree_selections.remove(&f.child);
    assert!(
        f.manifest.validate().is_err(),
        "the child is still live and cannot lose its working selection"
    );

    // A later namespace commit removes the directory without erasing its
    // immutable history, pinned child references, or explicit Git repository.
    let main = f.manifest.repository.main_branch_id.clone();
    let previous = f.manifest.branches[&main].head_commit_id.clone();
    let mut removal = f.manifest.commits[&previous].clone();
    removal.commit_id = new_id("commit").unwrap();
    removal.tree_id = new_id("tree").unwrap();
    removal.parent_commit_ids = vec![previous];
    removal.sequence = f
        .manifest
        .commits
        .values()
        .map(|commit| commit.sequence)
        .max()
        .unwrap()
        + 1;
    removal.operation_id = OperationId::generate().unwrap();
    removal.entries.clear();
    removal.pending_file_versions.clear();
    let id = removal.commit_id.clone();
    f.manifest.commits.insert(id.clone(), removal);
    f.manifest.branches.get_mut(&main).unwrap().head_commit_id = id;
    f.manifest.file_contexts.clear();
    f.manifest.validate().unwrap();
    assert!(f.manifest.directory_git_repositories.contains_key(&f.child));
    assert!(f.manifest.directory_trees.contains_key(&f.child_tree));
}

#[test]
fn directory_git_committed_child_cycle_is_detected_without_recursive_traversal() {
    let mut f = fixture();
    f.manifest.validate_git_tree_cycles().unwrap();
    let mut child = f.manifest.directory_trees[&f.child_tree].clone();
    child.directory_refs.insert(
        f.root.clone(),
        DirectoryReference {
            directory_branch_id: f.root_branch,
            tree_id: f.root_tree,
        },
    );
    f.manifest
        .directory_trees
        .insert(child.tree_id.clone(), child);
    assert!(f.manifest.validate_git_tree_cycles().is_err());
    assert!(f.manifest.validate().is_err());
}

#[test]
fn directory_git_independent_roots_do_not_admit_detached_or_cyclic_commit_graphs() {
    let f = fixture();
    let root_commit = f.manifest.directory_trees[&f.root_tree].commit_id.clone();
    let child_commit = f.manifest.directory_trees[&f.child_tree].commit_id.clone();
    let mut cyclic = f.manifest.clone();
    for (id, parent) in [(&root_commit, &child_commit), (&child_commit, &root_commit)] {
        let mut commit = cyclic.commits[id].clone();
        commit.parent_commit_ids = vec![parent.clone()];
        cyclic.commits.insert(id.clone(), commit);
    }
    let error = cyclic.validate().unwrap_err();
    assert!(error.to_string().contains("cycle or detached"));
    let mut detached = f.manifest.clone();
    let mut commit = detached.commits[&root_commit].clone();
    commit.kind = "save".into();
    detached.commits.insert(root_commit, commit);
    assert!(detached.validate().is_err());
    let mut forged = f.manifest.clone();
    index(&mut forged, &f.root, &f.root_branch).index_tree_id = new_id("dtree").unwrap();
    let (hash, objects) = graph::prepare(&forged, MAX_METADATA).unwrap();
    let objects = objects
        .into_iter()
        .map(|object| ((object.kind(), object.hash.clone()), object))
        .collect::<BTreeMap<_, _>>();
    assert!(
        read_bound_foreign_manifest(&hash, |kind, hash| {
            let object = &objects[&(kind, hash.to_owned())];
            AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(object.bytes())?)
        })
        .is_err()
    );
}

#[test]
fn committed_marker_and_child_projection_cannot_describe_different_contents() {
    let f = fixture();
    for alter_marker_too in [false, true] {
        let mut manifest = f.manifest.clone();
        let mut tree = manifest.directory_trees[&f.root_tree].clone();
        let file = tree
            .entries
            .iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap()
            .clone();
        tree.entries.retain(|entry| entry.entry_id != file.entry_id);
        tree.file_contexts.remove(file.lookup_key());
        tree.file_branches.remove(&file.file_id);
        tree.inherited_files.remove(&file.file_id);
        if alter_marker_too {
            let mut commit = manifest.commits[&tree.commit_id].clone();
            commit
                .entries
                .retain(|entry| entry.entry_id != file.entry_id);
            manifest.commits.insert(commit.commit_id.clone(), commit);
        }
        manifest.directory_trees.insert(tree.tree_id.clone(), tree);
        let error = manifest.validate().unwrap_err();
        assert!(error.to_string().contains(if alter_marker_too {
            "child expansion differs"
        } else {
            "exact tree projection"
        }));
    }
}

#[test]
fn committed_parent_cannot_change_a_pinned_child_file_context() {
    let f = fixture();
    let mut manifest = f.manifest;
    let mut tree = manifest.directory_trees[&f.root_tree].clone();
    let context = tree.file_contexts.values_mut().next().unwrap();
    let mut alias = manifest.branches[&context.branch_id].clone();
    alias.branch_id = new_id("br").unwrap();
    alias.name = "another valid file branch".into();
    context.branch_id = alias.branch_id.clone();
    manifest.branches.insert(alias.branch_id.clone(), alias);
    manifest.directory_trees.insert(tree.tree_id.clone(), tree);
    let error = manifest.validate().unwrap_err();
    assert!(error.to_string().contains("entries or contexts differ"));
}
