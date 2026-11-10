//! Differential oracles retain the two pre-refactor algorithms from 87307cd6.
//! Only tests contain the old loops; production has one commit projection.
use super::*;
use storage_contract::{DeletedPath, HistoryState, HistoryTree, NamespaceDeletions};

fn mapped_entry(scope: &CommitScope<'_>, entry: &Entry) -> Result<Entry> {
    let mut mapped = entry.clone();
    mapped.entry_id = scope.mapped(&entry.entry_id)?;
    if entry.kind == NamespaceEntryKind::File {
        mapped.file_id = scope.mapped(&entry.file_id)?;
        mapped.version_id = scope.mapped(&entry.version_id)?;
    }
    Ok(mapped)
}

fn single_oracle(
    previous: &State,
    target: &Repository,
    original: &Commit,
    scope: &CommitScope<'_>,
    target_root: &str,
) -> Result<Commit> {
    let source = CopySources {
        file: (*scope.files.first().unwrap()).into(),
        versions: Vec::new(),
        branches: Vec::new(),
    };
    let ids = scope.ids;
    let mapped = |id: &str| scope.mapped(id);
    let mapped_list = |ids: &[String]| ids.iter().map(|id| mapped(id)).collect::<Result<Vec<_>>>();
    let file = mapped(&source.file)?;

    let mut commit = original.clone();
    commit.commit_id = mapped(&original.commit_id)?;
    commit.tree_id = mapped(&original.tree_id)?;
    commit.history_tree_id = target.history_tree_id.clone();
    commit.parent_commit_ids = mapped_list(&original.parent_commit_ids)?;
    if original.parent_commit_ids.is_empty() {
        commit.parent_commit_ids = vec![target_root.to_owned()];
        commit.kind = "file_history_projection".into();
    } else if original.kind == "directory_commit" {
        // Preserve the file snapshot, not a foreign directory Git marker.
        commit.kind = "file_history_projection".into();
    }
    // Provenance belonging to another file is not part of this projection.
    // Real file-node/commit/version references are remapped, never borrowed.
    commit.provenance = original
        .provenance
        .as_ref()
        .and_then(|id| ids.get(id).cloned());
    commit.entries = original
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::File && entry.file_id == source.file
                || entry.kind == NamespaceEntryKind::Directory
                    && original.entries.iter().any(|file| {
                        file.file_id == source.file
                            && namespace::is_descendant(file.lookup_key(), entry.lookup_key())
                    })
        })
        .map(|entry| {
            let mut entry = entry.clone();
            entry.entry_id = mapped(&entry.entry_id)?;
            if entry.kind == NamespaceEntryKind::File {
                entry.file_id = file.clone();
                entry.version_id = mapped(&entry.version_id)?;
            }
            Ok(entry)
        })
        .collect::<Result<Vec<_>>>()?;
    commit.pending_file_versions = original
        .pending_file_versions
        .get(&source.file)
        .map(|version| Ok::<_, Error>([(file.clone(), mapped(version)?)].into()))
        .transpose()?
        .unwrap_or_default();
    // A namespace removal has paths, not file IDs. Preserve only removals
    // that covered this file in the immediately preceding snapshots.
    if let Some(deleted) = &mut commit.namespace_deletions {
        deleted.paths.retain(|deleted| {
            original.parent_commit_ids.iter().any(|parent| {
                previous.commits[parent].entries.iter().any(|entry| {
                    entry.file_id == source.file
                        && (entry.path() == deleted.path
                            || namespace::is_descendant(
                                entry.lookup_key(),
                                &name_policy::path_key(&deleted.path),
                            ))
                })
            })
        });
        if deleted.paths.is_empty() {
            commit.namespace_deletions = None;
        }
    }
    Ok(commit)
}

