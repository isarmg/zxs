use super::{
    BodyKind, Model, Pending, Phase, ReadItem, Review, Target, blocks_writes, can_initialize,
    can_review, can_submit, requests, wire,
};
use crate::files::test_entries::parse_entry;
use crate::ui::State;
use json_codec::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::file_draft::{
    DraftBase, DraftLocation, DraftPayload, DraftPayloadId, WorkingRevision,
};
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotHeadDiscovery,
};
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftExpectation, DirectoryUnbornExpectation,
    SelectedDirectoryDraft,
};
use storage_contract::snapshot_worktree::{
    InitializeSnapshot, SnapshotHandle, SnapshotHost, SnapshotNamespace, SnapshotRootContext,
};
use storage_contract::working_tree::{
    DirectoryId, RootId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{
    BranchId, CommitId, ContentHash, EntryId, FileId, FileNodeId, Generation, OperationId,
    RepositoryId, VersionId,
};
use wire::{Candidate, Command, Record};

fn repository() -> RepositoryId {
    RepositoryId::from_random([1; 16])
}
pub(super) fn location(seed: u8, name: &str) -> DraftLocation {
    DraftLocation {
        repository: repository(),
        entry: EntryId::from_random([seed; 16]),
        file: FileId::from_random([seed; 16]),
        path: name_policy::PortablePath::parse(name).unwrap(),
    }
}
fn source() -> SnapshotHandle {
    SnapshotHandle {
        publication: OperationId::from_random([2; 16]),
        repository_id: SnapshotRepositoryId::from_random([3; 16]),
        branch_id: SnapshotBranchId::from_random([4; 16]),
        node_id: SnapshotNodeId::from_random([5; 16]),
    }
}
pub(super) fn head() -> DirectorySnapshotHead {
    DirectorySnapshotHead {
        repository_id: repository(),
        context: SnapshotRootContext {
            host: SnapshotHost {
                root_id: RootId::from_random([6; 16]),
                directory_id: DirectoryId::from_random([7; 16]),
                expected_root_generation: Generation::new(3),
            },
            source: SnapshotNamespace {
                branch_id: BranchId::from_random([8; 16]),
                root: None,
                expected_branch_generation: Generation::new(4),
            },
        },
        head: SnapshotHeadDiscovery::Head {
            snapshot: source(),
            branch_name: storage_contract::BranchName::parse("actual-branch").unwrap(),
        },
    }
}
fn revision() -> WorkingRevision {
    WorkingRevision::new(Generation::new(2))
}
pub(super) fn born() -> Candidate {
    Candidate {
        selected: SelectedDirectoryDraft {
            target: location(11, "changed.txt"),
            expected: DirectoryDraftExpectation::Born {
                revision: revision(),
                base: DraftBase::new(
                    BranchId::from_random([12; 16]),
                    format!("fnode_{}", "0a".repeat(16))
                        .parse::<FileNodeId>()
                        .unwrap(),
                    VersionId::from_random([13; 16]),
                    Generation::new(1),
                ),
                payload: DraftPayload::new(
                    DraftPayloadId::from_random([14; 16]),
                    "ab".repeat(32).parse::<ContentHash>().unwrap(),
                    7,
                ),
            },
        },
        size: 7,
    }
}
pub(super) fn unborn() -> Candidate {
    Candidate {
        selected: SelectedDirectoryDraft {
            target: location(21, "first.txt"),
            expected: DirectoryDraftExpectation::Unborn {
                revision: revision(),
                content: DirectoryUnbornExpectation::Empty {},
            },
        },
        size: 0,
    }
}
fn record() -> Record {
    Record::new(
        "alice".parse().unwrap(),
        Command::Commit {
            repository: repository(),
            request: CommitDirectoryDrafts {
                source: source(),
                selected: vec![born().selected, unborn().selected],
                message: "one explicit set".into(),
                operation_id: OperationId::from_random([9; 16]),
            },
        },
    )
    .unwrap()
}
pub(super) fn state() -> State {
    let principal = "alice".parse().unwrap();
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        principal: Some(principal),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.repository_id = repository().to_string();
    state.files.namespace_head = CommitId::from_random([10; 16]).to_string();
    state.files.loaded = true;
    state.files.writable = true;
    state.files.generation = 2;
    state.files.directory_snapshot = Model {
        owner: Some((7, state.principal.clone().unwrap())),
        storage_ready: true,
        ..Default::default()
    };
    state
}
pub(super) fn unborn_row() -> Value {
    let target = unborn().selected.target;
    json!({
        "entry_id":target.entry, "name":"first.txt", "path":"first.txt",
        "kind":"file", "size":"0", "file_id":target.file, "working_state":"unborn",
        "repository_id":target.repository, "repository_path":"first.txt",
        "container":true, "selected_branch_id":null, "read_only":false,
    })
}

#[test]
fn directory_snapshot_uses_actual_head_and_exact_mixed_original_request() {
    let mut state = state();
    let target = Target::capture(&state).unwrap();
    assert_eq!(
        target.endpoint(),
        format!("/api/repositories/{}/snapshot-head", repository())
    );
    let observed = wire::head(json_codec::to_value(head()).unwrap(), repository()).unwrap();
    state.files.directory_snapshot.review = Some(Review {
        target: target.clone(),
        head: observed,
        candidates: vec![born(), unborn()],
        selected: BTreeSet::new(),
    });
    assert!(can_review(&state));
    assert!(
        !can_submit(&state),
        "review never silently selects all drafts"
    );
    state
        .files
        .directory_snapshot
        .review
        .as_mut()
        .unwrap()
        .selected
        .insert(location(21, "first.txt").entry);
    assert!(can_submit(&state));
    state.files.generation += 1;
    assert!(
        !can_submit(&state),
        "a new listing invalidates the reviewed source"
    );
    state.files.generation -= 1;
    state.files.repository_path = "child".into();
    assert!(Target::capture(&state).is_some(), "a child can contribute to its original root");
    assert!(!target.current(&state));
    assert!(!can_submit(&state), "a child never inherits a root page review");
    state.files.repository_path.clear();

    let record = Arc::new(record());
    let body = record.command.body().unwrap();
    assert_eq!(
        record.command.endpoint(),
        "/api/directory-snapshots/commits"
    );
    assert_eq!(body["source"], json_codec::to_value(source()).unwrap());
    assert_eq!(body["selected"].as_array().unwrap().len(), 2);
    assert!(body["selected"][0]["expected"].get("base").is_some());
    assert!(body["selected"][1]["expected"].get("base").is_none());
    assert_eq!(body["selected"][1]["expected"]["content"]["state"], "empty");
    let frozen = record.encode().unwrap();
    assert_eq!(Record::parse(&frozen, &record.principal).unwrap(), *record);
    state.files.directory_snapshot.pending = Some(Pending {
        record: Arc::clone(&record),
        phase: Phase::Pending,
    });
    assert!(blocks_writes(&state));
    assert!(!can_submit(&state));
    state.files.clear_selection();
    assert_eq!(
        requests::retry_record(&state).unwrap().encode().unwrap(),
        frozen
    );
    state
        .files
        .directory_snapshot
        .pending
        .as_mut()
        .unwrap()
        .phase = Phase::Sending;
    assert!(requests::retry_record(&state).is_none());
    state
        .files
        .directory_snapshot
        .pending
        .as_mut()
        .unwrap()
        .phase = Phase::Settled;
    assert!(requests::retry_record(&state).is_none());
    state.session_epoch += 1;
    assert!(!target.current(&state));
    assert!(requests::retry_record(&state).is_none());
}

#[test]
fn directory_snapshot_review_keeps_unborn_identity_and_rejects_wrong_responses() {
    let target = unborn().selected.target;
    let entry = parse_entry(unborn_row()).unwrap();
    let item = ReadItem::from_entry(&entry, repository()).unwrap().unwrap();
    assert_eq!(item.target, target);
    assert_eq!(item.kind, BodyKind::Unborn);
    assert!(item.endpoint().contains("/unborn-working?"));
    let value = json!({
        "target":target, "expected":{"revision":"2","content":{"state":"empty"}},
        "mode":"manual", "automatic":null, "logical_size":"0",
    });
    let mut incomplete = value.clone();
    incomplete.as_object_mut().unwrap().remove("automatic");
    assert!(wire::unborn(incomplete, target.clone()).is_err());
    let mut inconsistent = value.clone();
    inconsistent["mode"] = "automatic".into();
    assert!(wire::unborn(inconsistent, target.clone()).is_err());
    assert_eq!(
        wire::unborn(value.clone(), target.clone()).unwrap(),
        unborn()
    );
    let mut bad = value.clone();
    bad["target"]["entry"] = json!(EntryId::from_random([99; 16]));
    assert!(wire::unborn(bad, target.clone()).is_err());
    let mut bad = value;
    bad["logical_size"] = json!("1");
    assert!(wire::unborn(bad, target).is_err());

    let born = born();
    let DirectoryDraftExpectation::Born {
        revision,
        base,
        payload,
    } = born.selected.expected
    else {
        panic!("fixture must be born")
    };
    let value = json!({
        "base":base,
        "working":{"state":"manual_dirty","revision":revision,"base":base,"payload":payload},
    });
    assert_eq!(
        wire::born(value, born.selected.target.clone()).unwrap(),
        Some(born)
    );
    let clean = json!({"base":base,"working":{"state":"manual_clean","revision":revision}});
    assert!(
        wire::born(clean, location(11, "changed.txt"))
            .unwrap()
            .is_none()
    );
    let mut wrong = json_codec::to_value(head()).unwrap();
    wrong["repository_id"] = json!(RepositoryId::from_random([99; 16]));
    assert!(wire::head(wrong, repository()).is_err());
    let mut wrong = json_codec::to_value(head()).unwrap();
    wrong["context"]["host"] = json!(["x", "y", "3"]);
    assert!(wire::head(wrong, repository()).is_err());
}

#[test]
fn directory_snapshot_initializes_only_a_complete_empty_or_born_root() {
    let mut state = state();
    let mut context = head();
    context.head = SnapshotHeadDiscovery::NotInitialized;
    state.files.directory_snapshot.review = Some(Review {
        target: Target::capture(&state).unwrap(),
        head: context.clone(),
        candidates: vec![],
        selected: BTreeSet::new(),
    });
    assert!(
        can_initialize(&state),
        "an empty root is the normal starting path"
    );
    state.files.query = "filtered".into();
    assert!(!can_initialize(&state));
    state.files.query.clear();
    state.files.next_cursor = Some("next".into());
    assert!(!can_initialize(&state));
    state.files.next_cursor = None;
    state.files.entries = vec![parse_entry(unborn_row()).unwrap()].into();
    state.files.total_entries = 1;
    assert!(
        !can_initialize(&state),
        "unborn data must not be omitted or implicitly committed"
    );
    let record = Record::new(
        "alice".parse().unwrap(),
        Command::Initialize {
            repository: repository(),
            request: InitializeSnapshot {
                host: context.context.host,
                source: context.context.source,
                operation_id: OperationId::from_random([44; 16]),
            },
        },
    )
    .unwrap();
    assert_eq!(record.command.endpoint(), "/api/directory-snapshots");
    assert!(record.command.outcome_endpoint().is_none());
    let mut receipt = source();
    receipt.publication = record.command.operation();
    assert!(
        record
            .result(json_codec::to_value(receipt).unwrap())
            .is_ok()
    );
    // The reserve and independently generated completion are intentionally distinct.
    assert_ne!(source().publication, record.command.operation());
    assert!(
        record
            .result(json_codec::to_value(source()).unwrap())
            .is_ok()
    );
    assert!(requests::verify_initialization_head(source(), &head()).is_ok());
    let mut unrelated = source();
    unrelated.branch_id = SnapshotBranchId::from_random([99; 16]);
    assert!(requests::verify_initialization_head(unrelated, &head()).is_err());
}

#[test]
fn directory_snapshot_receipt_and_recovery_keep_exact_scope() {
    let record = record();
    let mut snapshot = source();
    snapshot.publication = record.command.operation();
    snapshot.node_id = SnapshotNodeId::from_random([55; 16]);
    let receipt = json!({
        "operation_id":record.command.operation(), "publication_sequence":"8",
        "commit_state":"PUBLISHED", "protection_at_publish":"TARGET_MET",
        "result":{"outcome":"changed","snapshot":snapshot},
    });
    assert!(record.result(receipt.clone()).is_ok());
    for (field, value) in [
        ("operation_id", json!(OperationId::from_random([99; 16]))),
        ("commit_state", json!("PREPARED")),
        ("result", json!({"outcome":"unchanged","snapshot":snapshot})),
    ] {
        let mut bad = receipt.clone();
        bad[field] = value;
        assert!(record.result(bad).is_err(), "{field}");
    }
    let endpoint = record.command.outcome_endpoint().unwrap();
    assert!(endpoint.contains(&format!("source_publication={}", source().publication)));
    assert!(endpoint.contains(&format!("node_id={}", source().node_id)));
    let frozen = record.encode().unwrap();
    assert!(Record::parse(&frozen, &"bob".parse().unwrap()).is_err());
    let duplicate = frozen.replacen(
        "{",
        "{\"format\":\"directory-snapshot-original-request\",",
        1,
    );
    assert!(Record::parse(&duplicate, &record.principal).is_err());
    let mut malformed: Value = json_codec::from_str(&frozen).unwrap();
    malformed["command"]["request"]["selected"][0]["target"] = json!(["x", "y"]);
    assert!(Record::parse(&malformed.to_string(), &record.principal).is_err());
    let mut duplicate = record.clone();
    let Command::Commit { request, .. } = &mut duplicate.command else {
        panic!("fixture must commit")
    };
    request.selected.push(request.selected[0].clone());
    assert!(Record::parse(&duplicate.encode().unwrap(), &record.principal).is_err());
}

#[test]
fn directory_snapshot_storage_keys_retain_independent_tab_originals() {
    let first = record();
    let mut second = record();
    let Command::Commit { request, .. } = &mut second.command else {
        panic!("fixture must commit")
    };
    request.operation_id = OperationId::from_random([77; 16]);
    let first_key = super::storage::key(&first);
    let second_key = super::storage::key(&second);
    assert_ne!(first_key, second_key);
    let mut browser = std::collections::BTreeMap::new();
    browser.insert(first_key.clone(), first.encode().unwrap());
    browser.insert(second_key.clone(), second.encode().unwrap());
    assert_eq!(
        browser.len(),
        2,
        "interleaved saves have distinct immutable keys"
    );
    for (key, bytes) in &browser {
        assert!(super::storage::decode(key, bytes, &first.principal).is_ok());
    }
    assert!(
        super::storage::decode(&second_key, &first.encode().unwrap(), &first.principal).is_err()
    );
    browser.remove(&first_key);
    assert_eq!(
        super::storage::decode(
            &second_key,
            browser.get(&second_key).unwrap(),
            &second.principal,
        )
        .unwrap(),
        second
    );
}

#[test]
fn directory_snapshot_registers_a_real_empty_disk_without_dummy_repository() {
    let mut state = state();
    state.files.repository_id.clear();
    state.files.namespace_head.clear();
    state.files.slot = "disk-a".into();
    assert!(Target::capture(&state).is_none());
    assert!(super::can_register(&state));
    state.files.query = "filtered".into();
    assert!(!super::can_register(&state));
    state.files.query.clear();
    let operation_id = OperationId::from_random([88; 16]);
    let record = Record::new(
        "alice".parse().unwrap(),
        Command::RegisterRoot {
            slot: "disk-a".parse().unwrap(),
            operation_id,
        },
    )
    .unwrap();
    assert_eq!(
        record.command.endpoint(),
        "/api/file-manager/disks/disk-a/snapshot-root"
    );
    assert_eq!(
        record.command.body().unwrap(),
        json!({"operation_id":operation_id})
    );
    assert!(
        record
            .result(json!({"slot_id":"disk-a","repository_id":repository()}))
            .is_ok()
    );
    assert!(
        record
            .result(json!({"slot_id":"disk-b","repository_id":repository()}))
            .is_err()
    );
    let bytes = record.encode().unwrap();
    assert!(
        !bytes.contains("repository"),
        "root registration has no synthetic repository"
    );
    assert_eq!(Record::parse(&bytes, &record.principal).unwrap(), record);
}
