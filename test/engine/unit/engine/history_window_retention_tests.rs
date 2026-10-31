//! Real publications keep historical evidence; a window is not an old-root service.
use super::*;
use crate::journal::{Candidate, Publication, PublicationSeal, RecordType, scan};
use std::cmp::Reverse;
use std::io::Read;
use storage_contract::history_window::{
    HistoryCollection, HistoryCursor, HistoryNodeSummary, HistoryPageCursors, HistoryPageIdentity,
    HistoryPageLimit, HistoryWindowPage, HistoryWindowRequest,
};

struct Saved {
    entry: Entry,
    request: CommitRequest,
    result: OperationResult,
    bytes: Vec<u8>,
}

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("state"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let repository = create_repository(&mut engine, "retained");
    (temporary, engine, repository)
}

fn create_repository(engine: &mut Engine, name: &str) -> Repository {
    engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) -> Saved {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "retained history publication".into(),
        request_digest: None,
    };
    let result = engine.commit_workspace(request.clone()).unwrap();
    let entry = engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "file.txt")
        .unwrap();
    Saved {
        entry,
        request,
        result,
        bytes: bytes.to_vec(),
    }
}

fn request(file: &str) -> HistoryWindowRequest {
    HistoryWindowRequest {
        file_id: file.parse().unwrap(),
        collection: HistoryCollection::Nodes,
        location: None,
        history_root: None,
        cursor: None,
        anchor_node_id: None,
        limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes).unwrap(),
    }
}

fn page(
    engine: &Engine,
    request: &HistoryWindowRequest,
) -> (
    HistoryPageIdentity,
    Vec<HistoryNodeSummary>,
    HistoryPageCursors,
) {
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let result = read.page().unwrap();
    engine.recheck_history_window(&read).unwrap();
    let HistoryWindowPage::Nodes {
        identity,
        nodes,
        page,
        ..
    } = result
    else {
        panic!("node page requested");
    };
    assert!(nodes.len() <= request.limit.get());
    (identity, nodes, page)
}

// Snapshot and history graph readers authenticate every dependency. This is a
// retention regression, not a claim that the engine implements metadata GC.
struct RetainedPublication {
    candidate_hash: String,
    publication_hash: String,
    candidate: Candidate,
    publication: Publication,
    seal: PublicationSeal,
    state_bytes: Vec<u8>,
    history_roots: BTreeMap<String, String>,
}

impl RetainedPublication {
    fn capture(engine: &Engine, operation: OperationId) -> Self {
        let members = engine.members();
        let (record, _) = scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap();
        let candidate: Candidate = crate::publication_evidence::verified_object_at(
            &engine.disk_resources,
            &members,
            "candidate",
            &record.candidate_hash,
        )
        .unwrap();
        assert_eq!(candidate.operation_id, operation);
        let publication = crate::publication_evidence::verified_object_at(
            &engine.disk_resources,
            &members,
            "publication",
            &record.object_hash,
        )
        .unwrap();
        let seal = crate::publication_evidence::verified_publication_seal_at(
            &engine.disk_resources,
            &members,
            &candidate,
            &publication,
        )
        .unwrap();
        let admitted = crate::publication_evidence::verified_snapshot_at(
            &engine.disk_resources,
            &members,
            &candidate.snapshot_hash,
        )
        .unwrap();
        let (_, history_roots) = crate::history_package::bound_history_objects_with_roots_at(
            &engine.disk_resources,
            &members,
            &candidate,
            Some(&admitted),
        )
        .unwrap();
        Self {
            candidate_hash: record.candidate_hash,
            publication_hash: record.object_hash,
            candidate,
            publication,
            seal,
            state_bytes: canonical_json(admitted.state()).unwrap(),
            history_roots,
        }
    }