fn aggregate_oracle(
    previous: &State,
    target: &Repository,
    original: &Commit,
    scope: &CommitScope<'_>,
    target_root: &str,
) -> Result<Commit> {
    let files = &scope.files;
    let directories = &scope.directories;

    let mut commit = original.clone();
    commit.commit_id = scope.mapped(&original.commit_id)?;
    commit.tree_id = scope.mapped(&original.tree_id)?;
    commit.history_tree_id = target.history_tree_id.clone();
    commit.parent_commit_ids = original
        .parent_commit_ids
        .iter()
        .map(|id| scope.mapped(id))
        .collect::<Result<_>>()?;
    let retained_directory_commit = original.kind == "directory_commit"
        && original
            .provenance
            .as_ref()
            .is_some_and(|tree| scope.trees.contains(tree.as_str()));
    if !retained_directory_commit {
        if commit.parent_commit_ids.is_empty() {
            commit.parent_commit_ids.push(target_root.to_owned());
            commit.kind = "file_history_projection".into();
        } else if original.kind == "directory_commit" {
            commit.kind = "file_history_projection".into();
        }
    }
    let selected = original
        .entries
        .iter()
        .filter(|entry| {
            (entry.kind == NamespaceEntryKind::File && files.contains(entry.file_id.as_str()))
                || directories.contains(entry.entry_id.as_str())
        })
        .map(Entry::lookup_key)
        .collect::<BTreeSet<_>>();
    commit.entries = original
        .entries
        .iter()
        .filter(|entry| {
            selected.contains(entry.lookup_key())
                || (entry.kind == NamespaceEntryKind::Directory
                    && selected
                        .iter()
                        .any(|path| namespace::is_descendant(path, entry.lookup_key())))
        })
        .map(|entry| mapped_entry(scope, entry))
        .collect::<Result<_>>()?;
    commit.pending_file_versions = original
        .pending_file_versions
        .iter()
        .filter(|(file, _)| files.contains(file.as_str()))
        .map(|(file, version)| Ok((scope.mapped(file)?, scope.mapped(version)?)))
        .collect::<Result<_>>()?;
    commit.provenance = original
        .provenance
        .as_ref()
        .and_then(|id| scope.ids.get(id).cloned());
    // Old removal paths remain historical. Keep only ones affecting a
    // selected member in the immediate parent snapshots.
    if let Some(deletions) = &mut commit.namespace_deletions {
        deletions.paths.retain(|deleted| {
            original.parent_commit_ids.iter().any(|parent| {
                previous.commits[parent].entries.iter().any(|entry| {
                    (files.contains(entry.file_id.as_str())
                        || directories.contains(entry.entry_id.as_str()))
                        && (entry.path() == deleted.path
                            || namespace::is_descendant(
                                entry.lookup_key(),
                                &name_policy::path_key(&deleted.path),
                            ))
                })
            })
        });
        if deletions.paths.is_empty() {
            commit.namespace_deletions = None;
        }
    }
    Ok(commit)
}

fn entry(path: &str, file: Option<&str>) -> Entry {
    Entry {
        entry_id: format!("entry_{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: if file.is_some() {
            NamespaceEntryKind::File
        } else {
            NamespaceEntryKind::Directory
        },
        file_id: file.unwrap_or("").into(),
        version_id: file.map(|id| format!("version_{id}")).unwrap_or_default(),
        size: u64::from(file.is_some()),
    }
}

fn fixture() -> (
    State,
    State,
    Repository,
    Vec<Commit>,
    BTreeMap<String, String>,
) {
    let target = Repository {
        repository_id: "repo_target".into(),
        history_tree_id: "history_target".into(),
        main_branch_id: "branch_target".into(),
        name: "target".into(),
        primary_slot_id: "P01".into(),
        target_copies: 1,
        name_policy: "portable".into(),
        unicode_version: "fixed".into(),
        smb_enabled: false,
        smb_binding_generation: 0,
        file_manager_root: false,
    };
    let entries = vec![
        entry("Docs", None),
        entry("Docs/alias.txt", Some("file_a")),
        entry("Docs/empty", None),
        entry("Docs/nested", None),
        entry("Docs/nested/A.txt", Some("file_a")),
        entry("Docs/nested/B.txt", Some("file_b")),
        entry("Docs-other", None),
        entry("Docs-other/B.txt", Some("file_b")),
        entry("Unicode", None),
        entry("Unicode/é.txt", Some("file_a")),
    ];
    let root = Commit {
        commit_id: "commit_source".into(),
        history_tree_id: "history_source".into(),
        kind: "repository_root".into(),
        parent_commit_ids: Vec::new(),
        tree_id: "tree_source".into(),
        sequence: 7,
        operation_id: OperationId::generate().unwrap(),
        author: "original author".into(),
        message: "original message".into(),
        entries: entries.clone(),
        namespace_deletions: None,
        pending_file_versions: [
            ("file_a".into(), "version_file_a".into()),
            ("file_b".into(), "version_file_b".into()),
        ]
        .into(),
        provenance: Some("outside".into()),
    };
    let directory = Commit {
        commit_id: "commit_directory".into(),
        tree_id: "tree_directory".into(),
        kind: "directory_commit".into(),
        parent_commit_ids: vec![root.commit_id.clone()],
        provenance: Some("dtree_retained".into()),
        ..root.clone()
    };
    let empty = Commit {
        commit_id: "commit_empty".into(),
        tree_id: "tree_empty".into(),
        kind: "directory_commit".into(),
        entries: Vec::new(),
        pending_file_versions: BTreeMap::new(),
        provenance: Some("dtree_retained".into()),
        ..root.clone()
    };
    let deletion = Commit {
        commit_id: "commit_deletion".into(),
        tree_id: "tree_deletion".into(),
        kind: "namespace_delete".into(),
        parent_commit_ids: vec![root.commit_id.clone(), directory.commit_id.clone()],
        namespace_deletions: Some(NamespaceDeletions {
            timestamp: 123,
            paths: [
                "Docs/nested",
                "docs/ALIAS.txt",
                "Docs/empty",
                "Docs-other",
                "missing",
            ]
            .into_iter()
            .map(|path| DeletedPath {
                path: path.into(),
                kind: NamespaceEntryKind::Directory,
            })
            .collect(),
        }),
        ..root.clone()
    };
    let commits = vec![root, directory, empty, deletion];
    let mut ids = BTreeMap::new();
    for commit in &commits {
        for id in [&commit.commit_id, &commit.tree_id] {
            ids.insert(id.clone(), format!("mapped-{id}"));
        }
    }
    for entry in &entries {
        for id in [&entry.entry_id, &entry.file_id, &entry.version_id] {
            if !id.is_empty() {
                ids.insert(id.clone(), format!("mapped-{id}"));
            }
        }
    }
    ids.insert("dtree_retained".into(), "mapped-dtree_retained".into());
    let mut previous = State::default();
    for commit in &commits {
        previous
            .commits
            .insert(commit.commit_id.clone(), commit.clone());
    }
    let mut next = State::default();
    next.histories.insert(
        target.history_tree_id.clone(),
        HistoryTree {
            history_tree_id: target.history_tree_id.clone(),
            repository_id: target.repository_id.clone(),
            root_commit_id: "commit_target_root".into(),
            main_branch_id: Some(target.main_branch_id.clone()),
            generation: 1,
            state: HistoryState::Active,
            provenance: None,
        },
    );
    (previous, next, target, commits, ids)
}

