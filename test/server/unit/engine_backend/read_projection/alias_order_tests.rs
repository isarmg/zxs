use super::*;

fn register(
    engine: &mut Engine,
    repo: &storage_engine::Repository,
    path: &str,
    name: &str,
) -> String {
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse(path).unwrap()),
            &storage_contract::BranchName::parse(name).unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn select(engine: &mut Engine, repo: &storage_engine::Repository, path: &str, name: &str) {
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse(path).unwrap()),
            &storage_contract::BranchName::parse(name).unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
}

/// Retain the original nested alias while moving another directory branch's
/// same identity to a punctuation-prefix sibling, then inherit a newer body
/// there. Both aliases are created through the public mutation APIs.
fn inherited_aliases(engine: &mut Engine, repo: &storage_engine::Repository) -> String {
    save_projection_file(engine, &repo.main_branch_id, "a/c.txt", b"nested original");
    save_projection_file(engine, &repo.main_branch_id, "a-b/stay.txt", b"sibling");
    let moving = register(engine, repo, "a", "moving");
    register(engine, repo, "a", "retained");
    let target = register(engine, repo, "a-b", "target");
    let original = engine.state().directory_branches[&moving]
        .entries
        .iter()
        .find(|entry| entry.path() == "a/c.txt")
        .unwrap()
        .clone();
    let source_tree = engine.state().directory_branches[&moving]
        .head_tree_id
        .clone();
    let target_tree = engine.state().directory_branches[&target]
        .head_tree_id
        .clone();
    engine
        .directory_namespace_batch(
            &repo.repository_id,
            &storage_contract::DirectoryNamespaceBatch {
                kind: storage_contract::DirectoryBatchKind::Move,
                items: vec![storage_contract::DirectoryNamespaceItem {
                    source: storage_contract::DirectoryNamespaceSource {
                        directory_branch_id: moving,
                        expected_tree_id: source_tree,
                        path: "a/c.txt".into(),
                        entry_id: original.entry_id,
                    },
                    target: Some(storage_contract::DirectoryNamespaceTarget {
                        directory_branch_id: target,
                        expected_tree_id: target_tree,
                        path: "a-b/report.txt".into(),
                    }),
                }],
            },
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    select(engine, repo, "a", "retained");
    select(engine, repo, "a-b", "target");
    let branch = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("punctuation draft").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .select_file_branch_at(
            &repo.repository_id,
            &name_policy::PortablePath::parse("a-b/report.txt").unwrap(),
            &original.file_id,
            &branch,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_projection_file(
        engine,
        &repo.main_branch_id,
        "a-b/report.txt",
        b"new sibling body",
    );
    register(engine, repo, "a-b", "inherited newer");
    select(engine, repo, "a-b", "inherited newer");
    original.file_id
}

fn assert_alias_sources(
    engine: &Engine,
    repo: &storage_engine::Repository,
    commit_id: &str,
    file: &str,
    expected_version: Option<&str>,
) {
    let commit = &engine.state().commits[commit_id];
    let mut entries = engine
        .list_tree_at(&repo.main_branch_id, commit_id)
        .unwrap();
    // Independent legacy flat oracle: do not assume the walk under test has
    // already returned entries in the right order.
    entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    let expected: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|entry| {
            legacy_pending_entry_source(engine, &repo.main_branch_id, commit, &entries, entry)
                .map(|source| (entry.file_id.clone(), source.to_owned()))
        })
        .collect();
    if let Some(expected_version) = expected_version {
        assert_eq!(expected[file], expected_version);
    }
    let resolver = PendingSources::new(engine, &repo.main_branch_id, commit);
    assert_eq!(resolver.pending_versions().unwrap(), expected);
    let snapshot = crate::TreeSnapshot::capture(
        engine,
        crate::tests::tree_request(&json!({
            "branch_id": repo.main_branch_id, "query": {"commit_id": commit_id}
        })),
    )
    .unwrap();
    assert_eq!(snapshot.entries, entries);
    assert_eq!(snapshot.pending_file_versions, expected);
    let first = entries.iter().find(|entry| entry.file_id == file).unwrap();
    assert_eq!(first.path(), "a-b/report.txt");
    assert_eq!(
        engine
            .commit_namespace(commit_id)
            .unwrap()
            .find_file(file)
            .unwrap()
            .as_ref(),
        Some(first)
    );
}

#[test]
fn punctuation_aliases_preserve_first_path_and_last_inherited_source() {
    let (root, mut engine, repo, _) = fixture();
    let file = inherited_aliases(&mut engine, &repo);
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let aliases: Vec<_> = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.file_id == file)
        .collect();
    assert_eq!(aliases.len(), 2);
    assert_eq!(aliases[0].entry_id, aliases[1].entry_id);
    assert_ne!(aliases[0].version_id, aliases[1].version_id);
    for alias in &aliases {
        assert!(
            engine
                .file_context(&repo.repository_id, &alias.path)
                .unwrap()
                .unwrap()
                .inherited_from
                .is_some()
        );
    }
    let last_version = aliases
        .iter()
        .find(|entry| entry.path() == "a/c.txt")
        .unwrap()
        .version_id
        .clone();
    assert_alias_sources(&engine, &repo, &head, &file, Some(&last_version));
    // After the main head moves, preserve the legacy resolver's last-wins
    // rule among the contexts present in the frozen commit and directory trees.
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "unrelated.txt",
        b"advance head",
    );
    assert_alias_sources(&engine, &repo, &head, &file, None);
    drop(engine);
    let recovered = Engine::open(&root.path().join("system")).unwrap();
    assert_alias_sources(&recovered, &repo, &head, &file, None);
}
