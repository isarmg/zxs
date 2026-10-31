use super::*;
use crate::journal::{Candidate, RecordType, load_object, scan};
use storage_contract::history_window::{
    HistoryCollection, HistoryFileId, HistoryLocation, HistoryPageLimit, HistoryRepositoryId,
    HistoryWindowPage, HistoryWindowRequest,
};

fn fixture() -> (tempfile::TempDir, Engine, Repository, Entry) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let repository = repository(&mut engine, "target");
    let file = save(&mut engine, &repository, b"target body");
    (temp, engine, repository, file)
}

fn repository(engine: &mut Engine, name: &str) -> Repository {
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse(name).unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "history root fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "file.txt")
        .unwrap()
}

// Read the actual published Candidate and authenticated catalog independently
// of the query binding, rather than re-encoding State or consulting SQLite.
fn published_catalog(engine: &Engine) -> BTreeMap<String, String> {
    let members = engine.members();
    let published = scan(&members[0])
        .unwrap()
        .records
        .into_iter()
        .rev()
        .find(|(record, _)| {
            record.record_type == RecordType::Publication
                && record.sequence == engine.state().sequence
        })
        .unwrap()
        .0;
    let candidate: Candidate =
        load_object(&members[0], "candidate", &published.candidate_hash).unwrap();
    assert_eq!(candidate.sequence, engine.state().sequence);
    crate::history_package::resolve_catalog(&candidate.history_roots, |kind, hash| {
        crate::publication_evidence::verified_graph_object_at(
            &engine.disk_resources,
            &members,
            kind,
            hash,
        )
    })
    .unwrap()
    .0
}

fn observed_root(engine: &Engine, history: &str) -> String {
    engine.metadata_view().history_root(history).unwrap().into()
}

fn node_request(file: &str) -> HistoryWindowRequest {
    HistoryWindowRequest {
        file_id: HistoryFileId::parse(file).unwrap(),
        collection: HistoryCollection::Nodes,
        location: None,
        history_root: None,
        cursor: None,
        anchor_node_id: None,
        limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes).unwrap(),
    }
}

fn assert_catalog_matches(engine: &Engine) {
    let view = engine.metadata_view();
    let catalog = published_catalog(engine);
    assert_eq!(
        catalog.keys().collect::<Vec<_>>(),
        engine.state.histories.keys().collect::<Vec<_>>()
    );
    for (history, root) in catalog {
        assert_eq!(view.history_root(&history).unwrap(), root);
    }
}

#[test]
fn published_history_root_is_exact_across_full_and_checkpoint_cold_recovery() {
    let (temp, engine, repository, _file) = fixture();
    let root = observed_root(&engine, &repository.history_tree_id);
    assert_catalog_matches(&engine);
    drop(engine);

    for _ in 0..2 {
        let engine = Engine::open(&temp.path().join("state")).unwrap();
        assert!(engine.published_state_base.is_none());
        assert_eq!(observed_root(&engine, &repository.history_tree_id), root);
        assert_catalog_matches(&engine);
        for member in engine.members() {
            assert!(
                member
                    .root
                    .join("metadata/replay-checkpoint.json")
                    .is_file()
            );
        }
    }
}