#[test]
fn aggregate_commit_projection_matches_original_for_every_member_subset() {
    let (previous, initial, target, commits, ids) = fixture();
    let before = json_codec::to_vec(&previous).unwrap();
    for files in 0..4 {
        for directories in 0..8 {
            for retained in [false, true] {
                let scope = CommitScope {
                    ids: &ids,
                    files: ["file_a", "file_b"]
                        .into_iter()
                        .enumerate()
                        .filter(|(bit, _)| files & (1 << bit) != 0)
                        .map(|(_, id)| id)
                        .collect(),
                    directories: ["entry_Docs/empty", "entry_Docs/nested", "entry_Docs-other"]
                        .into_iter()
                        .enumerate()
                        .filter(|(bit, _)| directories & (1 << bit) != 0)
                        .map(|(_, id)| id)
                        .collect(),
                    trees: if retained {
                        ["dtree_retained"].into()
                    } else {
                        BTreeSet::new()
                    },
                    missing: invalid,
                };
                let mut expected = initial.clone();
                for original in &commits {
                    let commit = aggregate_oracle(
                        &previous,
                        &target,
                        original,
                        &scope,
                        "commit_target_root",
                    )
                    .unwrap();
                    expected.commits.insert(commit.commit_id.clone(), commit);
                }
                let mut actual = initial.clone();
                project(&previous, &mut actual, &target, &commits, scope).unwrap();
                assert_eq!(
                    json_codec::to_vec(&actual).unwrap(),
                    json_codec::to_vec(&expected).unwrap()
                );
            }
        }
    }
    assert_eq!(json_codec::to_vec(&previous).unwrap(), before);
}

#[test]
fn single_file_commit_projection_matches_original_and_preserves_error_category() {
    let (previous, initial, target, commits, ids) = fixture();
    for file in ["file_a", "file_b"] {
        let scope = CommitScope {
            ids: &ids,
            files: [file].into(),
            directories: BTreeSet::new(),
            trees: BTreeSet::new(),
            missing: unsupported,
        };
        let mut expected = initial.clone();
        for original in &commits {
            let commit =
                single_oracle(&previous, &target, original, &scope, "commit_target_root").unwrap();
            expected.commits.insert(commit.commit_id.clone(), commit);
        }
        let mut actual = initial.clone();
        project(&previous, &mut actual, &target, &commits, scope).unwrap();
        assert_eq!(
            json_codec::to_vec(&actual).unwrap(),
            json_codec::to_vec(&expected).unwrap()
        );
    }
    for missing in [unsupported as fn() -> Error, invalid] {
        for removed in [
            "commit_directory",
            "tree_directory",
            "commit_source",
            "entry_Docs",
            "file_a",
            "version_file_a",
        ] {
            let mut incomplete = ids.clone();
            incomplete.remove(removed);
            let scope = CommitScope {
                ids: &incomplete,
                files: ["file_a"].into(),
                directories: BTreeSet::new(),
                trees: BTreeSet::new(),
                missing,
            };
            let mut actual = initial.clone();
            let error = project(&previous, &mut actual, &target, [&commits[1]], scope).unwrap_err();
            assert_eq!(error.code(), missing().code(), "missing {removed}");
            assert_eq!(
                error.to_string(),
                missing().to_string(),
                "missing {removed}"
            );
            assert_eq!(
                json_codec::to_vec(&actual).unwrap(),
                json_codec::to_vec(&initial).unwrap()
            );
        }
    }
}
