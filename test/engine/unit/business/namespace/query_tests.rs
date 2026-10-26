use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    query_normalizations: usize,
    key_comparisons: usize,
}

std::thread_local! {
    static WORK: Cell<Work> = const { Cell::new(Work {
        query_normalizations: 0,
        key_comparisons: 0,
    }) };
}

pub(super) fn record_normalization() {
    WORK.with(|work| {
        let mut value = work.get();
        value.query_normalizations += 1;
        work.set(value);
    });
}

pub(super) fn record_comparison() {
    WORK.with(|work| {
        let mut value = work.get();
        value.key_comparisons += 1;
        work.set(value);
    });
}

fn take_work() -> Work {
    WORK.with(|work| work.replace(Work::default()))
}

fn entry(path: &str, kind: NamespaceEntryKind) -> Entry {
    Entry {
        entry_id: format!("entry:{path}"),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        lookup_key: name_policy::path_key(path),
        kind,
        file_id: format!("file:{path}"),
        version_id: format!("version:{path}"),
        size: 0,
    }
}

// The former SMB lookup normalized every visited display path.
fn legacy_lookup<'a>(entries: &'a [Entry], path: &str, visits: &mut usize) -> Option<&'a Entry> {
    let key = name_policy::path_key(path);
    entries.iter().find(|entry| {
        *visits += 1;
        name_policy::path_key(&entry.path) == key
    })
}

#[test]
fn canonical_lookup_matches_legacy_unicode_case_aliases_and_absence() {
    let mut entries = vec![
        entry("Straße", NamespaceEntryKind::Directory),
        entry("Straße/Café.txt", NamespaceEntryKind::File),
        entry("Σ", NamespaceEntryKind::Directory),
        entry("Σ/内容.txt", NamespaceEntryKind::File),
        entry("Ａ.txt", NamespaceEntryKind::File),
        entry("A.txt", NamespaceEntryKind::File),
        entry("𐐀.txt", NamespaceEntryKind::File),
    ];
    let mut alias = entries[1].clone();
    relocate_entry(&mut alias, "Alias.txt".into()).unwrap();
    entries.push(alias);
    validate_entries(&mut entries, 100).unwrap();
    let tree = NamespaceTree::from_published_entries(&entries);
    for path in [
        "strasse/CAFE\u{301}.TXT",
        "STRASSE",
        "ς/内容.txt",
        "σ/内容.txt",
        "ａ.TXT",
        "a.txt",
        "𐐨.TXT",
        "alias.TXT",
        "",
        "absent",
        "Straße-other",
    ] {
        let expected = legacy_lookup(&entries, path, &mut 0);
        assert_eq!(tree.get(path), expected, "{path}");
        assert_eq!(
            tree.get_key(&name_policy::path_key(path)),
            expected,
            "{path}"
        );
    }
    let original = tree.get("STRASSE/CAFÉ.TXT").unwrap();
    let alias = tree.get("ALIAS.TXT").unwrap();
    assert_eq!(original.file_id, alias.file_id);
    assert_ne!(original.path, alias.path);
    assert_eq!(original.path, "Straße/Café.txt");
    assert_eq!(original.name, "Café.txt");
}

#[test]
fn lookup_work_is_logarithmic_and_only_normalizes_the_query() {
    for count in [1_000usize, 10_000, 100_000] {
        let entries = (0..count)
            .map(|index| entry(&format!("file-{index:06}"), NamespaceEntryKind::File))
            .collect::<Vec<_>>();
        take_work();
        let tree = NamespaceTree::from_published_entries(&entries);
        assert_eq!(
            take_work(),
            Work::default(),
            "borrowing must not scan the tree"
        );
        let queries = [
            "FILE-000000".to_owned(),
            format!("FILE-{:06}", count - 1),
            "missing".into(),
        ];
        let mut legacy_entry_normalizations = 0;
        for query in &queries {
            let expected = legacy_lookup(&entries, query, &mut legacy_entry_normalizations);
            assert_eq!(tree.get(query), expected);
        }
        let actual = take_work();
        assert_eq!(actual.query_normalizations, queries.len());
        let comparison_bound = (usize::BITS - (count - 1).leading_zeros()) as usize + 1;
        assert!(actual.key_comparisons <= queries.len() * comparison_bound);
        assert_eq!(legacy_entry_normalizations, 2 * count + 1);
        for query in &queries {
            tree.get_key(&name_policy::path_key(query));
        }
        assert_eq!(take_work().query_normalizations, 0);
        println!(
            "N={count}: old stored-path normalizations={legacy_entry_normalizations}; new query normalizations={}, key comparisons={}",
            actual.query_normalizations, actual.key_comparisons
        );
    }
}

