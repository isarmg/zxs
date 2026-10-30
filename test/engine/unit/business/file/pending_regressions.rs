use super::*;
use std::io::Read;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([
            root.path().join("p"),
            root.path().join("b"),
            root.path().join("c"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("branch regression").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (root, engine, repo)
}
fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "report.txt", bytes)
        .unwrap();
    commit(engine, workspace)
}
fn commit(engine: &mut Engine, workspace: Workspace) -> OperationResult {
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Regression fixture".into(),
            request_digest: None,
        })
        .unwrap()
}
fn file(engine: &Engine, branch: &str) -> Entry {
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
}

fn pending_alias_fixture() -> (tempfile::TempDir, Engine, Repository, Vec<Entry>) {
    let (root, mut engine, repo) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["a.txt", "b.txt", "c.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"source")
            .unwrap();
    }
    commit(&mut engine, workspace);
    let original = engine.list_tree(&repo.main_branch_id).unwrap();
    let mut aliases = original.clone();
    for entry in &mut aliases {
        entry.relocate(
            name_policy::PortablePath::parse(&format!("alias-{}", entry.name())).unwrap(),
        );
    }
    let mut second_alias = aliases[0].clone();
    second_alias.relocate(name_policy::PortablePath::parse("another-a.txt").unwrap());
    aliases.push(second_alias);
    let operation = OperationId::generate();
    engine
        .mutate("fixture.pending_aliases", "admin", operation, json!({}), |_, state, scratch| {
            let branch = state.branches[&repo.main_branch_id].clone();
            let mut head = state.commits[&branch.head_commit_id].clone();
            head.parent_commit_ids = vec![head.commit_id.clone()];
            head.commit_id = new_id("commit");
            head.tree_id = new_id("tree");
            head.sequence = state.sequence;
            head.operation_id = operation;
            head.namespace = crate::namespace::build_from_entries(&aliases, &mut scratch.namespace)?;
            // A commit-only reservation and a branch-only reservation remain
            // independent. All source identities are real published versions.
            head.pending_file_versions = original[..2].iter()
                .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
                .collect();
            let id = head.commit_id.clone();
            state.commits.insert(id.clone(), head);
            let branch = state.branches.get_mut(&repo.main_branch_id).unwrap();
            branch.head_commit_id = id.clone();
            branch.pending_file_versions = [0, 2].into_iter()
                .map(|index| (original[index].file_id.clone(), original[index].version_id.clone()))
                .collect();
            // Existing physical ownership is deliberately not relocated by
            // this logical alias composition.
            Ok(json!({"commit_id":id,"branch_id":repo.main_branch_id,"history_tree_id":repo.history_tree_id}))
        })
        .unwrap();
    (root, engine, repo, original)
}

#[test]
fn direct_namespace_preserves_pending_aliases_and_removes_only_last_occurrences() {
    let (_root, mut engine, repo, original) = pending_alias_fixture();
    let initial_branch = engine.state().branches[&repo.main_branch_id].clone();
    let initial_head = &engine.state().commits[&initial_branch.head_commit_id];
    let initial_commit_pending = initial_head.pending_file_versions.clone();
    // Measure the shared resolver with this direct API's actual candidate
    // union. Each missing physical hint plus one collective walk reads the
    // one-page alias namespace; there is no per-ID fallback traversal.
    let reads = std::cell::Cell::new(0);
    let live = engine
        .with_namespace_source(|base| {
            let source = crate::namespace::LoaderSource(
                |reference: &storage_contract::NamespaceReference| {
                    reads.set(reads.get() + 1);
                    base.node(reference)
                },
            );
            let candidates = initial_head
                .pending_file_versions
                .keys()
                .chain(initial_branch.pending_file_versions.keys())
                .map(String::as_str)
                .collect();
            super::super::workspace::live_pending_files(
                &initial_head.namespace,
                candidates,
                &engine.state().file_locations,
                &source,
            )
        })
        .unwrap();
    assert_eq!(live.len(), 3);
    assert_eq!(reads.get(), 4);

    let before = canonical_json(engine.state()).unwrap();
    let error = engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Delete, "missing.txt", None).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::EntryNotFound);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);

    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Delete, "alias-a.txt", None).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let branch = &engine.state().branches[&repo.main_branch_id];
    assert_eq!(
        branch.pending_file_versions,
        initial_branch.pending_file_versions
    );
    assert_eq!(
        engine.state().commits[&branch.head_commit_id].pending_file_versions,
        initial_commit_pending
    );

    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Delete, "another-a.txt", None)
                .unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let branch = &engine.state().branches[&repo.main_branch_id];
    assert_eq!(
        branch.pending_file_versions,
        std::collections::BTreeMap::from([(
            original[2].file_id.clone(),
            original[2].version_id.clone(),
        )])
    );
    assert_eq!(
        engine.state().commits[&branch.head_commit_id].pending_file_versions,
        std::collections::BTreeMap::from([(
            original[1].file_id.clone(),
            original[1].version_id.clone(),
        )])
    );
}

