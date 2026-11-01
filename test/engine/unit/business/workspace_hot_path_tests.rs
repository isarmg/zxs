//! Exact namespace bindings and unchanged-table ownership in real commit plans.
use super::*;

fn entry(path: &str, identity: u64) -> Entry {
    Entry {
        entry_id: format!("entry_{identity:032x}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: format!("file_{identity:032x}"),
        version_id: format!("ver_{identity:032x}"),
        size: identity,
    }
}

// Original owned projection, kept only here as an independent behavior reference.
fn owned_scope(
    entries: &[Entry],
    key: &str,
) -> Vec<(String, String, NamespaceEntryKind, String, String)> {
    let prefix = format!("{key}/");
    entries
        .iter()
        .filter(|entry| entry.lookup_key() == key || entry.lookup_key().starts_with(&prefix))
        .map(|entry| {
            (
                entry.entry_id.clone(),
                entry.file_id.clone(),
                entry.kind,
                entry.path().to_owned(),
                entry.version_id.clone(),
            )
        })
        .collect()
}

fn compare_scope(current: &[Entry], base: &[Entry], key: &str) -> bool {
    let expected = owned_scope(current, key) == owned_scope(base, key);
    assert_eq!(namespace_scope_matches(current, base, key), expected);
    expected
}

#[test]
fn borrowed_scope_matches_exact_owned_fields_order_and_path_boundaries() {
    let baseline = vec![
        entry("Folder/one.txt", 1),
        entry("Folder/two.txt", 2),
        entry("Folder/three.txt", 3),
        entry("Folderish/outside.txt", 4),
    ];
    let key = name_policy::path_key("FOLDER");
    assert!(compare_scope(&baseline, &baseline, &key));
    for index in 0..3 {
        for field in 0..5 {
            let mut changed = baseline.clone();
            match field {
                0 => changed[index].entry_id.push('x'),
                1 => changed[index].file_id.push('x'),
                2 => changed[index].kind = NamespaceEntryKind::Directory,
                3 => {
                    changed[index].path = name_policy::PortablePath::parse(
                        &changed[index].path().to_ascii_uppercase(),
                    )
                    .unwrap()
                }
                4 => changed[index].version_id.push('x'),
                _ => unreachable!(),
            }
            assert!(!compare_scope(&changed, &baseline, &key));
        }
    }
    let mut reordered = baseline.clone();
    reordered.swap(0, 1);
    assert!(!compare_scope(&reordered, &baseline, &key));
    let mut shorter = baseline.clone();
    shorter.remove(2);
    assert!(!compare_scope(&shorter, &baseline, &key));
    let mut longer = baseline.clone();
    longer.push(entry("Folder/four.txt", 5));
    assert!(!compare_scope(&longer, &baseline, &key));
    let mut ignored = baseline.clone();
    ignored[0].size += 1;
    ignored[3].version_id.push('x');
    assert!(compare_scope(&ignored, &baseline, &key));
    assert!(compare_scope(&[], &baseline, "missing"));
    assert!(compare_scope(&[], &baseline, ""));
}

#[test]
fn borrowed_scope_preserves_normalized_membership_and_exact_display_identity() {
    for (path, query, changed_path) in [
        ("Straße/file.txt", "STRASSE", "STRASSE/file.txt"),
        ("Café/file.txt", "Cafe\u{301}", "CAFÉ/file.txt"),
    ] {
        let baseline = vec![entry(path, 1)];
        let key = name_policy::path_key(query);
        assert!(compare_scope(&baseline, &baseline, &key));
        let mut changed = baseline.clone();
        changed[0].path = name_policy::PortablePath::parse(changed_path).unwrap();
        assert_eq!(changed[0].lookup_key(), baseline[0].lookup_key());
        assert!(!compare_scope(&changed, &baseline, &key));
    }
}

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Workspace preparation cost").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    for path in ["Folder/Straße.txt", "Folder/other.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"original")
            .unwrap();
    }
    engine.commit_workspace(request(&workspace)).unwrap();
    (temporary, engine, repository)
}

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "Preparation ownership regression".into(),
        request_digest: None,
    }
}

