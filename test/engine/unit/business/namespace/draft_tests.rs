use super::*;

fn entry(path: &str, kind: NamespaceEntryKind) -> Entry {
    Entry {
        entry_id: format!("entry:{path}"),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        lookup_key: name_policy::path_key(path),
        kind,
        file_id: if kind == NamespaceEntryKind::File {
            format!("file:{path}")
        } else {
            String::new()
        },
        version_id: if kind == NamespaceEntryKind::File {
            format!("version:{path}")
        } else {
            String::new()
        },
        size: 0,
    }
}

#[test]
fn unsorted_sources_and_replacements_preserve_legacy_order_and_aliases() {
    let mut alias = entry("Alias.txt", NamespaceEntryKind::File);
    let source = entry("Straße.txt", NamespaceEntryKind::File);
    alias.file_id = source.file_id.clone();
    alias.version_id = source.version_id.clone();
    let original = vec![
        entry("Z.txt", NamespaceEntryKind::File),
        source,
        alias,
        entry("A.txt", NamespaceEntryKind::File),
    ];
    let mut legacy = original.clone();
    let mut draft = NamespaceDraft::new(original.clone());
    for path in ["STRASSE.TXT", "z.TXT", "straße.txt"] {
        let mut replacement = draft.get(path).unwrap().clone();
        assert_eq!(
            Some(&replacement),
            super::super::find_unordered(&legacy, path)
        );
        replacement.version_id.push_str("-new");
        legacy.retain(|entry| entry.lookup_key != replacement.lookup_key);
        legacy.push(replacement.clone());
        draft.replace(replacement);
    }
    assert_eq!(draft.get("ALIAS.TXT"), Some(&original[2]));
    assert_eq!(draft.into_entries(), legacy);
}

#[test]
fn lookup_accepts_equivalent_queries_over_admitted_sources() {
    let original = vec![
        entry("Café", NamespaceEntryKind::Directory),
        entry("Café/Straße.txt", NamespaceEntryKind::File),
    ];
    let draft = NamespaceDraft::new(original.clone());
    assert_eq!(draft.get("Cafe\u{301}/STRASSE.TXT"), Some(&original[1]));
    assert!(draft.get("absent").is_none());
    assert_eq!(draft.into_entries(), original);
}

#[test]
fn shared_parent_creation_matches_legacy_order_and_preserves_existing_identity() {
    let source = vec![
        entry("Z.txt", NamespaceEntryKind::File),
        entry("Existing", NamespaceEntryKind::Directory),
    ];
    let mut legacy = source.clone();
    let mut draft = NamespaceDraft::new(source);
    for path in [
        "New/Nested/a.txt",
        "New/Nested/b.txt",
        "existing/Child/c.txt",
    ] {
        super::super::ensure_parents(&mut legacy, path).unwrap();
        draft.ensure_parents(path).unwrap();
        let file = entry(path, NamespaceEntryKind::File);
        legacy.retain(|previous| previous.lookup_key != file.lookup_key);
        legacy.push(file.clone());
        draft.replace(file);
    }
    let mut actual = draft.into_entries();
    // Newly materialized parent identities are independently random; all other
    // fields and the pre-existing parent identity must match exactly.
    for (actual, expected) in actual.iter_mut().zip(&legacy) {
        if actual.kind == NamespaceEntryKind::Directory && actual.path != "Existing" {
            actual.entry_id = expected.entry_id.clone();
        }
    }
    assert_eq!(actual, legacy);
    super::super::validate_entries(&mut actual, 100).unwrap();
}

#[test]
fn parent_file_errors_do_not_replace_or_drop_the_existing_entry() {
    let original = vec![entry("File", NamespaceEntryKind::File)];
    let mut draft = NamespaceDraft::new(original.clone());
    assert_eq!(
        draft.ensure_parents("file/child").unwrap_err().code(),
        storage_contract::ErrorCode::ParentNotDirectory
    );
    assert_eq!(draft.into_entries(), original);
}

#[test]
#[should_panic(expected = "invariant: admitted namespace lookup keys are unique")]
fn duplicate_admitted_keys_are_an_internal_bug() {
    let valid = entry("Straße.txt", NamespaceEntryKind::File);
    NamespaceDraft::new(vec![valid.clone(), valid]);
}

