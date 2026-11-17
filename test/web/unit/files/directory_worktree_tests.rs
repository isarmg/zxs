use super::{Observed, Scope, can_commit, can_create, can_load, requests, smb_path, status};
use crate::files::directory_snapshot::{
    Model, Pending, Phase, Review, Target, blocks_writes, wire,
};
use crate::ui::State;
use json_codec::json;
use std::{collections::BTreeSet, num::NonZeroU32, sync::Arc};
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotHeadDiscovery,
};
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitOutcome, WorktreeCommitReceipt, WorktreeCreationOutcome, WorktreeCreationReceipt,
};
use storage_contract::snapshot_worktree::{
    SnapshotHandle, SnapshotHost, SnapshotNamespace, SnapshotRootContext, WorktreeHandle,
    WorktreeId, WorktreeRevision, WorktreeStatus,
};
use storage_contract::working_tree::{
    DirectoryId, RootId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{BranchId, CommitId, Generation, OperationId, RepositoryId};
use wire::{Command, LookupResult, Publication, Record};

fn repository() -> RepositoryId {
    RepositoryId::from_random([1; 16])
}

fn source() -> SnapshotHandle {
    SnapshotHandle {
        publication: OperationId::from_random([2; 16]),
        repository_id: SnapshotRepositoryId::from_random([3; 16]),
        branch_id: SnapshotBranchId::from_random([4; 16]),
        node_id: SnapshotNodeId::from_random([5; 16]),
    }
}

fn handle() -> WorktreeHandle {
    WorktreeHandle {
        publication: OperationId::from_random([20; 16]),
        worktree_id: WorktreeId::from_random([21; 16]),
    }
}

fn observed_status() -> WorktreeStatus {
    WorktreeStatus {
        target: handle(),
        original: source(),
        parent_directory_id: DirectoryId::from_random([7; 16]),
        name: name_policy::LookupName::parse("worktree-actual-001").unwrap(),
        revision: WorktreeRevision {
            generation: Generation::new(2),
            manifest: json_codec::from_value(json!("ab".repeat(32))).unwrap(),
        },
    }
}

fn state() -> State {
    let principal = "alice".parse().unwrap();
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        principal: Some(principal),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.repository_id = repository().to_string();
    state.files.namespace_head = CommitId::from_random([9; 16]).to_string();
    state.files.loaded = true;
    state.files.writable = true;
    state.files.slot = "D01".into();
    state.files.disks.push(crate::files::model::Disk {
        name: "真实磁盘名".into(),
        slot_id: "D01".into(),
        level: NonZeroU32::new(1).unwrap(),
        online: true,
        writable: true,
        available_bytes: None,
        observation_failure: None,
    });
    state.files.directory_snapshot = Model {
        owner: Some((7, state.principal.clone().unwrap())),
        storage_ready: true,
        ..Default::default()
    };
    state.files.directory_snapshot.review = Some(Review {
        target: Target::capture(&state).unwrap(),
        head: DirectorySnapshotHead {
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
                branch_name: storage_contract::BranchName::parse("actual").unwrap(),
            },
        },
        candidates: Vec::new(),
        selected: BTreeSet::new(),
    });
    let model = &mut state.files.directory_snapshot.worktree;
    model.name = "worktree-actual-001".into();
    model.handle_text = handle().worktree_id.to_string();
    model.publication_text = handle().publication.to_string();
    model.message = "explicit worktree save".into();
    let scope = Scope::capture(&state).unwrap();
    state.files.directory_snapshot.worktree.observed = Some(Observed {
        scope,
        status: observed_status(),
    });
    state
}

fn creation(state: &State) -> Record {
    Record::new(
        state.principal.clone().unwrap(),
        requests::create_command(state, OperationId::from_random([30; 16])).unwrap(),
    )
    .unwrap()
}

fn commit(state: &State) -> Record {
    Record::new(
        state.principal.clone().unwrap(),
        requests::commit_command(state, OperationId::from_random([31; 16])).unwrap(),
    )
    .unwrap()
}

#[test]
fn worktree_ui_captures_actual_head_and_latest_exact_status_without_legacy_ids() {
    let mut state = state();
    assert!(can_create(&state));
    assert!(can_load(&state));
    assert!(can_commit(&state));
    let create = creation(&state);
    let body = create.command.body().unwrap();
    assert_eq!(create.command.endpoint(), "/api/directory-worktrees");
    assert_eq!(body["source"], json_codec::to_value(source()).unwrap());
    assert_eq!(
        body["parent_directory_id"],
        json!(DirectoryId::from_random([7; 16]))
    );
    assert_eq!(body["expected_root_generation"], "3");
    assert_eq!(body["name"], "worktree-actual-001");
    assert_ne!(create.command.operation(), handle().publication);
    assert!(
        matches!(create.result(json_codec::to_value(handle()).unwrap()).unwrap(),
        Publication::WorktreeCreated { target, .. } if target == handle())
    );
    let model = &mut state.files.directory_snapshot.worktree;
    model.observed.as_mut().unwrap().status.revision.generation = Generation::new(5);
    let commit = commit(&state);
    let body = commit.command.body().unwrap();
    assert_eq!(
        commit.command.endpoint(),
        format!("/api/directory-worktrees/{}/commits", handle().worktree_id)
    );
    assert_eq!(body["target"], json_codec::to_value(handle()).unwrap());
    assert_eq!(body["expected"]["generation"], "5");
    assert_eq!(body["message"], "explicit worktree save");
}