#[test]
fn temporary_appends_and_renames_stay_unordered_until_validation() {
    let mut entries = vec![entry("Z", NamespaceEntryKind::Directory)];
    ensure_parents(&mut entries, "A/B/file.txt").unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["Z", "A", "A/B"]
    );
    let parent_id = find_unordered(&entries, "a/b").unwrap().entry_id.clone();
    ensure_parents(&mut entries, "a/b/other.txt").unwrap();
    assert_eq!(find_unordered(&entries, "A/B").unwrap().entry_id, parent_id);
    apply_namespace(
        &mut entries,
        &NamespaceOperation {
            kind: NamespaceOperationKind::Mkdir,
            path: "a/B/C".into(),
            destination: None,
        },
    )
    .unwrap();
    apply_namespace(
        &mut entries,
        &NamespaceOperation {
            kind: NamespaceOperationKind::Rename,
            path: "A".into(),
            destination: Some("0".into()),
        },
    )
    .unwrap();
    assert_eq!(find_unordered(&entries, "0/b/c").unwrap().path, "0/B/C");
    assert_eq!(
        apply_namespace(
            &mut entries,
            &NamespaceOperation {
                kind: NamespaceOperationKind::Mkdir,
                path: "0/b".into(),
                destination: None,
            }
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::NameCollision
    );
    let mutation_order = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(mutation_order, ["Z", "0", "0/B", "0/B/C"]);
    let draft = NamespaceDraft::new(entries);
    assert_eq!(draft.get("0/b").unwrap().entry_id, parent_id);
    let mut entries = draft.into_entries();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        mutation_order
    );
    validate_entries(&mut entries, 100).unwrap();
    let tree = NamespaceTree::from_published_entries(&entries);
    assert_eq!(tree.get("0/B").unwrap().entry_id, parent_id);
    assert!(
        entries
            .windows(2)
            .all(|pair| pair[0].lookup_key < pair[1].lookup_key)
    );
}

fn published_fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Lookup", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["Zed.txt", "Straße/Café.txt", "Alpha.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, path.as_bytes())
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "lookup fixture".into(),
            request_digest: None,
        })
        .unwrap();
    (temporary, engine, repo)
}

fn assert_published_matches_legacy(engine: &Engine, branch: &str) {
    let tree = engine.namespace_tree(branch).unwrap();
    let entries = engine.list_tree_ref(branch).unwrap();
    assert_eq!(tree.entries().as_ptr(), entries.as_ptr());
    assert!(
        entries
            .windows(2)
            .all(|pair| pair[0].lookup_key < pair[1].lookup_key)
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.lookup_key == name_policy::path_key(&entry.path))
    );
    for query in [
        "STRASSE/CAFE\u{301}.TXT",
        "MOVED/CAFE\u{301}.TXT",
        "ALPHA.TXT",
        "ZED.TXT",
        "missing",
    ] {
        assert_eq!(tree.get(query), legacy_lookup(entries, query, &mut 0));
    }
}

