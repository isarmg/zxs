use super::*;
use crate::files::test_entries::{entry, entry_wire, parse_entry};
use json_codec::json;
use storage_contract::file_draft::{DraftBase, FileWorkingState, WorkingRevision};
use storage_contract::{EntryId, FileId, Generation, RepositoryId};

fn location() -> DraftLocation {
    DraftLocation {
        repository: RepositoryId::from_random([1; 16]),
        path: name_policy::PortablePath::parse("notes.txt").unwrap(),
        entry: EntryId::from_random([2; 16]),
        file: FileId::from_random([3; 16]),
    }
}

fn state() -> State {
    let location = location();
    let version = VersionId::from_random([4; 16]);
    let branch = BranchId::from_random([5; 16]);
    let node: FileNodeId = format!("fnode_{}", "06".repeat(16)).parse().unwrap();
    let mut state = State {
        authenticated: true,
        principal: Some("alice".parse().unwrap()),
        session_epoch: 7,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    state.files.generation = 2;
    state.files.selected = Some(entry(json!({
        "entry_id":location.entry, "file_id":location.file, "path":location.path,
        "repository_id":location.repository, "repository_path":location.path,
        "version_id":version, "own_version_id":version, "selected_branch_id":branch,
        "node_id":node,
    })));
    state
}

fn observed(state: &State, working: Value) -> Snapshot {
    let target = Target::capture(state).unwrap();
    Snapshot::parse(
        json!({
            "base":DraftBase::new(target.branch, target.node, target.version, Generation::new(1)),
            "working":working,
        }),
        &target,
    )
    .unwrap()
}

fn review(state: &mut State, mode: &str) {
    let target = Target::capture(state).unwrap();
    let snapshot = observed(state, json!({"state":mode,"revision":"3"}));
    let generation = state.files.commit_mode.generation;
    complete_load(state, target, generation, Ok(snapshot));
}

#[test]
fn reviewed_mode_creates_only_explicit_exact_selected_file_command() {
    let mut state = state();
    assert!(can_load(&state));
    assert!(!can_change(&state, CommitMode::Automatic));
    review(&mut state, "manual_clean");
    assert!(can_change(&state, CommitMode::Automatic));
    assert!(!can_change(&state, CommitMode::Manual));
    let operation = OperationId::from_random([7; 16]);
    let pending = begin(&mut state, CommitMode::Automatic, operation).unwrap();
    assert_eq!(
        pending.endpoint(),
        format!("/api/file-manager/files/{}/working", location().file,)
    );
    assert_eq!(
        pending.body().unwrap(),
        json!({
            "operation_id":operation,
            "command":{"action":"set_mode", "target":location(), "expected":"3",
                "mode":"automatic"},
        })
    );
    assert!(
        begin(
            &mut state,
            CommitMode::Automatic,
            OperationId::from_random([8; 16])
        )
        .is_none()
    );
    state.files.mutation_pending = false;
    state.files.clear_selection();
    settle(
        &mut state.files.commit_mode,
        &operation.to_string(),
        SaveStatus::Unknown,
    );
    assert!(Arc::ptr_eq(
        state.files.commit_mode.pending.as_ref().unwrap(),
        &pending
    ));
    settle(
        &mut state.files.commit_mode,
        &operation.to_string(),
        SaveStatus::Published,
    );
    assert!(state.files.commit_mode.pending.is_none());
    assert!(state.files.commit_mode.review.is_none());

    let mut automatic = self::state();
    review(&mut automatic, "automatic");
    assert!(can_change(&automatic, CommitMode::Manual));
    assert!(!can_change(&automatic, CommitMode::Automatic));
    let manual = begin(&mut automatic, CommitMode::Manual, operation).unwrap();
    assert_eq!(manual.body().unwrap()["command"]["mode"], "manual");
}

#[test]
fn dirty_draft_and_stale_navigation_never_enable_automatic_or_reuse_a_review() {
    let mut state = state();
    let target = Target::capture(&state).unwrap();
    let clean = observed(&state, json!({"state":"manual_clean","revision":"3"}));
    let dirty = FileWorkingState::ManualDirty {
        revision: WorkingRevision::new(Generation::new(1)),
        base: clean.base,
        payload: storage_contract::file_draft::DraftPayload::new(
            storage_contract::file_draft::DraftPayloadId::from_random([8; 16]),
            "ab".repeat(32).parse().unwrap(),
            12,
        ),
    };
    let snapshot = observed(&state, json_codec::to_value(dirty).unwrap());
    let generation = state.files.commit_mode.generation;
    complete_load(&mut state, target.clone(), generation, Ok(snapshot));
    assert!(!can_change(&state, CommitMode::Automatic));
    assert!(!can_change(&state, CommitMode::Manual));
    assert!(
        begin(
            &mut state,
            CommitMode::Automatic,
            OperationId::from_random([9; 16])
        )
        .is_none()
    );
    state.files.generation += 1;
    assert!(!target.current(&state));
    state.files.commit_mode.clear_review();
    complete_load(&mut state, target, generation, Ok(clean));
    assert!(state.files.commit_mode.review.is_none());

    let row = entry_wire(json!({
        "path":"notes.txt", "file_id":location().file, "read_only":true,
    }));
    state.files.selected = Some(parse_entry(row).unwrap());
    assert!(Target::capture(&state).is_none());
    state.authenticated = false;
    assert!(!can_load(&state));
}

#[test]
fn mode_response_and_receipt_bind_selection_and_original_command() {
    let mut state = state();
    let target = Target::capture(&state).unwrap();
    let snapshot = observed(&state, json!({"state":"manual_clean","revision":"3"}));
    let mut value = json!({"base":snapshot.base, "working":snapshot.working});
    value["base"]["version_id"] = json!(VersionId::from_random([9; 16]));
    assert!(Snapshot::parse(value, &target).is_err());
    assert!(Snapshot::parse(json!([snapshot.base, snapshot.working]), &target).is_err());
    review(&mut state, "manual_clean");
    let pending = begin(
        &mut state,
        CommitMode::Automatic,
        OperationId::from_random([7; 16]),
    )
    .unwrap();
    let command = pending.body().unwrap()["command"].clone();
    let digest = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"file.working.mode", "principal":"alice", "body":command,
        }),
    )
    .unwrap();
    let receipt = json!({
        "operation_id":pending.operation, "operation_kind":"file.working.mode",
        "request_digest":digest, "result":{"request":command},
        "state":"completed", "commit_state":"PUBLISHED", "error":null,
        "protection_at_publish":"NOT_APPLICABLE", "publication_sequence":"5",
    });
    assert!(receipt_matches(
        &state,
        &pending.operation.to_string(),
        &receipt
    ));
    for (field, changed) in [
        ("operation_id", json!(OperationId::from_random([8; 16]))),
        ("operation_kind", json!("file.draft.commit")),
        ("request_digest", json!("00".repeat(32))),
        (
            "result",
            json!({"request":{"action":"set_mode", "mode":"manual"}}),
        ),
    ] {
        let mut bad = receipt.clone();
        bad[field] = changed;
        assert!(
            !receipt_matches(&state, &pending.operation.to_string(), &bad),
            "{field}"
        );
    }
    state.session_epoch += 1;
    assert!(!receipt_matches(
        &state,
        &pending.operation.to_string(),
        &receipt
    ));
}
