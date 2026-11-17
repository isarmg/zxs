use super::wire::{Command, Content, Expectation, Snapshot};
use super::{
    Model, Original, Pending, Phase, Record, Target, blocks_writes, can_review, can_submit,
    recovery::{complete_cleanup, complete_save, owns_phase, restore_loaded, retry_record},
};
use crate::files::test_entries::parse_entry;
use crate::ui::State;
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::file_draft::{DraftLocation, WorkingRevision};
use storage_contract::{EntryId, FileId, OperationId, RepositoryId};

fn target() -> DraftLocation {
    DraftLocation {
        repository: RepositoryId::from_random([1; 16]),
        entry: EntryId::from_random([2; 16]),
        file: FileId::from_random([3; 16]),
        path: name_policy::PortablePath::parse("first.txt").unwrap(),
    }
}
fn row() -> Value {
    let target = target();
    json!({
        "entry_id":target.entry, "name":"first.txt", "path":"first.txt",
        "kind":"file", "size":"0", "file_id":target.file, "working_state":"unborn",
        "repository_id":target.repository, "repository_path":"first.txt",
        "container":true, "selected_branch_id":null, "read_only":false,
    })
}
fn snapshot() -> Value {
    json!({
        "target":target(), "expected":{"revision":"0","content":{"state":"empty"}},
        "mode":"manual", "logical_size":"0",
    })
}
fn state() -> State {
    let principal: storage_contract::Principal = "alice".parse().unwrap();
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        principal: Some(principal.clone()),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    state.files.generation = 4;
    state.files.selected = Some(parse_entry(row()).unwrap());
    state.files.unborn = Model {
        owner: Some((7, principal)),
        storage_ready: true,
        ..Default::default()
    };
    state
}
fn original() -> Record {
    Record::new(
        "alice".parse().unwrap(),
        Command {
            operation_id: OperationId::from_random([4; 16]),
            target: target(),
            expected: Expectation {
                revision: json_codec::from_value::<WorkingRevision>(json!("0")).unwrap(),
                content: Content::Empty {},
            },
            message: "first explicit history".into(),
        },
    )
    .unwrap()
}

#[test]
fn listing_admits_unborn_without_manufacturing_version_or_branch_fields() {
    let entry = parse_entry(row()).unwrap();
    assert_eq!(entry.unborn(), Some(&target()));
    assert_eq!(entry.version_id(), None);
    assert_eq!(entry.node_id(), None);
    assert_eq!(entry.selected_branch_id(), None);
    assert!(!entry.visible_content() && !entry.inherited());
    assert_eq!(entry.size(), 0);
    for (field, value) in [
        ("version_id", json!("fake-version")),
        ("node_id", json!("fake-node")),
        ("own_version_id", Value::Null),
        ("materialized", json!(false)),
        ("selected_branch_id", json!("fake-branch")),
        ("read_only", json!(true)),
        ("backup", json!(true)),
        ("working_state", json!("unknown")),
        ("file_id", json!(EntryId::from_random([3; 16]))),
    ] {
        let mut bad = row();
        bad[field] = value;
        assert!(parse_entry(bad).is_err(), "{field}");
    }
    for field in [
        "working_state",
        "entry_id",
        "repository_id",
        "repository_path",
    ] {
        let mut bad = row();
        bad.as_object_mut().unwrap().remove(field);
        assert!(parse_entry(bad).is_err(), "{field}");
    }
}

#[test]
fn first_commit_review_pins_selection_and_retains_exact_original_across_navigation() {
    let mut state = state();
    assert!(can_review(&state));
    let selected = Target::capture(&state).unwrap();
    let observed = Snapshot::parse(snapshot(), &target()).unwrap();
    state.files.unborn.review = Some((selected.clone(), observed));
    assert!(can_submit(&state));
    state.files.generation += 1;
    assert!(!can_submit(&state));
    state.files.generation -= 1;
    let retained = Arc::new(Original::create(original()).unwrap());
    let record = &retained.request;
    let frozen = record.encode().unwrap();
    state.files.unborn.pending = Some(Pending {
        original: Arc::clone(&retained),
        phase: Phase::Pending,
    });
    assert!(blocks_writes(&state));
    assert!(!can_submit(&state));
    state.files.clear_selection();
    assert!(state.files.unborn.review.is_none());
    assert_eq!(
        retry_record(&state).unwrap().request.encode().unwrap(),
        frozen
    );
    assert_eq!(Record::parse(&frozen, &record.principal).unwrap(), *record);
    assert!(Record::parse(&frozen, &"bob".parse().unwrap()).is_err());
    let mut malformed: Value = json_codec::from_str(&frozen).unwrap();
    malformed["command"]["target"] = json!([target().repository, target().entry, target().file]);
    assert!(Record::parse(&malformed.to_string(), &record.principal).is_err());
    let duplicate = frozen.replacen(
        "{",
        "{\"format\":\"unborn-first-commit-original-request\",",
        1,
    );
    assert!(Record::parse(&duplicate, &record.principal).is_err());
    state.session_epoch += 1;
    assert!(retry_record(&state).is_none());
    assert!(!selected.current(&state));
    state.session_epoch -= 1;
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Sending;
    assert!(
        retry_record(&state).is_none(),
        "duplicate clicks cannot resubmit in flight"
    );
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
    assert!(
        retry_record(&state).is_none(),
        "cleanup retry never repeats a settled mutation"
    );
}