#[test]
fn published_and_reopened_trees_preserve_unicode_identity_and_historical_lookup() {
    let (temporary, mut engine, repo) = published_fixture();
    let branch = &repo.main_branch_id;
    let original = engine
        .namespace_tree(branch)
        .unwrap()
        .get("strasse/CAFÉ.txt")
        .unwrap()
        .clone();
    let original_head = engine.state().branches[branch].head_commit_id.clone();
    assert_published_matches_legacy(&engine, branch);
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: "STRASSE".into(),
                destination: Some("Moved".into()),
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let renamed = engine
        .namespace_tree(branch)
        .unwrap()
        .get("moved/Cafe\u{301}.txt")
        .unwrap();
    assert_eq!(renamed.file_id, original.file_id);
    assert_eq!(renamed.entry_id, original.entry_id);
    assert_eq!(renamed.version_id, original.version_id);
    assert_eq!(
        engine
            .namespace_tree_at(branch, &original_head)
            .unwrap()
            .get("STRASSE/CAFÉ.TXT"),
        Some(&original)
    );
    let collision = engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: NamespaceOperationKind::Mkdir,
                path: "MOVED".into(),
                destination: None,
            },
            "admin",
            &new_id("op"),
        )
        .unwrap_err();
    assert_eq!(collision.code(), storage_contract::ErrorCode::NameCollision);
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert_published_matches_legacy(&engine, branch);
    assert_eq!(
        engine
            .namespace_tree_at(branch, &original_head)
            .unwrap()
            .get("STRASSE/CAFÉ.TXT"),
        Some(&original)
    );
    let missing = engine.namespace_tree("missing").err().unwrap();
    assert_eq!(
        missing.code(),
        engine.list_tree_ref("missing").unwrap_err().code()
    );
    let missing = engine.namespace_tree_at(branch, "missing").err().unwrap();
    assert_eq!(
        missing.code(),
        engine
            .list_tree_at_ref(branch, "missing")
            .unwrap_err()
            .code()
    );
}

#[test]
fn publication_rejects_bad_keys_unsorted_entries_and_normalized_collisions() {
    let (temporary, mut engine, repo) = published_fixture();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let before = canonical_json(engine.state()).unwrap();
    for corruption in [
        "key",
        "name",
        "order",
        "duplicate",
        "folded_duplicate",
        "nfd_display",
    ] {
        let operation = new_id("op");
        let result = engine.mutate(
            "test.invalid.namespace",
            "admin",
            &operation,
            json_codec::json!({"corruption":corruption}),
            |_, state, _| {
                let entries = &mut state.commits.get_mut(&head).unwrap().entries;
                match corruption {
                    "key" => entries[0].lookup_key = "deliberately-unrelated-key".into(),
                    "name" => entries[0].name = "wrong.txt".into(),
                    "order" => entries.reverse(),
                    "duplicate" => entries.push(entries[0].clone()),
                    "folded_duplicate" => {
                        let mut duplicate = entries
                            .iter()
                            .find(|entry| entry.path == "Straße")
                            .unwrap()
                            .clone();
                        let path = duplicate.path.to_uppercase();
                        relocate_entry(&mut duplicate, path).unwrap();
                        entries.push(duplicate);
                    }
                    "nfd_display" => {
                        let entry = entries
                            .iter_mut()
                            .find(|entry| entry.path == "Straße/Café.txt")
                            .unwrap();
                        entry.path = "Straße/Cafe\u{301}.txt".into();
                        entry.name = "Cafe\u{301}.txt".into();
                    }
                    _ => unreachable!(),
                }
                Ok(json_codec::json!({}))
            },
        );
        let expected = if corruption.contains("duplicate") {
            storage_contract::ErrorCode::NameCollision
        } else if corruption == "nfd_display" {
            storage_contract::ErrorCode::NameNotNormalized
        } else {
            storage_contract::ErrorCode::HistoryPackageInvalid
        };
        assert_eq!(result.unwrap_err().code(), expected, "{corruption}");
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert!(!engine.state().operations.contains_key(&operation));
    }
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_published_matches_legacy(&engine, &repo.main_branch_id);
}

#[test]
fn query_views_keep_missing_foreign_and_migrated_branch_errors() {
    let (_temporary, mut engine, repo) = published_fixture();
    let other = engine
        .create_repository("Other lookup", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let foreign = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    assert!(
        engine
            .namespace_tree(&other.main_branch_id)
            .unwrap()
            .get("anything")
            .is_none()
    );
    for (branch, commit) in [
        ("missing", foreign.as_str()),
        (repo.main_branch_id.as_str(), "missing"),
        (repo.main_branch_id.as_str(), foreign.as_str()),
    ] {
        let old = engine.list_tree_at_ref(branch, commit).err().unwrap();
        let new = engine.namespace_tree_at(branch, commit).err().unwrap();
        assert_eq!(new.code(), old.code());
        assert_eq!(new.to_string(), old.to_string());
    }
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .state = BranchState::Migrated;
    let old = engine.list_tree_ref(&repo.main_branch_id).err().unwrap();
    let new = engine.namespace_tree(&repo.main_branch_id).err().unwrap();
    assert_eq!(new.code(), storage_contract::ErrorCode::TreeMoved);
    assert_eq!(new.to_string(), old.to_string());
}