    fn assert_readable(&self, engine: &Engine) {
        let members = engine.members();
        let candidate: Candidate = crate::publication_evidence::verified_object_at(
            &engine.disk_resources,
            &members,
            "candidate",
            &self.candidate_hash,
        )
        .unwrap();
        let publication: Publication = crate::publication_evidence::verified_object_at(
            &engine.disk_resources,
            &members,
            "publication",
            &self.publication_hash,
        )
        .unwrap();
        let seal = crate::publication_evidence::verified_publication_seal_at(
            &engine.disk_resources,
            &members,
            &candidate,
            &publication,
        )
        .unwrap();
        assert_eq!(
            canonical_json(&candidate).unwrap(),
            canonical_json(&self.candidate).unwrap()
        );
        assert_eq!(
            canonical_json(&publication).unwrap(),
            canonical_json(&self.publication).unwrap()
        );
        assert_eq!(
            canonical_json(&seal).unwrap(),
            canonical_json(&self.seal).unwrap()
        );
        let admitted = crate::publication_evidence::verified_snapshot_at(
            &engine.disk_resources,
            &members,
            &candidate.snapshot_hash,
        )
        .unwrap();
        assert_eq!(canonical_json(admitted.state()).unwrap(), self.state_bytes);
        let (_, roots) = crate::history_package::bound_history_objects_with_roots_at(
            &engine.disk_resources,
            &members,
            &candidate,
            Some(&admitted),
        )
        .unwrap();
        assert_eq!(roots, self.history_roots);
    }
}

fn assert_receipt(engine: &Engine, saved: &Saved) {
    let record = engine.operation_record(saved.request.operation_id).unwrap();
    assert_eq!(record.owner(), Some("admin"));
    assert_eq!(
        canonical_json(record.result()).unwrap(),
        canonical_json(&saved.result).unwrap()
    );
}

fn assert_body_bytes(engine: &Engine, version: &str, expected: &[u8]) {
    let mut opened = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
}

