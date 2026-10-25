use super::*;

fn entry(path: &str, kind: &str) -> Entry {
    Entry {
        entry_id: format!("entry:{path}"),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        lookup_key: name_policy::path_key(path),
        kind: kind.into(),
        file_id: if kind == "file" {
            format!("file:{path}")
        } else {
            String::new()
        },
        version_id: if kind == "file" {
            format!("version:{path}")
        } else {
            String::new()
        },
        size: 0,
    }
}

#[test]
fn unsorted_sources_and_replacements_preserve_legacy_order_and_aliases() {
    let mut alias = entry("Alias.txt", "file");
    let source = entry("Straße.txt", "file");
    alias.file_id = source.file_id.clone();
    alias.version_id = source.version_id.clone();
    let original = vec![
        entry("Z.txt", "file"),
        source,
        alias,
        entry("A.txt", "file"),
    ];
    let mut legacy = original.clone();
    let mut draft = NamespaceDraft::new(original.clone()).unwrap();
    for path in ["STRASSE.TXT", "z.TXT", "straße.txt"] {
        let mut replacement = draft.get(path).unwrap().clone();
        assert_eq!(
            Some(&replacement),
            super::super::find_unordered(&legacy, path)
        );
        replacement.version_id.push_str("-new");
        legacy.retain(|entry| entry.lookup_key != replacement.lookup_key);
        legacy.push(replacement.clone());
        draft.replace(replacement).unwrap();
    }
    assert_eq!(draft.get("ALIAS.TXT"), Some(&original[2]));
    assert_eq!(draft.into_entries(), legacy);
}

#[test]
fn lookup_accepts_equivalent_queries_without_weakening_source_validation() {
    let original = vec![entry("Café", "directory"), entry("Café/Straße.txt", "file")];
    let mut draft = NamespaceDraft::new(original.clone()).unwrap();
    assert_eq!(draft.get("Cafe\u{301}/STRASSE.TXT"), Some(&original[1]));
    assert!(draft.get("absent").is_none());
    let invalid = entry("Cafe\u{301}/new.txt", "file");
    assert_eq!(
        draft.replace(invalid).unwrap_err().code(),
        storage_contract::ErrorCode::StateInvalid
    );
    assert_eq!(draft.into_entries(), original);
}

#[test]
fn shared_parent_creation_matches_legacy_order_and_preserves_existing_identity() {
    let source = vec![entry("Z.txt", "file"), entry("Existing", "directory")];
    let mut legacy = source.clone();
    let mut draft = NamespaceDraft::new(source).unwrap();
    for path in [
        "New/Nested/a.txt",
        "New/Nested/b.txt",
        "existing/Child/c.txt",
    ] {
        super::super::ensure_parents(&mut legacy, path).unwrap();
        draft.ensure_parents(path).unwrap();
        let file = entry(path, "file");
        legacy.retain(|previous| previous.lookup_key != file.lookup_key);
        legacy.push(file.clone());
        draft.replace(file).unwrap();
    }
    let mut actual = draft.into_entries();
    // Newly materialized parent identities are independently random; all other
    // fields and the pre-existing parent identity must match exactly.
    for (actual, expected) in actual.iter_mut().zip(&legacy) {
        if actual.kind == "directory" && actual.path != "Existing" {
            actual.entry_id = expected.entry_id.clone();
        }
    }
    assert_eq!(actual, legacy);
    super::super::validate_entries(&mut actual, 100).unwrap();
}

#[test]
fn parent_file_errors_do_not_replace_or_drop_the_existing_entry() {
    let original = vec![entry("File", "file")];
    let mut draft = NamespaceDraft::new(original.clone()).unwrap();
    assert_eq!(
        draft.ensure_parents("file/child").unwrap_err().code(),
        storage_contract::ErrorCode::ParentNotDirectory
    );
    assert_eq!(draft.into_entries(), original);
}

#[test]
fn malformed_sources_are_rejected_not_normalized_or_deduplicated() {
    let valid = entry("Straße.txt", "file");
    let mut wrong_key = valid.clone();
    wrong_key.lookup_key = "unrelated".into();
    let mut wrong_name = valid.clone();
    wrong_name.name = "other.txt".into();
    let mut wrong_kind = valid.clone();
    wrong_kind.kind = "unknown".into();
    for source in [
        vec![valid.clone(), valid.clone()],
        vec![valid.clone(), entry("STRASSE.TXT", "file")],
        vec![wrong_key],
        vec![wrong_name],
        vec![wrong_kind],
        vec![entry("Cafe\u{301}.txt", "file")],
        vec![entry("missing/child", "file")],
        vec![entry("parent", "file"), entry("parent/child", "file")],
    ] {
        let error = match NamespaceDraft::new(source) {
            Ok(_) => panic!("malformed source was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
    }
}

#[test]
fn flattened_draft_keeps_namespace_operations_and_deletion_order_unchanged() {
    let source = vec![
        entry("Folder", "directory"),
        entry("Folder/z", "file"),
        entry("Folder/a", "file"),
    ];
    let mut legacy = source.clone();
    let mut draft = NamespaceDraft::new(source.clone()).unwrap();
    let mut replacement = source[1].clone();
    replacement.version_id = "changed".into();
    legacy.retain(|entry| entry.lookup_key != replacement.lookup_key);
    legacy.push(replacement.clone());
    draft.replace(replacement).unwrap();
    let operations = [
        NamespaceOperation {
            kind: "rename".into(),
            path: "FOLDER".into(),
            destination: Some("Renamed".into()),
        },
        NamespaceOperation {
            kind: "delete_tree".into(),
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
            .map(|i| entry(&format!("file-{i:06}"), "file"))
            .collect();
        let mut draft = NamespaceDraft::new(entries).unwrap();
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
            draft.replace(replacement).unwrap();
        }
        assert!(draft.get("missing").is_none());
        let entries = draft.into_entries();
        assert_eq!(entries.len(), count);
        assert_eq!(
            work.get(),
            Work {
                source_visits: 2 * count,
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
    let mut draft = NamespaceDraft::new(Vec::new()).unwrap();
    let work = draft.work.clone();
    for i in 0..count {
        let path = format!("Parent/Shared/file-{i:06}");
        assert!(draft.get(&path).is_none());
        draft.ensure_parents(&path).unwrap();
        draft.replace(entry(&path, "file")).unwrap();
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
fn workspace_commit_rejects_duplicate_source_without_publishing_or_losing_staged_bytes() {
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
    let before = canonical_json(engine.state()).unwrap();
    let staged = canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap();
    let error = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "must not publish".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        staged
    );
}