#[test]
fn flattened_draft_keeps_namespace_operations_and_deletion_order_unchanged() {
    let source = vec![
        entry("Folder", NamespaceEntryKind::Directory),
        entry("Folder/z", NamespaceEntryKind::File),
        entry("Folder/a", NamespaceEntryKind::File),
    ];
    let mut legacy = source.clone();
    let mut draft = NamespaceDraft::new(source.clone());
    let mut replacement = source[1].clone();
    replacement.version_id = "changed".into();
    legacy.retain(|entry| entry.lookup_key != replacement.lookup_key);
    legacy.push(replacement.clone());
    draft.replace(replacement);
    let operations = [
        NamespaceOperation {
            kind: NamespaceOperationKind::Rename,
            path: "FOLDER".into(),
            destination: Some("Renamed".into()),
        },
        NamespaceOperation {
            kind: NamespaceOperationKind::DeleteTree,
            path: "RENAMED".into(),
            destination: None,
        },
    ];
    let mut actual = draft.into_entries();
    let expected_deleted =
        super::super::apply_workspace_namespace(&mut legacy, &source, &operations).unwrap();
    let actual_deleted =
        super::super::apply_workspace_namespace(&mut actual, &source, &operations).unwrap();
    assert_eq!(actual_deleted, expected_deleted);
    assert_eq!(actual, legacy);
    super::super::validate_entries(&mut actual, 100).unwrap();
}

#[test]
fn thousand_to_hundred_thousand_replacements_have_linear_draft_work() {
    for count in [1_000, 10_000, 100_000] {
        let entries = (0..count)
            .rev()
            .map(|i| entry(&format!("file-{i:06}"), NamespaceEntryKind::File))
            .collect();
        let mut draft = NamespaceDraft::new(entries);
        let work = draft.work.clone();
        // Same two lookup passes and replace operations as workspace commit.
        for i in 0..count {
            assert!(draft.get(&format!("FILE-{i:06}")).is_some());
        }
        for i in 0..count {
            let path = format!("file-{i:06}");
            let mut replacement = draft.get(&path).unwrap().clone();
            replacement.version_id.push_str("-updated");
            draft.ensure_parents(&path).unwrap();
            draft.replace(replacement);
        }
        assert!(draft.get("missing").is_none());
        let entries = draft.into_entries();
        assert_eq!(entries.len(), count);
        assert_eq!(
            work.get(),
            Work {
                source_visits: count,
                map_operations: 4 * count + 1,
                appended_slots: count,
                flattened_slots: 2 * count,
            }
        );
        // Counts measure actual draft operations, not BTreeMap comparisons.
        // Each map operation has the standard library's logarithmic bound.
        assert_eq!(entries[0].path, "file-000000");
        assert_eq!(entries[count - 1].path, format!("file-{:06}", count - 1));
    }
}

#[test]
fn hundred_thousand_shared_parent_writes_do_not_rescan_siblings() {
    let count = 100_000;
    let mut draft = NamespaceDraft::new(Vec::new());
    let work = draft.work.clone();
    for i in 0..count {
        let path = format!("Parent/Shared/file-{i:06}");
        assert!(draft.get(&path).is_none());
        draft.ensure_parents(&path).unwrap();
        draft.replace(entry(&path, NamespaceEntryKind::File));
    }
    let entries = draft.into_entries();
    assert_eq!(entries.len(), count + 2);
    assert_eq!(
        work.get(),
        Work {
            source_visits: 0,
            map_operations: 4 * count + 2,
            appended_slots: count + 2,
            flattened_slots: count + 2,
        }
    );
}

#[test]
#[should_panic(expected = "invariant: admitted namespace lookup keys are unique")]
fn workspace_commit_exposes_corrupted_admitted_source() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Draft", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let initial = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&initial.workspace_id, "Original.txt", b"original")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: initial.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: initial.base_head,
            fencing_token: initial.fencing_token,
            message: "initial".into(),
            request_digest: None,
        })
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "New.txt", b"staged bytes")
        .unwrap();
    // Inject corruption only in test memory, not via a read/load admission path.
    let source = std::sync::Arc::make_mut(&mut engine.state)
        .commits
        .get_mut(&workspace.base_head)
        .unwrap();
    source.entries.push(source.entries[0].clone());
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "must not publish".into(),
            request_digest: None,
        })
        .unwrap();
}
