use super::*;
use crate::history_package::{AdmittedState, RawState};
use json_codec::json;
use storage_contract::{CommitState, CurrentHealth, OperationId, ProtectionAtPublish};

fn sample(kind: &str, result: Value) -> OperationResult {
    OperationResult {
        operation_id: OperationId::from_random([33; 16]),
        kind: kind.into(), commit_id: None, history_tree_id: None, branch_id: None,
        commit_state: CommitState::Published, protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy, request_digest: "1".repeat(64), sequence: 1, result,
    }
}

#[test]
fn original_radix_writer_and_separate_decoded_state_both_refuse_mixed_receipts() {
    for original in [
        sample("snapshot.repository.reserve", json!({})),
        sample("snapshot.worktree.complete", json!({})),
        sample("snapshot.working.commit", json!({})),
        sample("directory.git.initialize", json!({})),
        sample("directory.namespace.batch", json!({})),
        sample("branch.create", json!({})),
        sample("history.historical_save", json!({})),
        sample("history.split", json!({})),
        sample("history.restore", json!({})),
        sample("workspace.commit", json!({"restored_path_audit":null})),
        sample("workspace.commit", json!({"snapshot_worktree":null})),
        sample("file.draft.commit", json!({"directory":{"format":"directory_draft_unchanged"}})),
        sample("file.branches.merge", json!({"directory_branch_ids":[]})),
        sample("repository.create", json!({"created_repository":{"file_manager_root":false}})),
        sample("repository.create", json!({"created_repository":{"file_manager_root":true}})),
    ] {
        let mut state = State::default();
        state.sequence = 1;
        state.operations.insert(original.operation_id, original.clone());
        assert_eq!(crate::journal::prepare_snapshot(&state, 1024 * 1024).err().unwrap().code(),
            ErrorCode::StateInvalid, "writer: {}", original.kind);
        // Distinct raw disk/portable decode; do not obtain a clean snapshot
        // by round-tripping through the writer that correctly rejected it.
        let raw: RawState = storage_contract::decode_strict(
            &storage_contract::canonical_json(&state).unwrap(),
        ).unwrap();
        assert_eq!(AdmittedState::try_from(raw).err().unwrap().code(),
            ErrorCode::StateInvalid, "reader: {}", original.kind);
        let mut failed = original;
        failed.commit_state = CommitState::PublishedDegraded;
        assert!(operation(&failed).is_err());
    }
}

#[test]
fn ordinary_directories_purge_and_user_metadata_are_not_unsupported_receipts() {
    for kind_name in ["directory.import.stop", "file.delete.batch", storage_contract::file_deletion_batch::UNDO_KIND,
        "file.namespace", "workspace.commit", "file.history.merge", "file.working.birth", "content.purge"]
    {
        let original = sample(kind_name, json!({"user_note":{"snapshot_worktree":"my text"},
            "name":"project.zxs", "directory":{"name":"regular folder"}}));
        operation(&original).unwrap();
        encoded_row("operations", &json_codec::to_value(&original).unwrap()).unwrap();
    }
    for retired in ["directory.delete", "directory.delete.undo"] {
        let marker = sample(retired, json!({"files":[]}));
        assert!(operation(&marker).is_err());
        assert!(encoded_row("operations", &json_codec::to_value(&marker).unwrap()).is_err());
    }
    let mut retained = State::default();
    retained.events.push(crate::Event {
        sequence: 1, operation_id: OperationId::from_random([0; 16]),
        kind: "snapshot.worktree.reserve".into(), object_id: "unbound".into(),
    });
    assert!(decoded_records(&retained).is_err());
    for kind_name in ["split", "historical_save", "unknown"] {
        assert!(plan(kind_name).is_err());
        assert!(encoded_row("plans", &json!({"kind":kind_name})).is_err());
    }
    assert!(plan("purge").is_ok());
    for invalid in [json!({}), json!({"file_manager_root":false}), json!({"file_manager_root":"true"})] {
        assert!(encoded_row("repositories", &invalid).is_err());
    }
    encoded_row("repositories", &json!({"file_manager_root":true})).unwrap();
    // A positive domain check is not full root creation admission.
    operation(&sample("repository.create", json!({
        "file_manager_root_origin":{}, "created_repository":{"file_manager_root":true},
    }))).unwrap();
}

// Each name is independently disallowed, including a valid empty-map
// reference and even without a mixed receipt or any non-empty history.
const NON_FILE_TABLES: [&str; 7] = [
    "file_write_origins", "directory_branches", "directory_git_repositories", "directory_trees",
    "directory_tree_selections", "directory_selections", "directory_selection_generations",
];

#[test]
fn fixed_snapshot_rejects_every_extra_old_map_before_loading_any_descendant() {
    let state = State::default();
    let clean = crate::journal::prepare_snapshot(&state, 1024 * 1024).unwrap().descriptor;
    crate::journal::validate_snapshot_schema(&clean).unwrap();
    for old_name in NON_FILE_TABLES {
        let mut foreign = clean.clone();
        assert!(!foreign.maps.contains_key(old_name));
        foreign.maps.insert(old_name.into(), clean.maps["namespace_directories"].clone());
        let error = crate::journal::resolve_admitted_snapshot(foreign,
            |_, _| panic!("incompatible fixed root must fail before reading child material"))
            .err().unwrap();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        let mut flat = json_codec::to_value(&state).unwrap();
        flat[old_name] = json!({});
        assert!(json_codec::from_value::<RawState>(flat).is_err(), "{old_name}");
    }
    let mut unsupported = state;
    unsupported.directory_selections.insert("retained".into(), "unknown".into());
    assert!(storage_contract::canonical_json(&unsupported).is_err());
    assert!(crate::journal::prepare_snapshot(&unsupported, 1024 * 1024).is_err());
}

#[test]
fn a_relabelled_or_ambiguous_local_plan_cannot_become_a_file_purge() {
    let history = storage_contract::new_id("history").unwrap();
    let v = storage_contract::new_id("ver").unwrap();
    let c = storage_contract::new_id("commit").unwrap();
    let input = json!({"history_tree_id":history,"version_ids":[v]});
    assert!(plan_input("purge", &input).is_ok());
    assert!(plan_input("split", &input).is_err());
    for bad in [
        json!({"history_tree_id":history,"cut_commit_id":c}),
        json!({"history_tree_id":history,"version_ids":[v],"cut_commit_id":c}),
        json!({"history_tree_id":history,"source_commit_id":c,"name":"copy"}),
        json!({"history_tree_id":history,"version_ids":[v,v]}),
        json!({"history_tree_id":history,"version_ids":[]}),
    ] {
        assert!(plan_input("purge", &bad).is_err());
        assert!(encoded_row("plans", &json!({"kind":"purge","input":bad})).is_err());
    }
}