#[test]
fn snapshot_and_original_receipt_require_exact_content_pin_and_strict_shape() {
    let location = target();
    let mut saved = snapshot();
    saved["logical_size"] = json!("9");
    assert!(Snapshot::parse(saved.clone(), &location).is_err());
    saved["expected"]["content"] = json!({
        "state":"saved", "generation":"1", "commitment":"ab".repeat(32),
    });
    let parsed = Snapshot::parse(saved.clone(), &location).unwrap();
    assert_eq!(parsed.logical_size, 9);
    let mut mismatched = saved.clone();
    mismatched["target"]["entry"] = json!(EntryId::from_random([8; 16]));
    assert!(Snapshot::parse(mismatched, &location).is_err());
    saved["version_id"] = json!("fake-version");
    assert!(Snapshot::parse(saved, &location).is_err());

    let record = original();
    let native = json!({
        "target":location, "expected":"0", "body":{"kind":"empty"},
        "message":record.command.message,
    });
    let digest = storage_contract::object_hash(
        "request",
        &json!({
            "kind":super::wire::KIND, "principal":record.principal, "body":native,
        }),
    )
    .unwrap();
    let value = json!({
        "operation_id":record.command.operation_id, "operation_kind":super::wire::KIND,
        "request_digest":digest, "state":"completed", "commit_state":"PUBLISHED",
        "result":{"request":native}, "error":null, "protection_at_publish":"TARGET_MET",
        "publication_sequence":"3",
    });
    assert_eq!(
        record.receipt_status(&value).unwrap(),
        Some(super::SaveStatus::Published)
    );
    for (field, replacement) in [
        ("operation_id", json!(OperationId::from_random([9; 16]))),
        ("operation_kind", json!("file.working.save")),
        ("request_digest", json!("00".repeat(32))),
        ("result", json!({"request":{"target":location}})),
    ] {
        let mut bad = value.clone();
        bad[field] = replacement;
        assert_eq!(record.receipt_status(&bad).unwrap(), None, "{field}");
    }
}

fn pending(state: &mut State, phase: Phase) -> Arc<Original> {
    let original = Arc::new(Original::create(original()).unwrap());
    state.files.unborn.pending = Some(Pending {
        original: Arc::clone(&original),
        phase,
    });
    original
}

#[test]
fn lock_wait_blocks_new_work_and_failed_retention_never_admits_a_send() {
    let mut state = state();
    let original = pending(&mut state, Phase::Retaining);
    let epoch = state.session_epoch;
    assert!(blocks_writes(&state));
    assert!(!can_review(&state));
    assert!(!can_submit(&state));
    assert!(retry_record(&state).is_none());
    assert!(owns_phase(&state, epoch, &original, Phase::Retaining));
    let error = super::invalid("lock unavailable");
    assert!(!complete_save(&mut state, epoch, &original, Err(error)));
    assert!(!state.files.unborn.changing_storage());
    assert!(!state.files.unborn.storage_ready);
    assert!(state.files.unborn.pending.is_none());
    assert!(blocks_writes(&state));
    assert_eq!(
        state.files.unborn.error.as_ref().unwrap().message,
        "lock unavailable"
    );
}

#[test]
fn late_retention_cannot_adopt_another_owner_or_a_new_session() {
    for changed_account in [false, true] {
        let mut state = state();
        let original = pending(&mut state, Phase::Retaining);
        let epoch = state.session_epoch;
        state.session_epoch += 1;
        if changed_account {
            state.principal = Some("bob".parse().unwrap());
        }
        assert!(!owns_phase(&state, epoch, &original, Phase::Retaining));
        let owner = (state.session_epoch, state.principal.clone().unwrap());
        restore_loaded(&mut state.files.unborn, owner.clone(), Ok(None));
        assert!(!complete_save(&mut state, epoch, &original, Ok(())));
        assert_eq!(state.files.unborn.owner, Some(owner));
        assert!(state.files.unborn.pending.is_none());
    }
}

#[test]
fn new_owner_read_failure_does_not_expose_the_previous_original() {
    let mut state = state();
    pending(&mut state, Phase::Removing);
    restore_loaded(
        &mut state.files.unborn,
        (8, "bob".parse().unwrap()),
        Err(super::invalid("new owner storage unavailable")),
    );
    assert!(state.files.unborn.pending.is_none());
    assert!(!state.files.unborn.storage_ready);
    assert_eq!(state.files.unborn.owner, Some((8, "bob".parse().unwrap())));
}