#[test]
fn legal_split_windows_use_first_commit_order_and_preserve_original_operation_ids() {
    let (temporary, mut engine, repository) = fixture();
    let first = save(&mut engine, &repository, b"first");
    let retained = RetainedPublication::capture(&engine, first.request.operation_id);
    let original_read = engine
        .capture_history_window(
            request(&first.entry.file_id),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    let second = save(&mut engine, &repository, b"second");
    let third = save(&mut engine, &repository, b"third");
    let saved = [&first, &second, &third];
    let plan = engine
        .create_split_plan(
            &repository.history_tree_id,
            first.result.commit_id.as_deref().unwrap(),
            "admin",
        )
        .unwrap();
    let split_operation = OperationId::generate();
    let split = engine
        .commit_plan(&plan.plan_id, "admin", split_operation)
        .unwrap();
    let mut expected = saved
        .iter()
        .map(|saved| {
            let mapped = split.result["version_mapping"][&saved.entry.version_id]
                .as_str()
                .unwrap();
            assert_eq!(
                engine.state.versions[mapped].version_sequence,
                split.sequence
            );
            assert!(engine.state.versions[mapped].created_as_full_checkpoint);
            assert_ne!(saved.result.sequence, split.sequence);
            (Reverse(saved.result.sequence), mapped.to_owned())
        })
        .collect::<Vec<_>>();
    expected.sort();
    let file = engine.state.versions[&expected[0].1].file_id.clone();
    let mut query = request(&file);
    let mut actual = Vec::new();
    let mut previous: Option<HistoryCursor> = None;
    for index in 0..expected.len() {
        let (identity, nodes, cursors) = page(&engine, &query);
        assert_eq!(
            identity.context.history_tree_id.as_str(),
            split.history_tree_id.as_deref().unwrap()
        );
        assert_eq!(nodes.len(), 1);
        actual.push((Reverse(nodes[0].sequence), nodes[0].node_id.to_string()));
        previous = cursors.previous_cursor;
        query.history_root = Some(identity.history_root);
        query.cursor = cursors.next_cursor;
        assert_eq!(query.cursor.is_some(), index + 1 < expected.len());
    }
    assert_eq!(actual, expected);
    let mut backward = vec![expected.last().unwrap().clone()];
    for _ in 1..expected.len() {
        query.cursor = previous.take();
        assert!(query.cursor.is_some());
        let (_, nodes, cursors) = page(&engine, &query);
        backward.push((Reverse(nodes[0].sequence), nodes[0].node_id.to_string()));
        previous = cursors.previous_cursor;
    }
    assert!(previous.is_none());
    backward.reverse();
    assert_eq!(backward, expected);
    for (Reverse(sequence), node) in &expected {
        query.cursor = None;
        query.anchor_node_id = Some(node.parse().unwrap());
        let (_, nodes, _) = page(&engine, &query);
        assert_eq!(nodes[0].node_id.as_str(), node);
        assert_eq!(nodes[0].sequence, *sequence);
    }
    assert_eq!(
        engine
            .recheck_history_window(&original_read)
            .unwrap_err()
            .code(),
        ErrorCode::TreeMoved
    );
    drop(original_read);
    retained.assert_readable(&engine);
    for saved in saved {
        assert_receipt(&engine, saved);
        let mapped = split.result["version_mapping"][&saved.entry.version_id]
            .as_str()
            .unwrap();
        assert_body_bytes(&engine, mapped, &saved.bytes);
        assert_eq!(
            canonical_json(&engine.commit_workspace(saved.request.clone()).unwrap()).unwrap(),
            canonical_json(&saved.result).unwrap()
        );
    }
    assert_eq!(
        canonical_json(
            &engine
                .commit_plan(&plan.plan_id, "admin", split_operation)
                .unwrap()
        )
        .unwrap(),
        canonical_json(&split).unwrap()
    );
    drop(engine);
    let mut engine = Engine::open(&temporary.path().join("state")).unwrap();
    retained.assert_readable(&engine);
    for saved in [&first, &second, &third] {
        assert_receipt(&engine, saved);
        let mapped = split.result["version_mapping"][&saved.entry.version_id]
            .as_str()
            .unwrap();
        assert!(engine.state.versions[mapped].created_as_full_checkpoint);
        assert_body_bytes(&engine, mapped, &saved.bytes);
        assert_eq!(
            canonical_json(&engine.commit_workspace(saved.request.clone()).unwrap()).unwrap(),
            canonical_json(&saved.result).unwrap()
        );
    }
}

#[test]
fn history_read_pins_the_whole_metadata_generation_only_until_the_read_drops() {
    let (_temporary, mut engine, repository) = fixture();
    let target = save(&mut engine, &repository, b"target");
    let unrelated = create_repository(&mut engine, "unrelated");
    let other = save(&mut engine, &unrelated, b"unrelated");
    let generation = Arc::downgrade(&engine.state);
    let handles = Arc::strong_count(&engine.state);
    let read = engine
        .capture_history_window(
            request(&target.entry.file_id),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(Arc::strong_count(&engine.state), handles + 1);
    let before = read.page().unwrap();
    engine
        .set_file_note(
            &other.entry.file_id,
            None,
            &Note::parse("changed elsewhere".into()).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert!(!generation.ptr_eq(&Arc::downgrade(&engine.state)));
    {
        let retained = generation.upgrade().unwrap();
        assert!(retained.versions.contains_key(&other.entry.version_id));
        assert!(!Arc::ptr_eq(&retained, &engine.state));
    }
    engine.recheck_history_window(&read).unwrap();
    assert_eq!(read.page().unwrap(), before);
    drop(read);
    assert!(generation.upgrade().is_none());
}

#[test]
fn old_metadata_roots_survive_explicit_body_purge_and_cold_restart() {
    let (temporary, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, b"old");
    let retained = RetainedPublication::capture(&engine, old.request.operation_id);
    let latest = save(&mut engine, &repository, b"latest");
    let (identity, _, _) = page(&engine, &request(&old.entry.file_id));
    let plan = engine
        .create_purge_plan(
            &repository.history_tree_id,
            vec![old.entry.version_id.clone()],
            "admin",
        )
        .unwrap();
    let operation = OperationId::generate();
    let published = engine
        .commit_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(
        engine.state.content_states[&old.entry.version_id],
        ContentState::Purged
    );
    retained.assert_readable(&engine);
    assert_receipt(&engine, &old);
    let mut stale = request(&old.entry.file_id);
    stale.history_root = Some(identity.history_root);
    assert_eq!(
        engine
            .capture_history_window(stale, HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    assert_eq!(
        engine
            .commit_plan(&plan.plan_id, "admin", operation)
            .unwrap()
            .result,
        published.result
    );
    drop(engine);
    let engine = Engine::open(&temporary.path().join("state")).unwrap();
    retained.assert_readable(&engine);
    assert_receipt(&engine, &old);
    assert_eq!(
        engine.state.content_states[&latest.entry.version_id],
        ContentState::Available
    );
    assert_body_bytes(&engine, &latest.entry.version_id, &latest.bytes);
}