fn planned_state(engine: &Engine, request: CommitRequest) -> Box<State> {
    match engine
        .plan_workspace_commit_inner(request, None)
        .unwrap()
        .mutation
    {
        MutationPlan::Prepared { next, .. } => next,
        MutationPlan::Existing(_) => panic!("new operation must create a candidate"),
    }
}

fn assert_clean_shared<V>(before: &TrackedMap<V>, after: &TrackedMap<V>) {
    assert!(
        std::ptr::eq::<BTreeMap<String, V>>(&**before, &**after),
        "an unchanged table must retain its original allocation"
    );
    match after.changes_since(before) {
        MapChanges::Keys(keys) => {
            assert!(keys.is_empty(), "unchanged table must have no dirty keys")
        }
        MapChanges::All => panic!("unchanged table lost its exact provenance"),
    }
}

#[test]
fn existing_file_content_plan_keeps_name_and_location_tables_shared() {
    let (temporary, mut engine, repository) = fixture();
    let original = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("FOLDER/STRASSE.txt")
        .unwrap()
        .clone();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "FOLDER/STRASSE.txt", b"changed")
        .unwrap();
    let request = request(&workspace);
    let before = canonical_json(engine.state()).unwrap();
    let candidate = planned_state(&engine, request.clone());
    assert_clean_shared(&engine.state().file_locations, &candidate.file_locations);
    assert_clean_shared(&engine.state().file_names, &candidate.file_names);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    drop(candidate);
    engine.commit_workspace(request).unwrap();
    let current = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("FOLDER/STRASSE.txt")
        .unwrap();
    assert_eq!(current.entry_id, original.entry_id);
    assert_eq!(current.file_id, original.file_id);
    assert_eq!(current.path(), original.path());
    assert_ne!(current.version_id, original.version_id);
    assert_eq!(engine.state().file_names[&current.file_id], original.name());
    assert_eq!(
        engine.state().file_locations[&current.file_id],
        original.path()
    );
    let mut content = engine.open_content(&current.version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"changed");
    drop(content);
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    assert!(engine.check().unwrap().issues.is_empty());
}

