use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    query_normalizations: usize,
}

std::thread_local! {
    static WORK: Cell<Work> = const { Cell::new(Work {
        query_normalizations: 0,
    }) };
}

pub(super) fn record_normalization() {
    WORK.with(|work| {
        let mut value = work.get();
        value.query_normalizations += 1;
        work.set(value);
    });
}

fn take_work() -> Work {
    WORK.with(|work| work.replace(Work::default()))
}

fn entry(path: &str, kind: NamespaceEntryKind) -> Entry {
    // The paged wire cannot carry payload fields on directories.
    let file = matches!(kind, NamespaceEntryKind::File);
    Entry {
        entry_id: format!("entry:{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind,
        file_id: if file {
            format!("file:{path}")
        } else {
            String::new()
        },
        version_id: if file {
            format!("version:{path}")
        } else {
            String::new()
        },
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

/// Store a hand-built namespace on the engine's members, so the ordinary
/// paged lookup path (bounded cache over published replicas) serves it.
fn planted_tree<'a>(engine: &'a Engine, entries: &[Entry]) -> NamespaceTree<'a> {
    let (namespace, mut objects) = crate::namespace::fixture_namespace(entries);
    let members = engine.members();
    for object in objects.take() {
        object.store(&members).unwrap();
    }
    NamespaceTree {
        engine,
        namespace,
        members,
    }
}

#[test]
fn canonical_lookup_matches_legacy_unicode_case_aliases_and_absence() {
    let (_temporary, engine, _repo) = published_fixture();
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
    relocate_entry(
        &mut alias,
        name_policy::PortablePath::parse("Alias.txt").unwrap(),
    );
    entries.push(alias);
    validate_entries(&mut entries, 100).unwrap();
    let tree = planted_tree(&engine, &entries);
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
        let expected = legacy_lookup(&entries, path, &mut 0).cloned();
        assert_eq!(tree.get(path).unwrap(), expected, "{path}");
        assert_eq!(
            tree.get_key(&name_policy::path_key(path)).unwrap(),
            expected,
            "{path}"
        );
    }
    let original = tree.get("STRASSE/CAFÉ.TXT").unwrap().unwrap();
    let alias = tree.get("ALIAS.TXT").unwrap().unwrap();
    assert_eq!(original.file_id, alias.file_id);
    assert_ne!(original.path, alias.path);
    assert_eq!(original.path(), "Straße/Café.txt");
    assert_eq!(original.name(), "Café.txt");
}

#[test]
fn lookup_work_is_bounded_and_only_normalizes_the_query() {
    let (_temporary, engine, _repo) = published_fixture();
    for count in [1_000usize, 10_000] {
        let entries = (0..count)
            .map(|index| entry(&format!("file-{index:06}"), NamespaceEntryKind::File))
            .collect::<Vec<_>>();
        take_work();
        let tree = planted_tree(&engine, &entries);
        assert_eq!(
            take_work(),
            Work::default(),
            "building must not normalize any query"
        );
        let queries = [
            "FILE-000000".to_owned(),
            format!("FILE-{:06}", count - 1),
            "missing".into(),
        ];
        let mut legacy_entry_normalizations = 0;
        for query in &queries {
            let expected = legacy_lookup(&entries, query, &mut legacy_entry_normalizations);
            assert_eq!(tree.get(query).unwrap().as_ref(), expected);
        }
        let actual = take_work();
        // Point lookups normalize the query string once and descend the
        // index; they never fold every stored display path like the legacy
        // flat scan did.
        assert_eq!(actual.query_normalizations, queries.len());
        assert_eq!(legacy_entry_normalizations, 2 * count + 1);
        for query in &queries {
            tree.get_key(&name_policy::path_key(query)).unwrap();
        }
        assert_eq!(take_work().query_normalizations, 0);
        println!(
            "N={count}: old stored-path normalizations={legacy_entry_normalizations}; new query normalizations={}",
            actual.query_normalizations
        );
    }
}

