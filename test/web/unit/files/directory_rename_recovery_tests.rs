use super::*;
use crate::files::model::{DirectoryPreview, DirectoryPreviewSource};

fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}

fn state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        principal: Some("alice".parse().unwrap()),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.loaded = true;
    state.files.writable = true;
    state.files.directory_branches_supported = true;
    state.files.slot = "P01".into();
    state.files.directory = "named/folder".into();
    state.files.repository_path = "folder".into();
    state.files.repository_id = id("repo", 1);
    state.files.directory_branches = vec![crate::files::test_entries::branch(json!({
        "directory_branch_id":id("dbr", 3), "repository_id":id("repo", 1),
        "head_tree_id":id("dtree", 4), "path":"folder", "current":true,
    }))];
    state.files.mkdir.owner = Some((7, "alice".parse().unwrap()));
    state
}

fn capture(state: &State, name: &str) -> Result<Record, api::Error> {
    Record::capture_directory_rename(
        state,
        &"alice".parse().unwrap(),
        id("op", 6).parse().unwrap(),
        &id("repo", 1),
        &id("dbr", 3),
        name,
    )
}

fn record() -> Record {
    capture(&state(), "Café").unwrap()
}

fn receipt(record: &Record) -> Value {
    json!({
        "operation_id":record.operation(), "operation_kind":"directory.branch.rename",
        "request_digest":record.digest, "state":"completed",
        "commit_state":storage_contract::CommitState::Published,
        "error":null, "protection_at_publish":storage_contract::ProtectionAtPublish::NotApplicable,
        "publication_sequence":"19",
        "result":{
            "directory_branch_id":id("dbr", 3), "name":"Café",
            "path":"folder", "head_tree_id":id("dtree", 4),
        },
    })
}

const ORIGINAL: &str = concat!(
    "{\"format\":\"directory-branch-original-request\",",
    "\"namespace\":{\"kind\":\"directory_branch\",\"request\":{",
    "\"action\":\"rename\",",
    "\"directory_branch_id\":\"dbr_00000000000000000000000000000003\",",
    "\"name\":\"Café\",\"repository_id\":\"repo_00000000000000000000000000000001\"}},",
    "\"operation\":\"op_00000000000000000000000000000006\",",
    "\"path\":\"named/folder\",\"principal\":\"alice\",\"slot\":\"P01\"}",
);

#[test]
fn rename_round_trip_retains_exact_patch_body_bytes_and_server_digest() {
    let record = record();
    assert_eq!(record.bytes.as_ref(), ORIGINAL);
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), ORIGINAL).unwrap();
    assert_eq!(restored.request(), record.request());
    assert_eq!(
        storage_contract::canonical_json(&restored.stored).unwrap(),
        ORIGINAL.as_bytes()
    );
    let (method, url, body) = restored.request();
    assert_eq!(method, "PATCH");
    assert_eq!(
        url,
        format!(
            "/api/repositories/{}/directory-branches/{}",
            id("repo", 1),
            id("dbr", 3)
        ),
    );
    assert_eq!(
        json_codec::to_string(&body).unwrap(),
        format!(r#"{{"name":"Café","operation_id":"{}"}}"#, id("op", 6)),
    );
    let expected = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"directory.branch.rename", "principal":"alice",
            "body":{
                "repository_id":id("repo", 1), "directory_branch_id":id("dbr", 3), "name":"Café",
            },
        }),
    )
    .unwrap();
    assert_eq!(restored.digest.as_deref(), Some(expected.as_str()));
    assert!(restored.can_lookup());
    assert!(!restored.is_directory_checkout());
    assert!(restored.label().contains("Café"));
    assert!(restored.label().contains(&id("dbr", 3)));
}