#[test]
fn inactive_selected_owner_cannot_receive_main_workspace_writes() {
    for state in [BranchState::Archived, BranchState::Merged] {
        let (_root, mut engine, repo) = fixture();
        save(&mut engine, &repo.main_branch_id, b"original");
        let original = file(&engine, &repo.main_branch_id);
        let branch = engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                &storage_contract::BranchName::parse("selected draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        engine
            .select_file_branch(
                &original.file_id,
                &branch,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        std::sync::Arc::make_mut(&mut engine.state)
            .branches
            .get_mut(&branch)
            .unwrap()
            .state = state;
        let before = engine.state.branches[&branch].head_commit_id.clone();
        assert_eq!(
            engine
                .stage_bytes(&workspace.workspace_id, "report.txt", b"must not publish")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::BranchNotActive
        );
        let current = engine.state.commits[&workspace.base_head].clone();
        let mut changed = current.clone();
        changed.commit_id = new_id("commit");
        let mut built = crate::namespace::NamespaceObjects::new();
        changed.namespace = engine
            .with_namespace_source(|base| {
                let mut entry = crate::namespace::NamespaceView::new(&current.namespace, base)
                    .collect_entries()?
                    .into_iter()
                    .find(|entry| entry.kind == NamespaceEntryKind::File)
                    .unwrap();
                entry.version_id = new_id("ver");
                let mut editor = crate::namespace::NamespaceEditor::open(&current.namespace, base)?;
                editor.upsert_file(&entry)?;
                editor.seal(&mut built)
            })
            .unwrap();
        std::sync::Arc::make_mut(&mut engine.state)
            .commits
            .insert(changed.commit_id.clone(), changed.clone());
        let members = engine.members();
        // Field-disjoint borrows: the reader never touches `state`.
        let reader = crate::segment_store::SegmentReader {
            resources: &engine.disk_resources,
            members: &members,
            caches: &engine.segment_caches,
            observe: None,
        };
        let base =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        let draft = crate::namespace::DraftSource {
            fresh: &built,
            base: &base,
        };
        let mut fresh = crate::namespace::NamespaceObjects::new();
        let error = synchronize_directory_edits(
            std::sync::Arc::make_mut(&mut engine.state),
            &changed.commit_id,
            &["report.txt".into()],
            "admin",
            storage_contract::OperationId::generate(),
            &draft,
            &mut fresh,
        )
        .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::BranchNotActive);
        assert_eq!(engine.state.branches[&branch].head_commit_id, before);
    }
}

#[test]
fn inherited_range_fences_selection_and_preserves_parent_bytes() {
    let (_root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"SOURCE_BYTES");
    let original = file(&engine, &repo.main_branch_id);
    let pending = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("pending").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let pending = pending.result["branch_id"].as_str().unwrap().to_owned();
    let materialized_main = engine.state().file_selections[&original.file_id].clone();
    engine
        .select_file_branch(
            &original.file_id,
            &pending,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let old_workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .select_file_branch(
            &original.file_id,
            &materialized_main,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let error = engine
        .stage_range(
            &old_workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    assert!(
        engine
            .workspace(&old_workspace.workspace_id)
            .unwrap()
            .files
            .is_empty()
    );
    assert_eq!(engine.file_history(&original.file_id).unwrap().len(), 1);

    engine
        .select_file_branch(
            &original.file_id,
            &pending,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap();
    let mut content = engine
        .open_workspace_content(&workspace.workspace_id, &staged.item_id, "admin")
        .unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        bytes, b"XOURCE_BYTES",
        "an inherited range edits the visible parent without changing that immutable version"
    );
    drop(content);
    commit(&mut engine, workspace);
    let written = file(&engine, &repo.main_branch_id);
    assert_eq!(
        engine.state().versions[&written.version_id].source_version_ids,
        vec![original.version_id]
    );
    assert_eq!(written.size, 12);
}

#[test]
fn selected_file_materialization_replaces_a_split_main_placeholder() {
    let (root, mut engine, repo) = fixture();
    let original = save(&mut engine, &repo.main_branch_id, b"original");
    let pending = engine
        .create_branch(
            original.commit_id.as_ref().unwrap(),
            &storage_contract::BranchName::parse("pending full").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &pending.head_commit_id, "admin")
        .unwrap();
    let split = engine
        .commit_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let repository = engine
        .state()
        .repositories
        .values()
        .find(|repo| Some(&repo.history_tree_id) == split.history_tree_id.as_ref())
        .unwrap()
        .clone();
    let placeholder = file(&engine, &repository.main_branch_id);
    assert!(!placeholder.version_id.is_empty());
    assert_eq!(placeholder.size, 8);
    let source = engine
        .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
        .unwrap()
        .to_owned();
    let created = engine
        .create_file_branch(
            &placeholder.file_id,
            &source,
            &storage_contract::BranchName::parse("actual").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = created.result["branch_id"].as_str().unwrap().to_owned();
    engine
        .select_file_branch(
            &placeholder.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save(&mut engine, &branch, b"written branch bytes");
    let written = file(&engine, &branch);
    let selected = file(&engine, &repository.main_branch_id);
    assert_eq!(selected.version_id, written.version_id);
    assert_eq!(selected.size, 20);
    assert!(
        engine
            .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
            .is_none()
    );
    assert!(
        !engine.state().branches[&repository.main_branch_id]
            .pending_file_versions
            .contains_key(&placeholder.file_id)
    );
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        file(&engine, &repository.main_branch_id).version_id,
        written.version_id
    );
    assert!(
        engine
            .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
            .is_none()
    );
}

#[test]
fn historical_range_edits_keep_the_explicit_old_snapshot_as_their_base() {
    let (_root, mut engine, repo) = fixture();
    let original = save(&mut engine, &repo.main_branch_id, b"old body");
    let source = original.commit_id.unwrap();
    save(&mut engine, &repo.main_branch_id, b"current body");
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source,
                name: "edited old snapshot".into(),
                workspace_id: Some(workspace.workspace_id),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let saved = engine
        .commit_historical_save_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let written = file(&engine, saved.branch_id.as_ref().unwrap());
    let mut content = engine.open_content(&written.version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"Xld body");
    let unchanged = file(&engine, &repo.main_branch_id);
    assert_ne!(written.version_id, unchanged.version_id);
    assert_eq!(unchanged.size, 12);
}
