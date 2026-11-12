//! Non-publishing production projection. No fixture installs a projected State.
use crate as engine_api;
#[path = "../../support/container_tags.rs"]
mod container_tags_fixture;
use super::*;

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    source: Repository,
    target: Repository,
    file: Entry,
}

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, body: &[u8]) {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "source.txt", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "move projection fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development([temp.path().join("source"), temp.path().join("target")]).unwrap();
        config.disks[1].level = 1;
        config.disks[1].upstream_slot_id = None;
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let source = engine
            .create_disk_root("P01", "admin", operation())
            .unwrap();
        let target = engine
            .create_disk_root("B01", "admin", operation())
            .unwrap();
        save(&mut engine, &source, b"old body");
        save(&mut engine, &source, b"current body");
        let file = engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .unwrap()
            .clone();
        Self {
            temp,
            engine,
            source,
            target,
            file,
        }
    }

    fn capture(&self) -> FileHistoryMoveCapture {
        self.engine
            .capture_file_history_move(
                FileHistoryMoveRequest {
                    namespace: &FileHistoryCopyRequest {
                        source_repository_id: self.source.repository_id.clone(),
                        source_path: "source.txt".into(),
                        expected_source_head: self.engine.state().branches
                            [&self.source.main_branch_id]
                            .head_commit_id
                            .clone(),
                        target_repository_id: self.target.repository_id.clone(),
                        target_name: "moved.txt".into(),
                        expected_target_head: self.engine.state().branches
                            [&self.target.main_branch_id]
                            .head_commit_id
                            .clone(),
                    },
                    source_file_id: &HistoryFileId::parse(&self.file.file_id).unwrap(),
                },
                &HistoryWindowCancellation::new(),
            )
            .unwrap()
    }

    fn project(&self) -> FileHistoryMoveProjection {
        self.engine
            .project_file_history_move(
                self.capture(),
                "admin",
                operation(),
                &HistoryWindowCancellation::new(),
            )
            .unwrap()
    }
}

fn recheck_error(engine: &Engine, projection: &FileHistoryMoveProjection) -> ErrorCode {
    engine
        .recheck_file_history_move_projection(projection, &HistoryWindowCancellation::new())
        .map(|_| ())
        .unwrap_err()
        .code()
}