#[test]
fn rename_capture_keeps_the_explicit_noncurrent_row_and_preserves_root_and_lookup_paths() {
    let mut current = state();
    // History rows are separately paged. The edited branch need not appear in
    // the working listing, and must never be replaced by its current branch.
    current.files.directory_branches = vec![crate::files::test_entries::branch(json!({
        "directory_branch_id":id("dbr", 8), "repository_id":id("repo", 1),
        "path":"folder", "current":true,
    }))];
    assert_eq!(
        capture(&current, "Café").unwrap().request(),
        record().request()
    );
    current.files.directory_branches.clear();
    assert_eq!(
        capture(&current, "Café").unwrap().bytes.as_ref(),
        ORIGINAL
    );
    current.files.repository_id = id("repo", 2);
    assert!(capture(&current, "Café").is_err());
    for (repository, branch) in [
        (id("repo", 2), id("dbr", 3)),
        (id("repo", 1), "invalid".into()),
    ] {
        assert!(
            Record::capture_directory_rename(
                &state(),
                &"alice".parse().unwrap(),
                id("op", 6).parse().unwrap(),
                &repository,
                &branch,
                "Café",
            )
            .is_err()
        );
    }
    for (display, path) in [("", ""), ("named", ""), ("named/FOLDER", "FOLDER")] {
        let mut state = state();
        state.files.directory = display.into();
        state.files.repository_path = path.into();
        let original = capture(&state, "Café").unwrap();
        assert_eq!(original.request(), record().request());
        let restored =
            Record::decode(&original.key(), &"alice".parse().unwrap(), &original.bytes).unwrap();
        assert_eq!(restored.request(), original.request());
        assert_eq!(restored.path(), display);
    }
    for name in ["", "bad/name", "Cafe\u{301}", &"文".repeat(43)] {
        assert!(capture(&state(), name).is_err());
    }
}

#[test]
fn rename_restore_rejects_missing_malformed_or_changed_binding_and_keeps_budgets() {
    let record = record();
    for field in ["repository_id", "directory_branch_id", "name"] {
        for value in [Value::Null, json!(false), json!("")] {
            let mut changed: Value = json_codec::from_str(ORIGINAL).unwrap();
            changed["namespace"]["request"][field] = value;
            assert!(
                Record::decode(
                    &record.key(),
                    &"alice".parse().unwrap(),
                    &changed.to_string(),
                )
                .is_err(),
                "{field}",
            );
        }
        let mut changed: Value = json_codec::from_str(ORIGINAL).unwrap();
        changed["namespace"]["request"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            Record::decode(
                &record.key(),
                &"alice".parse().unwrap(),
                &changed.to_string(),
            )
            .is_err(),
        );
    }
    for change in 0..4 {
        let mut changed: Value = json_codec::from_str(ORIGINAL).unwrap();
        match change {
            0 => changed["operation"] = id("op", 99).into(),
            1 => changed["namespace"]["request"]["method"] = "POST".into(),
            2 => changed["namespace"]["request"]["name"] = "Cafe\u{301}".into(),
            3 => changed["format"] = "folder-create-original-request".into(),
            _ => unreachable!(),
        }
        assert!(
            Record::decode(
                &record.key(),
                &"alice".parse().unwrap(),
                &changed.to_string(),
            )
            .is_err(),
            "{change}",
        );
    }
    assert!(Record::decode(&record.key(), &"bob".parse().unwrap(), ORIGINAL).is_err());
    let mut bytes = ORIGINAL.to_owned();
    bytes.push_str(&" ".repeat(MAX_RECORD_BYTES - bytes.len()));
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).unwrap();
    assert_eq!(restored.bytes.as_ref(), bytes);
    assert_eq!(restored.request(), record.request());
    bytes.push(' ');
    assert_eq!(
        Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes)
            .err()
            .unwrap()
            .code,
        ErrorCode::ResourceLimit,
    );
    assert_eq!(record.bytes.as_ref(), ORIGINAL);
}

