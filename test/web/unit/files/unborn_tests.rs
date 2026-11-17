use super::{
    Model, Pending, Phase, Record, Target, blocks_writes, can_review, can_submit, retry_record,
};
use super::wire::{Command, Content, Expectation, Snapshot};
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
        owner: Some((7, principal)), storage_ready: true, ..Default::default()
    };
    state
}
fn original() -> Record {
    Record::new("alice".parse().unwrap(), Command {
        operation_id: OperationId::from_random([4; 16]),
        target: target(),
        expected: Expectation {
            revision: json_codec::from_value::<WorkingRevision>(json!("0")).unwrap(),
            content: Content::Empty {},
        },
        message: "first explicit history".into(),
    }).unwrap()
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
    for field in ["working_state", "entry_id", "repository_id", "repository_path"] {
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
    let record = Arc::new(original());
    let frozen = record.encode().unwrap();
    state.files.unborn.pending = Some(Pending {
        record: Arc::clone(&record), phase: Phase::Pending,
    });
    assert!(blocks_writes(&state));
    assert!(!can_submit(&state));
    state.files.clear_selection();
    assert!(state.files.unborn.review.is_none());
    assert_eq!(retry_record(&state).unwrap().encode().unwrap(), frozen);
    assert_eq!(Record::parse(&frozen, &record.principal).unwrap(), *record);
    assert!(Record::parse(&frozen, &"bob".parse().unwrap()).is_err());
    let mut malformed: Value = json_codec::from_str(&frozen).unwrap();
    malformed["command"]["target"] = json!([target().repository, target().entry, target().file]);
    assert!(Record::parse(&malformed.to_string(), &record.principal).is_err());
    let duplicate = frozen.replacen(
        "{", "{\"format\":\"unborn-first-commit-original-request\",", 1,
    );
    assert!(Record::parse(&duplicate, &record.principal).is_err());
    state.session_epoch += 1;
    assert!(retry_record(&state).is_none());
    assert!(!selected.current(&state));
    state.session_epoch -= 1;
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Sending;
    assert!(retry_record(&state).is_none(), "duplicate clicks cannot resubmit in flight");
    state.files.unborn.pending.as_mut().unwrap().phase = Phase::Settled;
    assert!(retry_record(&state).is_none(), "cleanup retry never repeats a settled mutation");
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
    let digest = storage_contract::object_hash("request", &json!({
        "kind":super::wire::KIND, "principal":record.principal, "body":native,
    })).unwrap();
    let value = json!({
        "operation_id":record.command.operation_id, "operation_kind":super::wire::KIND,
        "request_digest":digest, "state":"completed", "commit_state":"PUBLISHED",
        "result":{"request":native}, "error":null, "protection_at_publish":"TARGET_MET",
        "publication_sequence":"3",
    });
    assert_eq!(record.receipt_status(&value).unwrap(), Some(super::SaveStatus::Published));
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
