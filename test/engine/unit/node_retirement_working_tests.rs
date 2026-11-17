//! Actual full/range draft producers, then exact publication/cold counterexamples.
use crate::node_retirement_engine::{self as retirement, Request};
use crate::{CommitRequest, Config, Engine, ErrorCode, ReadFile, State};
use name_policy::{Name, PortablePath};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use storage_contract::file_draft::{CommitMode, DraftBase, DraftCommand, DraftLocation};
use storage_contract::history_window::{HistoryVersionId, HistoryWindowCancellation};
use storage_contract::shared_history::SharedHistoryRequest;
use storage_contract::{ContentHash, Mode, OperationId, Principal};

struct Fixture {
    temporary: tempfile::TempDir,
    engine: Engine,
    target: DraftLocation,
    old: String,
    current: String,
    history: String,
}

fn fixture(incremental: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("disk");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development, [(Name::parse("Primary").unwrap(), disk)],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let mut versions = Vec::new();
    for body in [b"earlier body".as_slice(), b"current body".as_slice()] {
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "a", body).unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "retained draft base".into(),
            request_digest: None,
        }).unwrap();
        versions.push(engine.namespace_tree(&repository.main_branch_id).unwrap()
            .get("a").unwrap().version_id.clone());
    }
    let entry = engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("a").unwrap().clone();
    let target = DraftLocation {
        repository: repository.repository_id.parse().unwrap(),
        path: PortablePath::parse("a").unwrap(),
        entry: entry.entry_id.parse().unwrap(),
        file: entry.file_id.parse().unwrap(),
    };
    let principal: Principal = "admin".parse().unwrap();
    engine.change_file_working(&DraftCommand::SetMode {
        target: target.clone(),
        expected: engine.file_working_state(&target).unwrap().revision(),
        mode: CommitMode::Manual,
    }, &principal, OperationId::generate().unwrap()).unwrap();
    if incremental {
        let mut file = engine.open_native_working(&target, &principal, true).unwrap();
        assert_eq!(engine.write_native_working(&mut file, &principal, 2, b"draft").unwrap(), 5);
        engine.flush_native_working(&mut file, &principal).unwrap().unwrap();
        assert!(engine.file_working_state(&target).unwrap().incremental().is_some());
    } else {
        let bytes = b"complete saved draft";
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        source.sync_all().unwrap();
        let command = DraftCommand::Write {
            target: target.clone(),
            expected: engine.file_working_state(&target).unwrap().revision(),
            base: engine.file_draft_base(&target).unwrap(),
            content_hash: ContentHash::from_digest(blake3::hash(bytes)),
            logical_size: bytes.len() as u64,
        };
        engine.write_file_draft(
            &command, &principal, OperationId::generate().unwrap(), ReadFile::from(source),
        ).unwrap();
    }
    Fixture {
        temporary, engine, target, old: versions.remove(0), current: versions.remove(0),
        history: repository.history_tree_id,
    }
}

fn request(fixture: &Fixture) -> Request {
    Request {
        history_tree_id: fixture.history.clone(),
        history_root: crate::history_package::retirement_history_root(
            fixture.engine.state(), &fixture.history,
        ).unwrap(),
        origin_file_id: fixture.target.file.to_string(),
        version_ids: vec![fixture.old.clone()],
    }
}

/// A cross-record counterexample, deliberately not a published draft write.
/// Use a real earlier ancestor of this FILE, not a fabricated ID or payload.
fn with_older_base(fixture: &Fixture) -> State {
    use storage_contract::file_draft::FileWorkingState;
    let mut state = fixture.engine.state().clone();
    let record = state.file_containers.get_mut(&fixture.target.file).unwrap();
    let original = *record.working.draft().unwrap().0;
    let older = DraftBase::new(
        original.branch_id(), fixture.old.parse().unwrap(), fixture.old.parse().unwrap(),
        original.selection_generation(),
    );
    match &mut record.working {
        FileWorkingState::ManualDirty { base, .. }
        | FileWorkingState::ManualIncremental { base, .. } => *base = older,
        _ => panic!("fixture produced an actual full or incremental draft"),
    }
    state
}

#[test]
fn retirement_and_purge_pin_exact_full_and_incremental_draft_bases() {
    for incremental in [false, true] {
        let fixture = fixture(incremental);
        let before = fixture.engine.state().clone();
        retirement::require_unpinned_working(&before, |version| version == fixture.old).unwrap();
        let error = retirement::require_unpinned_working(
            &before, |version| version == fixture.current,
        ).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ContentPinned);
        fixture.engine.check_purge(&fixture.history, &[fixture.old.clone()]).unwrap();
        assert_eq!(fixture.engine.check_purge(
            &fixture.history, &[fixture.current.clone()],
        ).unwrap_err().code(), ErrorCode::ContentPinned);

        let older = with_older_base(&fixture);
        assert_eq!(retirement::decide(&older, request(&fixture)).unwrap_err().code(),
            ErrorCode::ContentPinned);
        assert_eq!(fixture.engine.state(), &before);

        // Missing/offline or unreadable physical material never turns a saved
        // draft reference into permission to purge its base. No disk probe is
        // consulted by this admission, and the real records are unchanged.
        let disk = fixture.temporary.path().join("disk");
        let parked = fixture.temporary.path().join("offline-disk");
        fs::rename(&disk, &parked).unwrap();
        fs::set_permissions(&parked, fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(retirement::decide(&older, request(&fixture)).unwrap_err().code(),
            ErrorCode::ContentPinned);
        fs::set_permissions(&parked, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(&parked, &disk).unwrap();
        assert_eq!(fixture.engine.state(), &before);
    }
}

#[test]
fn unrelated_draft_survives_retirement_exact_retry_and_cold_reopen() {
    for incremental in [false, true] {
        let mut fixture = fixture(incremental);
        let original = request(&fixture);
        let working = fixture.engine.file_working_state(&fixture.target).unwrap();
        let read = fixture.engine.capture_shared_history(
            SharedHistoryRequest::parse(&original.origin_file_id, &original.history_root).unwrap(),
            HistoryWindowCancellation::new(),
        ).unwrap();
        let selection = crate::PermanentNodeRemovalSelection::new(vec![
            HistoryVersionId::parse(&fixture.old).unwrap(),
        ]).unwrap();
        let operation = OperationId::generate().unwrap();
        let publication = fixture.engine.retire_file_nodes(
            &read, &selection, "admin", operation,
        ).unwrap().publication().clone();
        assert_eq!(fixture.engine.file_working_state(&fixture.target).unwrap(), working);
        assert_eq!(fixture.engine.retry_file_node_retirement_at_root(
            operation, "admin", &original.origin_file_id.parse().unwrap(),
            original.history_root.parse().unwrap(), &selection,
        ).unwrap().publication(), &publication);
        retirement::admit_current(fixture.engine.state()).unwrap();

        let revived = with_older_base(&fixture);
        assert_eq!(retirement::admit_current(&revived).unwrap_err().code(), ErrorCode::ContentPinned);
        assert_eq!(retirement::verify(fixture.engine.state(), &revived).unwrap_err().code(),
            ErrorCode::ContentPinned);
        drop(read);
        drop(fixture.engine);
        let mut reopened = Engine::open(&fixture.temporary.path().join("system")).unwrap();
        assert_eq!(reopened.file_working_state(&fixture.target).unwrap(), working);
        assert_eq!(reopened.resume_file_node_retirement(operation, "admin").unwrap().publication(),
            &publication);
        assert_eq!(reopened.file_working_state(&fixture.target).unwrap(), working);
    }
}