#[test]
fn rename_receipt_binds_repository_digest_branch_name_and_complete_existing_result() {
    let record = record();
    let valid = receipt(&record);
    assert!(valid["result"].get("input").is_none());
    for origin in [ReceiptOrigin::Lookup, ReceiptOrigin::OriginalRequest] {
        assert!(record.matches_receipt(&valid, origin));
        for change in 0..8 {
            let mut wrong = valid.clone();
            match change {
                0 => wrong["operation_kind"] = "directory.git.checkout".into(),
                1 => wrong["request_digest"] = "other".into(),
                2 => wrong["result"]["directory_branch_id"] = id("dbr", 9).into(),
                3 => wrong["result"]["name"] = "Another".into(),
                4 => wrong["result"]["head_tree_id"] = id("commit", 4).into(),
                5 => wrong["result"]["path"] = "../outside".into(),
                6 => wrong["result"]["input"] = json!({}),
                7 => wrong["result"]["name"] = "Cafe\u{301}".into(),
                _ => unreachable!(),
            }
            assert!(!record.matches_receipt(&wrong, origin), "{change}");
        }
        for field in ["directory_branch_id", "name", "head_tree_id", "path"] {
            let mut missing = valid.clone();
            missing["result"].as_object_mut().unwrap().remove(field);
            assert!(!record.matches_receipt(&missing, origin), "{field}");
        }
    }
    let mut other: Value = json_codec::from_str(ORIGINAL).unwrap();
    other["namespace"]["request"]["repository_id"] = id("repo", 2).into();
    let other = Record::decode(
        &record.key(),
        &"alice".parse().unwrap(),
        &other.to_string(),
    )
    .unwrap();
    assert_ne!(other.digest, record.digest);
    assert!(!other.matches_receipt(&valid, ReceiptOrigin::Lookup));

    // Label edits have no reviewed-tree CAS. A legitimate original completion
    // can report a later current tree or a renamed path without becoming a retry.
    let mut advanced = valid;
    advanced["result"]["path"] = "renamed-folder".into();
    advanced["result"]["head_tree_id"] = id("dtree", 8).into();
    assert!(record.matches_receipt(&advanced, ReceiptOrigin::Lookup));
}

#[test]
fn rename_navigation_unknown_outcome_and_relogin_never_change_the_original_request() {
    let record = Arc::new(record());
    let operation = record.operation();
    let mut state = state();
    state
        .files
        .mkdir
        .restore_records(Ok(vec![Arc::clone(&record)]));
    let original = record.request();
    state.files.directory = "another-folder".into();
    state.files.repository_id = id("repo", 99);
    state.files.generation += 1;
    assert!(blocks_writes(&state));
    assert!(!state.files.mkdir.ready(&state));
    let retry = retry_record(&state, 7, operation).unwrap();
    assert_eq!(retry.request(), original);
    assert_eq!(retry.bytes.as_ref(), ORIGINAL);
    assert!(poll_allowed(&state, &operation.to_string()));
    settle(
        &mut state.files.mkdir,
        &operation.to_string(),
        SaveStatus::Unknown,
    );
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Pending));
    state.files.mkdir.records[0].phase = Phase::Sending;
    assert!(retry_record(&state, 7, operation).is_none());
    assert!(!poll_allowed(&state, &operation.to_string()));

    state.session_epoch = 8;
    adopt_owner(&mut state, &(8, "alice".parse().unwrap()));
    assert!(retry_record(&state, 7, operation).is_none());
    assert_eq!(
        retry_record(&state, 8, operation).unwrap().request(),
        original
    );
    state.principal = Some("bob".parse().unwrap());
    state.session_epoch = 9;
    adopt_owner(&mut state, &(9, "bob".parse().unwrap()));
    assert!(state.files.mkdir.records.is_empty());
    assert_eq!(record.bytes.as_ref(), ORIGINAL);
    assert_eq!(record.request(), original);
}