#[test]
fn move_projects_one_complete_joint_graph_without_publishing_either_side() {
    let mut fixture = Fixture::new();
    let branch = fixture
        .engine
        .create_file_branch(
            &fixture.file.file_id,
            &fixture.file.version_id,
            &BranchName::parse("retained-branch").unwrap(),
            "admin",
            operation(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    fixture
        .engine
        .archive_branch(&branch, "admin", operation())
        .unwrap();
    let before = fixture.engine.state().clone();
    let projection = fixture.project();
    let deletion = projection.proposed_source_deletion();
    assert_eq!(deletion.branch_heads, projection.capture().branch_heads);
    assert!(deletion.branch_heads.contains_key(&branch));
    assert_eq!(
        deletion.parent_node_ids,
        projection.capture().parent_node_ids
    );
    assert_eq!(deletion.operation_id, projection.operation_id());
    assert_eq!(deletion.author, projection.principal());
    assert_eq!(deletion.sequence, projection.proposed_sequence());
    assert_eq!(deletion.created_at, projection.route.created_at);
    let target_file = projection.target_file_id().as_str();
    assert_ne!(target_file, fixture.file.file_id);
    assert_eq!(projection.version_mapping().len(), 2);
    assert_eq!(projection.branch_mapping().len(), 2);
    assert_eq!(projection.target_binding_generations().len(), 1);
    let next = &projection.writes.next;
    assert!(!next.file_deletions.contains_key(target_file));
    assert_eq!(next.operations, before.operations);
    assert_eq!(next.operation_principals, before.operation_principals);
    assert_eq!(next.events, before.events);
    for (old, new) in projection.version_mapping() {
        assert_eq!(next.versions[old], before.versions[old]);
        assert_eq!(next.versions[new].file_id, target_file);
        assert_eq!(
            next.versions[new].content_hash,
            before.versions[old].content_hash
        );
        assert_eq!(
            next.versions[new].logical_size,
            before.versions[old].logical_size
        );
        assert_eq!(next.placements[new].route_slot_ids, ["B01"]);
    }
    for old in projection.branch_mapping().keys() {
        assert_eq!(next.branches[old], before.branches[old]);
    }
    let arrival = &next.commits[&next.branches[&fixture.target.main_branch_id].head_commit_id];
    assert_eq!(arrival.kind, "file_history_move");
    assert_eq!(arrival.operation_id, projection.operation_id());
    assert_eq!(arrival.sequence, deletion.sequence);
    assert_eq!(arrival.entries[0].file_id, target_file);
    assert_eq!(arrival.entries[0].path(), "moved.txt");
    fixture
        .engine
        .recheck_file_history_move_projection(&projection, &HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(fixture.engine.state(), &before);
    assert!(
        fixture
            .engine
            .file_deletion(&fixture.file.file_id)
            .is_none()
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[test]
fn move_reconstruction_keeps_the_exact_frozen_clock_slot_route_and_identities() {
    let fixture = Fixture::new();
    let projection = fixture.project();
    let replay = compose(
        fixture.engine.state(),
        fixture.engine.admitted_current_namespaces().unwrap(),
        &ProjectionInput {
            source: ProjectionSource::from(&projection.capture),
            route: &projection.route,
            principal: &projection.principal,
            operation: projection.operation,
            sequence: projection.proposed_sequence(),
        },
    )
    .unwrap();
    assert_eq!(replay.next, projection.writes.next);
    assert_eq!(replay.version_mapping, projection.writes.version_mapping);
    assert_eq!(replay.branch_mapping, projection.writes.branch_mapping);
    assert_eq!(
        json_codec::to_value(&replay.lifecycle).unwrap(),
        json_codec::to_value(&projection.writes.lifecycle).unwrap()
    );
}

#[test]
fn move_projection_preserves_tags_notes_and_undone_lifecycle_without_importing_new_d() {
    let mut fixture = Fixture::new();
    let context = fixture
        .engine
        .file_context(&fixture.source.repository_id, "source.txt")
        .unwrap();
    let head = fixture.engine.file_branch_head(&context.branch_id).unwrap();
    container_tags_fixture::edit(&mut fixture.engine, &fixture.file.file_id, &["kept-tag".into()], &[], "admin", operation())
        .unwrap();
    fixture
        .engine
        .set_file_note(
            &fixture.file.file_id,
            None,
            &Note::parse("retained note".into()).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    let file = HistoryFileId::parse(&fixture.file.file_id).unwrap();
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&fixture.source.repository_id).unwrap(),
                file_id: &file,
                path: &name_policy::LookupPath::parse("source.txt").unwrap(),
            },
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let deleted = fixture.engine.file_deletion(file.as_str()).unwrap().clone();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &deleted.node_id,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let projection = fixture.project();
    let target = projection.target_file_id().as_str();
    let next = &projection.writes.next;
    assert_eq!(next.file_notes[target], "retained note");
    assert!(
        next.container_tags
            .get(&target.parse::<storage_contract::FileId>().unwrap())
            .unwrap()
            .tags().iter().any(|tag| tag.as_str() == "kept-tag")
    );
    assert_eq!(projection.retained_lifecycle_events(), 2);
    let events = &projection.writes.lifecycle.events;
    assert!(events.iter().all(|event| {
        event.node.file_id == target
            && event.node.node_id != deleted.node_id
            && event.operation_id != projection.operation_id()
            && event.node.node_id != projection.proposed_source_deletion().node_id
    }));
    assert_eq!(
        projection.writes.lifecycle.source_records_hash,
        projection.capture().source_records_digest().to_string()
    );
    assert!(!next.file_deletions.contains_key(target));
}

#[test]
fn changed_state_or_unpublished_workspace_invalidates_the_joint_projection() {
    let mut fixture = Fixture::new();
    let projection = fixture.project();
    fixture
        .engine
        .set_file_note(
            &fixture.file.file_id,
            None,
            &Note::parse("new source note".into()).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(
        recheck_error(&fixture.engine, &projection),
        ErrorCode::WorkspaceChanged
    );
    let projection = fixture.project();
    let sequence = fixture.engine.state().sequence;
    let _workspace = fixture
        .engine
        .create_workspace(&fixture.target.main_branch_id, "admin")
        .unwrap();
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert_eq!(
        recheck_error(&fixture.engine, &projection),
        ErrorCode::WorkspaceBusy
    );
}

#[test]
fn same_state_but_changed_decision_slot_cannot_reuse_a_joint_projection() {
    let mut fixture = Fixture::new();
    let projection = fixture.project();
    // Admission-only fixture: a resolved aborted slot can advance without
    // turning the proposed write set into published State.
    fixture.engine.next_slot += 1;
    assert_eq!(
        recheck_error(&fixture.engine, &projection),
        ErrorCode::WorkspaceChanged
    );
    assert!(
        fixture
            .engine
            .file_deletion(&fixture.file.file_id)
            .is_none()
    );
}

#[test]
fn joint_projection_has_no_side_effects_to_recover_after_cold_open() {
    let fixture = Fixture::new();
    let projection = fixture.project();
    let root = fixture.temp.path().join("system");
    let before = fixture.engine.state().clone();
    let target = fixture.target.clone();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let reopened = Engine::open(&root).unwrap();
    assert_eq!(reopened.state(), &before);
    assert_eq!(
        recheck_error(&reopened, &projection),
        ErrorCode::WorkspaceChanged
    );
    assert!(
        !reopened
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
    assert!(
        reopened
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
    drop(reopened);
    drop(temp);
}

#[test]
fn projection_rejects_used_operation_empty_actor_and_cancellation() {
    let fixture = Fixture::new();
    let used = *fixture.engine.state().operations.keys().next().unwrap();
    let cancellation = HistoryWindowCancellation::new();
    assert_eq!(
        fixture
            .engine
            .project_file_history_move(fixture.capture(), "admin", used, &cancellation,)
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fixture
            .engine
            .project_file_history_move(fixture.capture(), "", operation(), &cancellation,)
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::Unauthenticated
    );
    let capture = fixture.capture();
    cancellation.cancel();
    assert_eq!(
        fixture
            .engine
            .project_file_history_move(capture, "admin", operation(), &cancellation,)
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
}

#[test]
fn joint_projection_is_not_admitted_as_an_existing_copy_or_delete_publication() {
    let fixture = Fixture::new();
    let projection = fixture.project();
    assert_eq!(
        crate::file_deletion::publication::verify(
            fixture.engine.state(),
            &projection.writes.next,
            fixture.engine.admitted_current_namespaces().unwrap(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::PublicationInvalid
    );
    let mut changed_source = projection.writes.next.clone();
    let source_branch = projection.capture().branch_heads().keys().next().unwrap();
    changed_source
        .branches
        .get_mut(source_branch)
        .unwrap()
        .generation += 1;
    assert_eq!(
        crate::file_deletion::freeze::verify(&projection.writes.next, &changed_source)
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    let mut changed_target = projection.writes.next.clone();
    let target_branch = projection.branch_mapping().values().next().unwrap();
    changed_target
        .branches
        .get_mut(target_branch)
        .unwrap()
        .generation += 1;
    crate::file_deletion::freeze::verify(&projection.writes.next, &changed_target).unwrap();
}