#[test]
fn temporary_appends_and_renames_stay_unordered_until_validation() {
    let mut entries = vec![entry("Z", NamespaceEntryKind::Directory)];
    ensure_parents(
        &mut entries,
        &name_policy::PortablePath::parse("A/B/file.txt").unwrap(),
    )
    .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["Z", "A", "A/B"]
    );
    let parent_id = find_unordered(&entries, "a/b").unwrap().entry_id.clone();
    ensure_parents(
        &mut entries,
        &name_policy::PortablePath::parse("a/b/other.txt").unwrap(),
    )
    .unwrap();
    assert_eq!(find_unordered(&entries, "A/B").unwrap().entry_id, parent_id);
    apply_namespace(
        &mut entries,
        &NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "a/B/C", None).unwrap(),
    )
    .unwrap();
    apply_namespace(
        &mut entries,
        &NamespaceOperation::parse(NamespaceOperationKind::Rename, "A", Some("0")).unwrap(),
    )
    .unwrap();
    assert_eq!(find_unordered(&entries, "0/b/c").unwrap().path, "0/B/C");
    assert_eq!(
        apply_namespace(
            &mut entries,
            &NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "0/b", None).unwrap()
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
    assert_eq!(
        draft
            .get_key(name_policy::LookupPath::parse("0/b").unwrap().key())
            .unwrap()
            .entry_id,
        parent_id
    );
    let mut entries = draft.into_entries();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        mutation_order
    );
    validate_entries(&mut entries, 100).unwrap();
    // The validated flat list packs into a paged namespace whose point reads
    // resolve the same identity.
    let namespace = crate::namespace::fixture_store::namespace(&entries);
    let view = crate::namespace::NamespaceView::new(
        &namespace,
        &crate::namespace::fixture_store::FixtureSource,
    );
    assert_eq!(
        view.resolve_key("0/b").unwrap().unwrap().entry_id,
        parent_id
    );
    assert!(
        entries
            .windows(2)
            .all(|pair| pair[0].lookup_key() < pair[1].lookup_key())
    );
}

fn published_fixture() -> (tempfile::TempDir, Engine, Repository) {
    published_fixture_with_members(1)
}

fn published_fixture_with_members(count: usize) -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut config = Config::development(
        (0..count).map(|index| temporary.path().join(format!("member-{index}"))),
    );
    config.disks.sort_by(|a, b| b.disk_id.cmp(&a.disk_id));
    let mut engine = Engine::initialize(&temporary.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Lookup").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
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
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "lookup fixture".into(),
            request_digest: None,
        })
        .unwrap();
    (temporary, engine, repo)
}

#[test]
fn query_views_keep_sorted_members_across_initial_and_repeated_reads() {
    let (temporary, engine, repo) = published_fixture_with_members(3);
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    // The first lookup loads cold objects; subsequent consumers and views
    // share the engine's existing bounded cache.
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    for tree in [
        engine.namespace_tree(&repo.main_branch_id).unwrap(),
        engine
            .namespace_tree_at(&repo.main_branch_id, head)
            .unwrap(),
        engine.commit_namespace(head).unwrap(),
    ] {
        assert_eq!(tree.members.len(), 3);
        assert!(
            tree.members
                .windows(2)
                .all(|pair| pair[0].disk_id < pair[1].disk_id)
        );
        for _ in 0..2 {
            let file = tree.get("STRASSE/CAFE\u{301}.TXT").unwrap().unwrap();
            assert_eq!(
                tree.get_key("strasse/café.txt").unwrap(),
                Some(file.clone())
            );
            assert_eq!(
                tree.directory_page("strasse", None, 200).unwrap(),
                Some(vec![file.clone()])
            );
            assert_eq!(tree.directory_totals("strasse").unwrap(), Some((1, 1)));
            assert_eq!(tree.find_file(&file.file_id).unwrap(), Some(file));
            let entries = tree.entries().unwrap();
            let mut walked = Vec::new();
            tree.walk(|entry| {
                walked.push(entry.clone());
                Ok(true)
            })
            .unwrap();
            assert_eq!(walked, entries);
            assert_eq!(tree.total_entries(), entries.len() as u64);
        }
    }
}