#[test]
fn rereading_original_preserves_settled_cleanup_and_rejects_replacement() {
    let mut state = state();
    let record = pending(&mut state, Phase::Settled);
    let owner = state.files.unborn.owner.clone().unwrap();
    let again = Original::create(original()).unwrap();
    restore_loaded(&mut state.files.unborn, owner.clone(), Ok(Some(again)));
    assert!(Arc::ptr_eq(
        &state.files.unborn.pending.as_ref().unwrap().original,
        &record
    ));
    assert_eq!(
        state.files.unborn.pending.as_ref().unwrap().phase,
        Phase::Settled
    );
    assert!(retry_record(&state).is_none());
    let mut replacement = original();
    replacement.command.operation_id = OperationId::from_random([9; 16]);
    let mut bytes = original().encode().unwrap();
    bytes.push(' ');
    let reencoded = Original::read(&bytes, &record.request.principal).unwrap();
    assert_eq!(reencoded.request, record.request);
    restore_loaded(&mut state.files.unborn, owner.clone(), Ok(Some(reencoded)));
    assert!(!state.files.unborn.storage_ready);
    assert!(Arc::ptr_eq(
        &state.files.unborn.pending.as_ref().unwrap().original,
        &record
    ));
    assert_eq!(
        state.files.unborn.pending.as_ref().unwrap().phase,
        Phase::Settled
    );
    restore_loaded(
        &mut state.files.unborn,
        owner.clone(),
        Ok(Some(Original::create(replacement.clone()).unwrap())),
    );
    let next = state.files.unborn.pending.as_ref().unwrap();
    assert_eq!(next.original.request, replacement);
    assert_eq!(next.phase, Phase::Pending);
    assert!(state.files.unborn.storage_ready);
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
    restore_loaded(&mut state.files.unborn, owner, Ok(None));
    assert!(state.files.unborn.storage_ready);
    assert!(state.files.unborn.pending.is_none());
}

#[test]
fn missing_or_unreadable_storage_does_not_discard_an_unconfirmed_original() {
    let mut state = state();
    let original = pending(&mut state, Phase::Pending);
    let owner = state.files.unborn.owner.clone().unwrap();
    for result in [Ok(None), Err(super::invalid("unreadable original"))] {
        restore_loaded(&mut state.files.unborn, owner.clone(), result);
        assert!(!state.files.unborn.storage_ready);
        assert!(Arc::ptr_eq(
            &state.files.unborn.pending.as_ref().unwrap().original,
            &original
        ));
        assert!(retry_record(&state).is_none());
        assert_eq!(
            state.files.unborn.pending.as_ref().unwrap().phase,
            Phase::Pending
        );
    }
}

#[test]
fn cleanup_completion_requires_the_same_owner_epoch_and_exact_settled_record() {
    let mut state = state();
    let record = pending(&mut state, Phase::Removing);
    let epoch = state.session_epoch;
    complete_cleanup(
        &mut state,
        epoch,
        &record,
        Err(super::invalid("remove failed")),
    );
    assert_eq!(
        state.files.unborn.pending.as_ref().unwrap().phase,
        Phase::Settled
    );
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Removing;
    let stale = Arc::new(Original::create(original()).unwrap());
    complete_cleanup(&mut state, epoch, &stale, Ok(()));
    assert!(state.files.unborn.pending.is_some());
    complete_cleanup(&mut state, epoch + 1, &record, Ok(()));
    assert!(state.files.unborn.pending.is_some());
    complete_cleanup(&mut state, epoch, &record, Ok(()));
    assert!(state.files.unborn.pending.is_none());
    assert!(state.files.unborn.error.is_none());
    assert!(!state.files.unborn.changing_storage());
}

#[test]
fn same_account_relogin_recovers_the_hint_without_repeating_work_or_unsettling_publication() {
    for (phase, expected) in [
        (Phase::LookingUp, Phase::Pending),
        (Phase::RetryQueued, Phase::Pending),
        (Phase::Sending, Phase::Pending),
        (Phase::Removing, Phase::Settled),
    ] {
        let mut state = state();
        let record = pending(&mut state, phase);
        let prior = state.session_epoch;
        state.session_epoch += 1;
        let owner = (state.session_epoch, state.principal.clone().unwrap());
        restore_loaded(
            &mut state.files.unborn,
            owner,
            Ok(Some(Original::create(original()).unwrap())),
        );
        let retained = state.files.unborn.pending.as_ref().unwrap();
        assert!(Arc::ptr_eq(&retained.original, &record));
        assert_eq!(retained.phase, expected);
        assert!(!owns_phase(&state, prior, &record, phase));
    }
}