#[test]
fn unrelated_history_publications_do_not_change_the_target_history_root() {
    let (temp, mut engine, target, _file) = fixture();
    let root = observed_root(&engine, &target.history_tree_id);
    let sequence = engine.state().sequence;
    let unrelated = repository(&mut engine, "unrelated");
    let unrelated_file = save(&mut engine, &unrelated, b"unrelated body");
    engine
        .set_file_note(
            &unrelated_file.file_id,
            None,
            &storage_contract::Note::parse("unrelated metadata".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert!(engine.state().sequence > sequence);
    assert_eq!(observed_root(&engine, &target.history_tree_id), root);
    assert_ne!(observed_root(&engine, &unrelated.history_tree_id), root);
    assert_catalog_matches(&engine);
    drop(engine);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(observed_root(&engine, &target.history_tree_id), root);
    assert_catalog_matches(&engine);
}

#[test]
fn file_and_version_notes_change_the_root_without_new_content_or_history_generation() {
    let (_temp, mut engine, repository, file) = fixture();
    let history = &repository.history_tree_id;
    let generation = engine.state.histories[history].generation;
    let bodies = engine.state.versions.len();
    let old_view = engine.metadata_view();
    let original = old_view.history_root(history).unwrap().to_owned();
    let mut previous = original.clone();

    for version in [None, Some(file.version_id.as_str())] {
        engine
            .set_file_note(
                &file.file_id,
                version,
                &storage_contract::Note::parse("note only".into()).unwrap(),
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        let current = observed_root(&engine, history);
        assert_ne!(current, previous);
        assert_catalog_matches(&engine);
        assert_eq!(engine.state.versions.len(), bodies);
        assert_eq!(engine.state.histories[history].generation, generation);
        previous = current;
    }
    assert_eq!(old_view.history_root(history).unwrap(), original);
}

#[test]
fn tags_and_pending_file_branch_selection_each_change_the_exact_root() {
    let (_temp, mut engine, repository, file) = fixture();
    let history = &repository.history_tree_id;
    let branch = engine
        .create_file_branch(
            &file.file_id,
            &file.version_id,
            &storage_contract::BranchName::parse("pending alternate").unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let context = engine
        .file_context(&repository.repository_id, "file.txt")
        .unwrap();
    let target = FileTagTarget {
        branch_id: context.branch_id.clone(),
        source_node_id: file.version_id.clone(),
        expected_node_id: engine.file_branch_head(&context.branch_id).unwrap(),
    };
    let original = observed_root(&engine, history);
    let bodies = engine.state.versions.len();
    let generation = engine.state.histories[history].generation;
    engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &file.file_id,
            targets: &[target],
            add: &["review".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    assert_ne!(observed_root(&engine, history), original);
    assert_catalog_matches(&engine);

    let before_selection = observed_root(&engine, history);
    engine
        .select_file_branch(&file.file_id, &branch, "admin", OperationId::generate())
        .unwrap();
    assert_ne!(observed_root(&engine, history), before_selection);
    assert_eq!(engine.state.versions.len(), bodies);
    assert_eq!(engine.state.histories[history].generation, generation);
    assert_catalog_matches(&engine);
}

#[test]
fn logical_history_roots_do_not_depend_on_write_reuse_or_integrity_epoch() {
    let (_temp, mut engine, repository, _file) = fixture();
    let root = observed_root(&engine, &repository.history_tree_id);
    engine.metadata_integrity.invalidate();
    engine.published_state_base = None;
    assert_eq!(observed_root(&engine, &repository.history_tree_id), root);
}

#[test]
fn a_durable_local_plan_preserves_exact_roots_across_state_detachment_and_restart() {
    let (temp, mut engine, repository, file) = fixture();
    let history = &repository.history_tree_id;
    let old_view = engine.metadata_view();
    let retained = engine
        .capture_history_window(
            node_request(&file.file_id),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    let page = retained.page().unwrap();
    let root = old_view.history_root(history).unwrap().to_owned();
    let sequence = engine.state().sequence;
    let source = engine.state.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    let plan = engine
        .create_historical_save_plan(
            history,
            HistoricalSaveOptions {
                source_commit_id: source,
                name: "prepared history branch".into(),
                workspace_id: None,
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let new_view = engine.metadata_view();
    assert!(!std::ptr::eq(
        old_view.business_projection(),
        new_view.business_projection()
    ));
    assert!(
        !old_view
            .business_projection()
            .plans
            .contains_key(&plan.plan_id)
    );
    assert!(new_view.business_projection().plans.is_empty());
    assert_eq!(engine.state().plans[&plan.plan_id], plan);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(old_view.history_root(history).unwrap(), root);
    assert_eq!(new_view.history_root(history).unwrap(), root);
    engine.recheck_history_window(&retained).unwrap();
    assert_eq!(retained.page().unwrap(), page);
    assert_catalog_matches(&engine);
    drop(old_view);
    drop(new_view);
    drop(retained);
    drop(engine);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(observed_root(&engine, history), root);
    assert_eq!(engine.plan(&plan.plan_id).unwrap(), plan);
    assert_catalog_matches(&engine);
}

#[test]
fn captured_node_page_binds_the_published_root_and_effective_action_budget_after_restart() {
    let (temp, engine, repository, file) = fixture();
    let request = node_request(&file.file_id);
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    let HistoryWindowPage::Nodes {
        identity, nodes, ..
    } = &page
    else {
        panic!("node request must produce a node page");
    };
    assert_eq!(
        identity.history_root.to_string(),
        published_catalog(&engine)[&repository.history_tree_id]
    );
    assert_eq!(identity.file_id.as_str(), file.file_id);
    assert_eq!(identity.collection, HistoryCollection::Nodes);
    assert_eq!(identity.limit.get(), 1);
    assert_eq!(
        identity.action_max_entries,
        u64::try_from(engine.config.max_entries).unwrap()
    );
    assert_eq!(read.max_entries(), engine.config.max_entries);
    assert_eq!(&identity.context, read.context());
    assert_eq!(
        identity.query_hash,
        request.query_hash(read.context(), None)
    );
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].node_id.as_str(), file.version_id);
    assert!(nodes[0].current);
    engine.recheck_history_window(&read).unwrap();
    drop(read);
    drop(engine);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(read.page().unwrap(), page);
    engine.recheck_history_window(&read).unwrap();
}

#[test]
fn recheck_accepts_unrelated_history_changes_and_rejects_a_changed_target_note() {
    let (_temp, mut engine, target, file) = fixture();
    let request = node_request(&file.file_id);
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    let HistoryWindowPage::Nodes { identity, .. } = &page else {
        panic!("node request must produce a node page");
    };
    let unrelated = repository(&mut engine, "unrelated capture");
    let unrelated_file = save(&mut engine, &unrelated, b"unrelated content");
    engine
        .set_file_note(
            &unrelated_file.file_id,
            None,
            &storage_contract::Note::parse("unrelated note".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    engine.recheck_history_window(&read).unwrap();
    assert_eq!(read.page().unwrap(), page);
    assert_eq!(
        identity.history_root.to_string(),
        published_catalog(&engine)[&target.history_tree_id]
    );

    engine
        .set_file_note(
            &file.file_id,
            None,
            &storage_contract::Note::parse("target note changed".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.recheck_history_window(&read).unwrap_err().code(),
        ErrorCode::HistoryChanged
    );
    let mut stale = request;
    stale.history_root = Some(identity.history_root);
    assert_eq!(
        engine
            .capture_history_window(stale, HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
}

#[test]
fn explicit_history_location_resolves_its_exact_path_and_rejects_other_files_and_repositories() {
    let (_temp, mut engine, repository, file) = fixture();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "other.txt", b"another file")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "second file for path scope".into(),
            request_digest: None,
        })
        .unwrap();
    let other_file = engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "other.txt")
        .unwrap();
    let other_repository = self::repository(&mut engine, "other path scope");
    let mut request = node_request(&file.file_id);
    request.location = Some(HistoryLocation::new(
        HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
        name_policy::LookupPath::parse("FILE.TXT").unwrap(),
    ));
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(
        read.context().repository_id.as_str(),
        repository.repository_id
    );
    assert_eq!(
        read.context().history_tree_id.as_str(),
        repository.history_tree_id
    );
    assert_eq!(
        read.context()
            .path
            .as_ref()
            .map(name_policy::PortablePath::as_str),
        Some("file.txt")
    );
    assert_eq!(
        read.context()
            .current_node_id
            .as_ref()
            .map(|node| node.as_str()),
        Some(file.version_id.as_str())
    );
    let current = engine
        .file_context(&repository.repository_id, "file.txt")
        .unwrap();
    assert_eq!(
        read.context()
            .selected_branch_id
            .as_ref()
            .map(|branch| branch.as_str()),
        Some(current.branch_id.as_str())
    );
    engine.recheck_history_window(&read).unwrap();

    let mut wrong_file = request.clone();
    wrong_file.file_id = HistoryFileId::parse(&other_file.file_id).unwrap();
    let mut wrong_path = request.clone();
    wrong_path.location.as_mut().unwrap().path =
        name_policy::LookupPath::parse("missing.txt").unwrap();
    let mut wrong_repository = request;
    wrong_repository.location.as_mut().unwrap().repository_id =
        HistoryRepositoryId::parse(&other_repository.repository_id).unwrap();
    for invalid in [wrong_file, wrong_path, wrong_repository] {
        assert_eq!(
            engine
                .capture_history_window(invalid, HistoryWindowCancellation::new())
                .unwrap_err()
                .code(),
            ErrorCode::FileNotFound
        );
    }
}

#[test]
fn a_history_cursor_binds_requested_context_presence_and_normalized_path_identity() {
    let (_temp, mut engine, repository, file) = fixture();
    let newest = save(&mut engine, &repository, b"new target body");
    assert_eq!(newest.file_id, file.file_id);
    let mut request = node_request(&file.file_id);
    request.location = Some(HistoryLocation::new(
        HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
        name_policy::LookupPath::parse("FILE.TXT").unwrap(),
    ));
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let HistoryWindowPage::Nodes { nodes, page, .. } = read.page().unwrap() else {
        panic!("node request must produce a node page");
    };
    assert_eq!(nodes[0].node_id.as_str(), newest.version_id);
    let mut continuation = request;
    continuation.cursor = Some(page.next_cursor.unwrap());
    continuation.location.as_mut().unwrap().path =
        name_policy::LookupPath::parse("file.txt").unwrap();
    let next = engine
        .capture_history_window(continuation.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let HistoryWindowPage::Nodes { nodes, .. } = next.page().unwrap() else {
        panic!("node continuation must produce a node page");
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].node_id.as_str(), file.version_id);
    engine.recheck_history_window(&next).unwrap();

    continuation.location = None;
    assert_eq!(
        engine
            .capture_history_window(continuation, HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn a_pre_cancelled_history_capture_is_rejected_before_read_admission() {
    let (_temp, engine, _repository, file) = fixture();
    let cancellation = HistoryWindowCancellation::new();
    cancellation.clone().cancel();
    assert_eq!(
        engine
            .capture_history_window(node_request(&file.file_id), cancellation.clone())
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
}

#[test]
fn cancelling_a_retained_history_read_rejects_page_preparation_and_recheck() {
    let (_temp, engine, _repository, file) = fixture();
    let cancellation = HistoryWindowCancellation::new();
    let read = engine
        .capture_history_window(node_request(&file.file_id), cancellation.clone())
        .unwrap();
    read.page().unwrap();
    engine.recheck_history_window(&read).unwrap();
    read.cancellation().cancel();
    assert!(cancellation.is_cancelled());
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::OperationAborted);
    assert_eq!(
        engine.recheck_history_window(&read).unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        engine
            .capture_history_window(node_request(&file.file_id), cancellation)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
}

#[test]
fn missing_history_root_binding_fails_instead_of_manufacturing_a_root() {
    let (_temp, mut engine, repository, _file) = fixture();
    engine.history_window_roots = None;
    let view = engine.metadata_view();
    let error = view.history_root(&repository.history_tree_id).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
}

#[test]
fn history_root_binding_requires_the_same_immutable_state_allocation() {
    let (_temp, mut engine, repository, _file) = fixture();
    let old_view = engine.metadata_view();
    let root = old_view
        .history_root(&repository.history_tree_id)
        .unwrap()
        .to_owned();
    let before = canonical_json(engine.state()).unwrap();
    let _ = Arc::make_mut(&mut engine.state);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let view = engine.metadata_view();
    assert_eq!(
        view.history_root(&repository.history_tree_id)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        old_view.history_root(&repository.history_tree_id).unwrap(),
        root
    );
}

#[test]
fn absent_history_is_not_reported_as_an_empty_or_default_root() {
    let (_temp, engine, _repository, _file) = fixture();
    let view = engine.metadata_view();
    let error = view.history_root(&new_id("history")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::HistoryNotFound);
}

#[cfg(feature = "fault-injection")]
#[test]
fn indeterminate_publication_keeps_the_old_root_until_verified_cold_recovery() {
    let (temp, mut engine, repository, file) = fixture();
    let old = observed_root(&engine, &repository.history_tree_id);
    let before = engine.state().sequence;
    engine.inject_fault_once("after_accept_1");
    let error = engine
        .set_file_note(
            &file.file_id,
            None,
            &storage_contract::Note::parse("accepted pending note".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eq!(engine.state().sequence, before);
    assert_eq!(observed_root(&engine, &repository.history_tree_id), old);
    drop(engine);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert!(engine.state().sequence > before);
    assert_ne!(observed_root(&engine, &repository.history_tree_id), old);
    assert_catalog_matches(&engine);
}

#[test]
fn business_view_outlives_publication_without_retaining_receipts_or_scrub_baseline() {
    let (_temp, mut engine, repository, file) = fixture();
    let baseline = Arc::downgrade(&engine.state);
    assert!(!engine.state().operations.is_empty());
    assert!(!engine.state().operation_principals.is_empty());
    assert!(!engine.state().events.is_empty());
    let view = engine.metadata_view();
    let captured = view.business_projection();
    assert!(captured.operations.is_empty());
    assert!(captured.operation_principals.is_empty());
    assert!(captured.events.is_empty());
    assert!(captured.plans.is_empty());
    assert!(captured.control_auth.is_none());
    assert_eq!(baseline.strong_count(), 1);
    let root = view
        .history_root(&repository.history_tree_id)
        .unwrap()
        .to_owned();
    let scrub = engine.prepare_integrity_scrub().unwrap();
    assert_eq!(
        baseline.strong_count(),
        2,
        "scrub deliberately retains its complete baseline"
    );
    save(&mut engine, &repository, b"new publication");
    assert_eq!(
        baseline.strong_count(),
        1,
        "only the in-flight scrub pins the old State"
    );
    assert_eq!(
        view.history_root(&repository.history_tree_id).unwrap(),
        root
    );
    assert!(
        view.business_projection()
            .versions
            .contains_key(&file.version_id)
    );
    drop(scrub);
    assert!(
        baseline.upgrade().is_none(),
        "business view does not pin the scrub baseline"
    );
    drop(engine);
    assert_eq!(
        view.history_root(&repository.history_tree_id).unwrap(),
        root
    );
    assert!(
        view.business_projection()
            .versions
            .contains_key(&file.version_id)
    );
}