fn assert_published_matches_legacy(engine: &Engine, branch: &str) {
    let tree = engine.namespace_tree(branch).unwrap();
    let entries = engine.list_tree(branch).unwrap();
    assert_eq!(tree.entries().unwrap(), entries);
    assert_eq!(tree.total_entries(), entries.len() as u64);
    assert!(
        entries
            .windows(2)
            .all(|pair| pair[0].lookup_key() < pair[1].lookup_key())
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.lookup_key() == name_policy::path_key(&entry.path))
    );
    for query in [
        "STRASSE/CAFE\u{301}.TXT",
        "MOVED/CAFE\u{301}.TXT",
        "ALPHA.TXT",
        "ZED.TXT",
        "missing",
    ] {
        assert_eq!(
            tree.get(query).unwrap(),
            legacy_lookup(&entries, query, &mut 0).cloned()
        );
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
        .unwrap();
    let original_head = engine.state().branches[branch].head_commit_id.clone();
    assert_published_matches_legacy(&engine, branch);
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "STRASSE", Some("Moved"))
                .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let renamed = engine
        .namespace_tree(branch)
        .unwrap()
        .get("moved/Cafe\u{301}.txt")
        .unwrap()
        .unwrap();
    assert_eq!(renamed.file_id, original.file_id);
    assert_eq!(renamed.entry_id, original.entry_id);
    assert_eq!(renamed.version_id, original.version_id);
    assert_eq!(
        engine
            .namespace_tree_at(branch, &original_head)
            .unwrap()
            .get("STRASSE/CAFÉ.TXT")
            .unwrap(),
        Some(original.clone())
    );
    let collision = engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "MOVED", None).unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
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
            .get("STRASSE/CAFÉ.TXT")
            .unwrap(),
        Some(original)
    );
    let missing = engine.namespace_tree("missing").err().unwrap();
    assert_eq!(
        missing.code(),
        engine.list_tree("missing").unwrap_err().code()
    );
    let missing = engine.namespace_tree_at(branch, "missing").err().unwrap();
    assert_eq!(
        missing.code(),
        engine.list_tree_at(branch, "missing").unwrap_err().code()
    );
}

#[test]
fn publication_rejects_lying_totals_and_dangling_namespace_roots() {
    let (temporary, mut engine, repo) = published_fixture();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let before = canonical_json(engine.state()).unwrap();
    for corruption in ["total", "dangling_root"] {
        let operation = storage_contract::OperationId::generate();
        let result = engine.mutate(
            "test.invalid.namespace",
            "admin",
            operation,
            json_codec::json!({"corruption":corruption}),
            |_, state, _| {
                let namespace = &mut state.commits.get_mut(&head).unwrap().namespace;
                match corruption {
                    // Pages are canonical by construction, so corruption now
                    // means a root that lies about its verified graph.
                    "total" => namespace.entries += 1,
                    "dangling_root" => namespace.root.hash = "0".repeat(64),
                    _ => unreachable!(),
                }
                Ok(json_codec::json!({}))
            },
        );
        assert!(result.is_err(), "{corruption}");
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert!(engine.published_receipt(operation).unwrap().is_none());
    }
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_published_matches_legacy(&engine, &repo.main_branch_id);
}

#[test]
fn query_views_keep_missing_foreign_and_migrated_branch_errors() {
    let (_temporary, mut engine, repo) = published_fixture_with_members(3);
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other lookup").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let foreign = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    assert!(
        engine
            .namespace_tree(&other.main_branch_id)
            .unwrap()
            .get("anything")
            .unwrap()
            .is_none()
    );
    for (branch, commit) in [
        ("missing", foreign.as_str()),
        (repo.main_branch_id.as_str(), "missing"),
        (repo.main_branch_id.as_str(), foreign.as_str()),
    ] {
        let old = engine.list_tree_at(branch, commit).err().unwrap();
        let new = engine.namespace_tree_at(branch, commit).err().unwrap();
        assert_eq!(new.code(), old.code());
        assert_eq!(new.to_string(), old.to_string());
    }
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .state = BranchState::Migrated;
    let old = engine.list_tree(&repo.main_branch_id).err().unwrap();
    let new = engine.namespace_tree(&repo.main_branch_id).err().unwrap();
    assert_eq!(new.code(), storage_contract::ErrorCode::TreeMoved);
    assert_eq!(new.to_string(), old.to_string());
}

#[test]
fn entry_wire_rejects_mismatched_keys_names_and_non_nfc_display_at_read_boundary() {
    let original = entry("Straße/Café.txt", NamespaceEntryKind::File);
    for (field, value) in [
        ("lookup_key", "unrelated"),
        ("name", "wrong.txt"),
        ("path", "Straße/Cafe\u{301}.txt"),
    ] {
        let mut wire = json_codec::to_value(&original).unwrap();
        wire[field] = value.into();
        assert!(json_codec::from_value::<Entry>(wire).is_err(), "{field}");
    }
    assert_eq!(
        json_codec::from_value::<Entry>(json_codec::to_value(&original).unwrap()).unwrap(),
        original
    );
}