#[test]
fn worktree_ui_status_binds_exact_handle_source_parent_before_showing_real_smb_path() {
    let state = state();
    let scope = Scope::capture(&state).unwrap();
    let record = creation(&state);
    let Command::CreateWorktree { request, .. } = record.command else {
        panic!("create");
    };
    let value = json_codec::to_value(observed_status()).unwrap();
    assert!(status(value.clone(), &scope, handle(), Some(&request)).is_ok());
    assert_eq!(smb_path(&state).unwrap(), "真实磁盘名/worktree-actual-001");
    for (field, changed) in [
        (
            "target",
            json!(WorktreeHandle {
                publication: OperationId::from_random([99; 16]),
                ..handle()
            }),
        ),
        (
            "original",
            json!(SnapshotHandle {
                repository_id: SnapshotRepositoryId::from_random([99; 16]),
                ..source()
            }),
        ),
        (
            "parent_directory_id",
            json!(DirectoryId::from_random([99; 16])),
        ),
    ] {
        let mut wrong = value.clone();
        wrong[field] = changed;
        assert!(status(wrong, &scope, handle(), None).is_err(), "{field}");
    }
    let mut wrong = value;
    wrong["name"] = json!("another-instance");
    assert!(status(wrong, &scope, handle(), Some(&request)).is_err());
}

#[test]
fn worktree_ui_creation_lookup_requires_original_request_and_independent_completion() {
    let state = state();
    let record = creation(&state);
    let Command::CreateWorktree { request, .. } = record.command.clone() else {
        panic!("create");
    };
    let pending = WorktreeCreationReceipt {
        repository_id: repository(),
        request,
        result: WorktreeCreationOutcome::Pending {},
    };
    assert!(matches!(
        record
            .lookup_result(json_codec::to_value(&pending).unwrap())
            .unwrap(),
        LookupResult::Pending
    ));
    let complete = WorktreeCreationReceipt {
        result: WorktreeCreationOutcome::Complete { target: handle() },
        ..pending.clone()
    };
    assert!(
        matches!(record.lookup_result(json_codec::to_value(&complete).unwrap()).unwrap(),
        LookupResult::Complete(Publication::WorktreeCreated { target, .. }) if target == handle())
    );
    let mut wrong = complete;
    wrong.request.name = name_policy::Name::parse("other-name").unwrap();
    assert!(
        record
            .lookup_result(json_codec::to_value(wrong).unwrap())
            .is_err()
    );
    assert_eq!(
        record.command.outcome_endpoint().unwrap(),
        format!(
            "/api/repositories/{}/directory-worktrees/creations/{}",
            repository(),
            record.command.operation(),
        )
    );
}

#[test]
fn worktree_ui_commit_lookup_never_confuses_unchanged_revision_with_published_node() {
    let state = state();
    let record = commit(&state);
    let Command::CommitWorktree { request, .. } = record.command.clone() else {
        panic!("commit");
    };
    let pending = WorktreeCommitReceipt {
        repository_id: repository(),
        request,
        result: WorktreeCommitOutcome::Pending {},
    };
    assert!(matches!(
        record
            .lookup_result(json_codec::to_value(&pending).unwrap())
            .unwrap(),
        LookupResult::Pending
    ));
    let snapshot = SnapshotHandle {
        publication: OperationId::from_random([50; 16]),
        node_id: SnapshotNodeId::from_random([51; 16]),
        ..source()
    };
    assert_ne!(snapshot.publication, record.command.operation());
    let complete = WorktreeCommitReceipt {
        result: WorktreeCommitOutcome::Complete { snapshot },
        ..pending.clone()
    };
    assert!(
        matches!(record.lookup_result(json_codec::to_value(&complete).unwrap()).unwrap(),
        LookupResult::Complete(Publication::WorktreeCommitted { snapshot: result, .. })
            if result == snapshot)
    );
    assert!(
        record
            .result(json_codec::to_value(source()).unwrap())
            .is_err()
    );
    let mut wrong = complete;
    wrong.request.expected.generation = Generation::new(77);
    assert!(
        record
            .lookup_result(json_codec::to_value(wrong).unwrap())
            .is_err()
    );
    assert_eq!(
        record.command.outcome_endpoint().unwrap(),
        format!(
            "/api/repositories/{}/directory-worktrees/commits/{}",
            repository(),
            record.command.operation(),
        )
    );
}

#[test]
fn worktree_ui_reuses_original_request_journal_and_gates_old_scope_or_new_head() {
    let mut state = state();
    let record = Arc::new(commit(&state));
    let frozen = record.encode().unwrap();
    assert_eq!(Record::parse(&frozen, &record.principal).unwrap(), *record);
    assert!(Record::parse(&frozen, &"bob".parse().unwrap()).is_err());
    let key = crate::files::directory_snapshot::storage::key(&record);
    assert!(
        crate::files::directory_snapshot::storage::decode(&key, &frozen, &record.principal,)
            .is_ok()
    );
    state.files.directory_snapshot.pending = Some(Pending {
        record: Arc::clone(&record),
        phase: Phase::Pending,
    });
    state.files.directory_snapshot.worktree.message = "changed after submit".into();
    assert!(blocks_writes(&state));
    assert!(!can_create(&state));
    assert!(!can_commit(&state));
    assert_eq!(
        crate::files::directory_snapshot::requests::retry_record(&state)
            .unwrap()
            .encode()
            .unwrap(),
        frozen
    );
    state.files.directory_snapshot.pending = None;
    state
        .files
        .directory_snapshot
        .review
        .as_mut()
        .unwrap()
        .head
        .head = SnapshotHeadDiscovery::Head {
        snapshot: SnapshotHandle {
            node_id: SnapshotNodeId::from_random([90; 16]),
            ..source()
        },
        branch_name: storage_contract::BranchName::parse("actual").unwrap(),
    };
    assert!(
        !can_commit(&state),
        "this slice does not fork from an old source"
    );
    state.session_epoch += 1;
    assert!(!can_load(&state));
    assert!(smb_path(&state).is_none());
}