#[test]
fn rename_cleanup_failure_preserves_the_fence_without_checkout_or_resubmission() {
    let record = Arc::new(record());
    let operation = record.operation().to_string();
    let mut state = state();
    state
        .files
        .mkdir
        .restore_records(Ok(vec![Arc::clone(&record)]));
    state.files.directory_preview = Some(DirectoryPreview {
        source: DirectoryPreviewSource::Branch(state.files.directory_branches[0].clone()),
        path: state.files.directory.clone(),
    });
    assert!(!default_completion_applies(&state, &operation));
    assert!(state.files.default_operation.is_none());
    let status = FileOperation::from_receipt(&operation, &receipt(&record)).status();
    assert_eq!(status, SaveStatus::Published);
    settle(&mut state.files.mkdir, &operation, status);
    assert!(!complete_cleanup(
        &mut state.files.mkdir,
        &record,
        Err(failure("local removal failed")),
    ));
    assert!(blocks_writes(&state));
    assert!(retry_record(&state, 7, record.operation()).is_none());
    assert_eq!(state.files.mkdir.records[0].record.bytes.as_ref(), ORIGINAL);
    assert!(state.files.directory_preview.is_some());
    assert!(complete_cleanup(&mut state.files.mkdir, &record, Ok(())));
    assert!(blocks_writes(&state));
    state.files.mkdir.restore_records(Ok(Vec::new()));
    assert!(!blocks_writes(&state));
    assert!(state.files.directory_preview.is_some());
    assert!(state.files.operations.is_empty());
}

#[test]
fn rename_original_operation_and_local_conflicts_cannot_be_replaced_by_a_late_receipt() {
    let record = Arc::new(record());
    let operation = record.operation().to_string();
    let mut published = receipt(&record);
    published["operation_id"] = id("op", 99).into();
    let outcome = FileOperation::from_receipt(&operation, &published);
    assert_eq!(outcome.status(), SaveStatus::Unknown);
    let mut state = state();
    state
        .files
        .mkdir
        .restore_records(Ok(vec![Arc::clone(&record)]));
    settle(&mut state.files.mkdir, &operation, outcome.status());
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Pending));
    let changed = capture(&state, "Another").unwrap();
    state.files.mkdir.restore_records(Ok(vec![Arc::new(changed)]));
    assert!(state.files.mkdir.error.is_some());
    assert!(blocks_writes(&state));
    assert_eq!(state.files.mkdir.records[0].record.bytes.as_ref(), ORIGINAL);
    assert!(retry_record(&state, 7, record.operation()).is_none());
}

#[test]
fn rename_action_rejects_old_identity_and_write_fences_before_browser_work() {
    for change in 0..11 {
        Owner::new().with(|| {
            let mut initial = state();
            match change {
                0 => initial.authenticated = false,
                1 => initial.session_transition = crate::ui::SessionTransition::LoggingOut,
                2 => initial.files.loading = true,
                3 => initial.files.loaded = false,
                4 => initial.files.writable = false,
                5 => initial.files.directory_branches_supported = false,
                6 => initial.files.mkdir.error = Some(failure("storage write did not verify")),
                7 => {
                    initial.files.directory_preview = Some(DirectoryPreview {
                        source: DirectoryPreviewSource::Branch(
                            initial.files.directory_branches[0].clone(),
                        ),
                        path: initial.files.directory.clone(),
                    });
                }
                8 => initial.files.repository_id = id("repo", 99),
                9 => initial.management.storage = crate::management::Storage::RecoveryRequired,
                10 => initial
                    .files
                    .mkdir
                    .restore_records(Ok(vec![Arc::new(record())])),
                _ => unreachable!(),
            }
            let state = RwSignal::new(initial);
            assert!(
                crate::files::actions::rename_directory_branch(
                    state,
                    id("repo", 1),
                    id("dbr", 3),
                    "Café".into(),
                )
                .is_none(),
                "{change}",
            );
            assert!(!state.with_untracked(|s| s.files.mutation_pending));
            assert!(state.with_untracked(|s| s.files.operations.is_empty()));
        });
    }
    Owner::new().with(|| {
        let state = RwSignal::new(state());
        assert!(
            crate::files::actions::rename_directory_branch(
                state,
                id("repo", 1),
                String::new(),
                "Café".into(),
            )
            .is_none(),
        );
        assert!(state.with_untracked(|s| s.files.operations.is_empty()));
    });
}