#[test]
fn empty_workspace_plan_preserves_inherited_contexts_selections_and_origins() {
    let (_temporary, mut engine, repository) = fixture();
    let path = name_policy::LookupPath::parse("Folder").unwrap();
    let name = BranchName::parse("draft").unwrap();
    engine
        .create_directory_branches(
            &repository.repository_id,
            Some(&path),
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .select_directory_branches(
            &repository.repository_id,
            Some(&path),
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert!(!engine.state().file_contexts.is_empty());
    assert!(
        engine
            .state()
            .file_contexts
            .values()
            .any(|context| context.inherited_from.is_some())
    );
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let candidate = planned_state(&engine, request(&workspace));
    assert_clean_shared(&engine.state().file_contexts, &candidate.file_contexts);
    assert_clean_shared(&engine.state().file_selections, &candidate.file_selections);
    assert_clean_shared(
        &engine.state().file_write_origins,
        &candidate.file_write_origins,
    );
    assert_eq!(candidate.file_contexts, engine.state().file_contexts);
    assert_eq!(candidate.file_selections, engine.state().file_selections);
    assert_eq!(
        candidate.file_write_origins,
        engine.state().file_write_origins
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(candidate.branches, engine.state().branches);
}

#[test]
fn new_file_plan_marks_only_its_new_name_and_location_keys() {
    let (_temporary, mut engine, repository) = fixture();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "Folder/new.txt", b"new")
        .unwrap();
    let candidate = planned_state(&engine, request(&workspace));
    let head = &candidate.branches[&repository.main_branch_id].head_commit_id;
    let added = candidate.commits[head]
        .entries
        .iter()
        .find(|entry| entry.path() == "Folder/new.txt")
        .unwrap();
    assert_eq!(candidate.file_names[&added.file_id], "new.txt");
    assert_eq!(candidate.file_locations[&added.file_id], "Folder/new.txt");
    for (key, value) in engine.state().file_names.iter() {
        assert_eq!(candidate.file_names.get(key), Some(value));
    }
    for (key, value) in engine.state().file_locations.iter() {
        assert_eq!(candidate.file_locations.get(key), Some(value));
    }
    for changes in [
        candidate
            .file_names
            .changes_since(&engine.state().file_names),
        candidate
            .file_locations
            .changes_since(&engine.state().file_locations),
    ] {
        match changes {
            MapChanges::Keys(keys) => {
                assert_eq!(keys.iter().collect::<Vec<_>>(), vec![&added.file_id])
            }
            MapChanges::All => panic!("one new file must keep an exact physical-name write set"),
        }
    }
}

#[test]
fn content_only_contexts_match_the_legacy_replay_after_inherited_first_write() {
    let (_temporary, mut engine, repository) = fixture();
    let path = name_policy::LookupPath::parse("Folder").unwrap();
    let name = BranchName::parse("draft").unwrap();
    engine
        .create_directory_branches(
            &repository.repository_id,
            Some(&path),
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .select_directory_branches(
            &repository.repository_id,
            Some(&path),
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = engine.list_tree(&repository.main_branch_id).unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "FOLDER/STRASSE.txt",
            b"first inherited write",
        )
        .unwrap();
    let request = request(&workspace);
    let candidate = planned_state(&engine, request.clone());
    let head = &candidate.branches[&repository.main_branch_id].head_commit_id;
    let after = candidate.commits[head].entries.clone();
    let canonical = canonical_json(&candidate).unwrap();
    let mut reference = candidate.clone();
    super::super::directory::namespace_locations(
        &mut reference,
        &repository.repository_id,
        &before,
        &after,
        &[],
        "admin",
        request.operation_id,
    )
    .unwrap();
    assert_eq!(canonical_json(&reference).unwrap(), canonical);
    assert_eq!(reference.file_contexts, candidate.file_contexts);
    assert_eq!(reference.file_selections, candidate.file_selections);
    assert_eq!(reference.file_write_origins, candidate.file_write_origins);
}

#[test]
fn empty_namespace_commit_preserves_admission_and_error_precedence() {
    for rejection in ["owner", "fence", "base", "closed", "blocked"] {
        let (_temporary, mut engine, repository) = fixture();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        let mut request = request(&workspace);
        let expected = match rejection {
            "owner" => {
                request.principal = "other".into();
                storage_contract::ErrorCode::PermissionDenied
            }
            "fence" => {
                request.fencing_token += 1;
                storage_contract::ErrorCode::FencingTokenInvalid
            }
            "base" => {
                request.expected_head = "wrong".into();
                storage_contract::ErrorCode::BaseHeadMismatch
            }
            "closed" => {
                engine
                    .abort_workspace(&workspace.workspace_id, "admin")
                    .unwrap();
                request.principal = "other".into();
                request.fencing_token += 1;
                storage_contract::ErrorCode::WorkspaceClosed
            }
            "blocked" => {
                engine.blocked = true;
                request.principal = "other".into();
                storage_contract::ErrorCode::RecoveryRequired
            }
            _ => unreachable!(),
        };
        let before = canonical_json(engine.state()).unwrap();
        let saved = canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap();
        let operation = request.operation_id;
        let error = engine.commit_workspace(request).unwrap_err();
        assert_eq!(error.code(), expected, "{rejection}");
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "{rejection}"
        );
        assert_eq!(
            canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
            saved,
            "{rejection}"
        );
        assert!(
            !engine.state().operations.contains_key(&operation),
            "{rejection}"
        );
    }
}
