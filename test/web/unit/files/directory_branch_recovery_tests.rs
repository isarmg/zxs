use super::*;

fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn source() -> storage_contract::directory_git::DirectoryBranchCreateSource {
    storage_contract::directory_git::DirectoryBranchCreateSource {
        repository_id: id("repo", 1),
        directory_id: id("entry", 2),
        directory_branch_id: id("dbr", 3),
        working_tree_id: id("dtree", 4),
        head_tree_id: id("dtree", 5),
        index_generation: 9_007_199_254_740_993,
        namespace_head: id("commit", 6),
    }
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
    state.files.slot = "P01".into();
    state.files.directory = "named/folder".into();
    state.files.repository_path = "folder".into();
    state.files.repository_id = id("repo", 1);
    state.files.namespace_head = id("commit", 6);
    state
}
fn create() -> Record {
    Record::capture_directory_branch(
        &state(),
        &"alice".parse().unwrap(),
        id("op", 1).parse().unwrap(),
        source(),
        "draft".into(),
    )
    .unwrap()
}
fn historical_source() -> storage_contract::directory_git::HistoricalDirectoryBranchSource {
    storage_contract::directory_git::HistoricalDirectoryBranchSource::new(
        source(),
        id("dtree", 7).parse().unwrap(),
    )
    .unwrap()
}
fn historical_create() -> Record {
    Record::capture_historical_directory_branch(
        &state(),
        &"alice".parse().unwrap(),
        id("op", 5).parse().unwrap(),
        historical_source(),
        "history-draft".into(),
    )
    .unwrap()
}
fn checkout() -> Record {
    let stored = Stored {
        format: Format::DirectoryBranch,
        principal: "alice".parse().unwrap(),
        operation: id("op", 2).parse().unwrap(),
        slot: "P01".into(),
        path: Some(name_policy::PortablePath::parse("named/folder").unwrap()),
        namespace: Namespace::DirectoryBranch {
            request: DirectoryRequest::Checkout {
                repository_id: id("repo", 1),
                directory_branch_id: id("dbr", 9),
                expected_tree_id: id("dtree", 8),
                expected_head: id("commit", 6),
            },
        },
    };
    let bytes = String::from_utf8(storage_contract::canonical_json(&stored).unwrap()).unwrap();
    Record::bind(stored, Arc::from(bytes)).unwrap()
}
fn receipt(record: &Record) -> Value {
    let Namespace::DirectoryBranch { request } = &record.stored.namespace else {
        panic!("directory request");
    };
    let (kind, input, result) = match request {
        DirectoryRequest::Create {
            source,
            repository_path,
            name,
        } => {
            let input = json!({
                "repository_id":source.repository_id, "path":repository_path,
                "name":name, "source":source,
            });
            (
                "directory.git.branch-create",
                input,
                json!({
                    "directory_branch_id":id("dbr",10), "head_tree_id":source.working_tree_id,
                    "path":repository_path, "name":name, "selected":false, "materialized":false,
                }),
            )
        }
        DirectoryRequest::CreateHistorical {
            source,
            repository_path,
            name,
        } => {
            let path = repository_path
                .as_ref()
                .map_or("", name_policy::PortablePath::as_str);
            let input = json!({
                "repository_id":source.current().repository_id, "path":path,
                "name":name.as_str(), "source":source,
            });
            (
                "directory.git.branch-create-history",
                input,
                json!({
                    "directory_branch_id":id("dbr",10), "head_tree_id":source.tree_id(),
                    "path":path, "name":name.as_str(), "selected":false, "materialized":false,
                    "copied_files":"0", "placeholder_files":"0",
                }),
            )
        }
        DirectoryRequest::Checkout {
            repository_id,
            directory_branch_id,
            expected_tree_id,
            expected_head,
        } => {
            let input = json!({
                "repository_id":repository_id, "directory_branch_id":directory_branch_id,
                "expected_tree_id":expected_tree_id, "expected_head":expected_head,
            });
            (
                "directory.git.checkout",
                input,
                json!({
                    "directory_branch_id":directory_branch_id, "commit_id":id("commit",10),
                    "selected":true, "no_change":false,
                }),
            )
        }
    };
    let mut result = result;
    result["input"] = input;
    json!({
        "operation_id":record.operation(), "operation_kind":kind,
        "request_digest":record.digest, "result":result,
    })
}

#[test]
fn create_round_trip_retains_exact_post_url_body_operation_and_all_source_fields() {
    let record = create();
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), &record.bytes).unwrap();
    assert_eq!(record.request(), restored.request());
    assert_eq!(record.bytes, restored.bytes);
    let (method, url, body) = restored.request();
    assert_eq!(method, "POST");
    assert_eq!(url, "/api/file-manager/disks/P01/directory-branches");
    assert_eq!(body["source"], json!(source()));
    assert_eq!(body["source"]["index_generation"], "9007199254740993");
    assert_eq!(body["operation_id"], id("op", 1));
    assert_eq!(body["path"], "named/folder");
    assert_eq!(body["name"], "draft");
    let expected = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"directory.git.branch-create", "principal":"alice",
            "body":{"repository_id":id("repo",1),"path":"folder","name":"draft","source":source()},
        }),
    )
    .unwrap();
    assert_eq!(restored.digest.as_deref(), Some(expected.as_str()));
}

#[test]
fn checkout_round_trip_keeps_put_and_the_original_branch_basis() {
    let record = checkout();
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), &record.bytes).unwrap();
    assert_eq!(record.request(), restored.request());
    let (method, url, body) = restored.request();
    assert_eq!(method, "PUT");
    assert_eq!(
        url,
        format!("/api/repositories/{}/default-branch", id("repo", 1))
    );
    assert_eq!(
        body,
        json!({
            "operation_id":id("op",2), "directory_branch_id":id("dbr",9),
            "expected_tree_id":id("dtree",8), "expected_head":id("commit",6),
        })
    );
    assert!(!url.contains("directory-select"));
    let expected = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"directory.git.checkout", "principal":"alice",
            "body":{"repository_id":id("repo",1),"directory_branch_id":id("dbr",9),
                "expected_tree_id":id("dtree",8),"expected_head":id("commit",6)},
        }),
    )
    .unwrap();
    assert_eq!(restored.digest.as_deref(), Some(expected.as_str()));
}

#[test]
fn root_and_named_root_are_explicit_and_never_guess_a_directory_identity() {
    for display in ["", "named"] {
        let mut state = state();
        state.files.directory = display.into();
        state.files.repository_path.clear();
        let mut source = source();
        source.directory_id = source.repository_id.clone();
        let record = Record::capture_directory_branch(
            &state,
            &"alice".parse().unwrap(),
            id("op", 1).parse().unwrap(),
            source,
            "root-draft".into(),
        )
        .unwrap();
        let restored =
            Record::decode(&record.key(), &"alice".parse().unwrap(), &record.bytes).unwrap();
        assert_eq!(restored.request(), record.request());
        assert_eq!(restored.request().2["path"], display);
    }
    let mut state = state();
    state.files.repository_path = "elsewhere".into();
    assert!(
        Record::capture_directory_branch(
            &state,
            &"alice".parse().unwrap(),
            id("op", 1).parse().unwrap(),
            source(),
            "draft".into(),
        )
        .is_err()
    );
}

#[test]
fn malformed_source_and_recovery_records_are_rejected_without_changing_bytes() {
    let record = create();
    let original = record.bytes.clone();
    for field in [
        "repository_id",
        "directory_id",
        "directory_branch_id",
        "working_tree_id",
        "head_tree_id",
        "index_generation",
        "namespace_head",
    ] {
        let mut value: Value = json_codec::from_str(&original).unwrap();
        value["namespace"]["request"]["source"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let bytes = json_codec::to_string(&value).unwrap();
        assert!(
            Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).is_err(),
            "{field}"
        );
    }
    for change in 0..6 {
        let mut value: Value = json_codec::from_str(&original).unwrap();
        match change {
            0 => {
                value.as_object_mut().unwrap().remove("path");
            }
            1 => value["operation"] = id("op", 99).into(),
            2 => value["namespace"]["request"]["source"]["index_generation"] = "0".into(),
            3 => value["namespace"]["request"]["source"]["index_generation"] = 2.into(),
            4 => value["namespace"]["request"]["method"] = "GET".into(),
            5 => value["format"] = "folder-create-original-request".into(),
            _ => unreachable!(),
        }
        let bytes = json_codec::to_string(&value).unwrap();
        assert!(
            Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).is_err(),
            "{change}"
        );
    }
    assert!(Record::decode(&record.key(), &"bob".parse().unwrap(), &record.bytes).is_err());
    assert_eq!(record.bytes, original);
}

#[test]
fn receipt_needs_original_digest_kind_input_and_result_for_both_commands() {
    for record in [create(), checkout(), historical_create()] {
        let valid = receipt(&record);
        assert!(record.matches_receipt(&valid, ReceiptOrigin::Lookup));
        assert!(record.matches_receipt(&valid, ReceiptOrigin::OriginalRequest));
        for change in 0..4 {
            let mut wrong = valid.clone();
            match change {
                0 => wrong["operation_kind"] = "file.namespace".into(),
                1 => wrong["request_digest"] = "changed".into(),
                2 => wrong["result"]["input"]["repository_id"] = id("repo", 2).into(),
                3 => wrong["result"]["directory_branch_id"] = "invalid".into(),
                _ => unreachable!(),
            }
            assert!(
                !record.matches_receipt(&wrong, ReceiptOrigin::Lookup),
                "{change}"
            );
        }
    }
    let record = checkout();
    let mut no_change = receipt(&record);
    no_change["result"]["no_change"] = true.into();
    no_change["result"]
        .as_object_mut()
        .unwrap()
        .remove("selected");
    assert!(record.matches_receipt(&no_change, ReceiptOrigin::Lookup));
}

#[test]
fn navigation_retry_uses_the_retained_request_and_is_fenced_by_owner_and_phase() {
    for record in [create(), checkout(), historical_create()] {
        let record = Arc::new(record);
        let operation = record.operation();
        let original = record.request();
        let mut state = state();
        state.files.mkdir.owner = Some((state.session_epoch, state.principal.clone().unwrap()));
        state.files.mkdir.records.push(Retained {
            record: Arc::clone(&record),
            phase: Phase::Pending,
        });
        state.files.directory = "elsewhere".into();
        state.files.repository_id = id("repo", 99);
        state.files.namespace_head = id("commit", 99);
        let retry = retry_record(&state, 7, operation).unwrap();
        assert!(Arc::ptr_eq(&retry, &record));
        assert_eq!(retry.request(), original);
        assert!(blocks_writes(&state));
        assert!(!default_completion_applies(&state, &operation.to_string()));
        assert!(retry_record(&state, 8, operation).is_none());
        state.files.mkdir.records[0].phase = Phase::Sending;
        assert!(retry_record(&state, 7, operation).is_none());
        assert!(!poll_allowed(&state, &operation.to_string()));
        state.files.mkdir.records[0].phase = Phase::Settled;
        assert!(retry_record(&state, 7, operation).is_none());
        state.files.mkdir.records[0].phase = Phase::Pending;
        state.principal = Some("bob".parse().unwrap());
        assert!(retry_record(&state, 7, operation).is_none());
    }
}

#[test]
fn unknown_results_keep_material_and_account_changes_do_not_adopt_it() {
    let record = Arc::new(create());
    let operation = record.operation();
    let mut state = state();
    state.files.mkdir.owner = Some((7, "alice".parse().unwrap()));
    state.files.mkdir.records.push(Retained {
        record: Arc::clone(&record),
        phase: Phase::Pending,
    });
    settle(
        &mut state.files.mkdir,
        &operation.to_string(),
        SaveStatus::Unknown,
    );
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Pending));
    assert_eq!(state.files.mkdir.records[0].record.bytes, record.bytes);
    state.session_epoch = 8;
    adopt_owner(&mut state, &(8, "alice".parse().unwrap()));
    assert_eq!(state.files.mkdir.records.len(), 1);
    assert!(retry_record(&state, 8, operation).is_some());
    state.principal = Some("bob".parse().unwrap());
    state.session_epoch = 9;
    adopt_owner(&mut state, &(9, "bob".parse().unwrap()));
    assert!(state.files.mkdir.records.is_empty());
    assert_eq!(record.principal().as_str(), "alice");
}

#[test]
fn existing_folder_creation_keeps_its_original_post_shape() {
    let state = state();
    let record = Record::capture(
        &state,
        &"alice".parse().unwrap(),
        id("op", 3).parse().unwrap(),
        "new",
    )
    .unwrap();
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), &record.bytes).unwrap();
    assert_eq!(record.request(), restored.request());
    assert_eq!(
        restored.request(),
        (
            "POST",
            "/api/file-manager/disks/P01/namespace".into(),
            json!({"kind":"mkdir","path":"named/folder/new","operation_id":id("op",3)}),
        )
    );
}

#[test]
fn duplicate_keys_and_conflicting_same_operation_records_remain_blocked() {
    let original = Arc::new(create());
    let duplicate = format!("{{\"principal\":\"alice\",{}", &original.bytes[1..]);
    assert!(Record::decode(&original.key(), &"alice".parse().unwrap(), &duplicate).is_err());
    let changed = Arc::new(
        Record::capture_directory_branch(
            &state(),
            &"alice".parse().unwrap(),
            original.operation(),
            source(),
            "different-name".into(),
        )
        .unwrap(),
    );
    let mut model = Model::default();
    model.merge(vec![Arc::clone(&original)]).unwrap();
    assert!(model.merge(vec![changed]).is_err());
    assert_eq!(model.records.len(), 1);
    assert_eq!(model.records[0].record.bytes, original.bytes);
    assert_eq!(model.records[0].record.request(), original.request());
}

#[test]
fn original_checkout_completion_only_clears_its_own_displayed_preview() {
    use crate::files::model::{DirectoryPreview, DirectoryPreviewSource};
    let record = Arc::new(checkout());
    let operation = record.operation().to_string();
    let mut state = state();
    state.files.directory_preview = Some(DirectoryPreview {
        source: DirectoryPreviewSource::Branch(crate::files::test_entries::branch(json!({
            "directory_branch_id":id("dbr",9), "head_tree_id":id("dtree",8),
        }))),
        path: "named/folder".into(),
    });
    state.files.mkdir.records.push(Retained {
        record,
        phase: Phase::Pending,
    });
    assert!(default_completion_applies(&state, &operation));
    state.files.directory = "another-folder".into();
    assert!(!default_completion_applies(&state, &operation));
    state.files.directory = "named/folder".into();
    state.files.directory_preview = None;
    assert!(!default_completion_applies(&state, &operation));
}

#[test]
fn first_empty_disk_cleanup_retry_requests_a_fresh_scan_before_new_writes() {
    let mut state = state();
    state.files.directory.clear();
    state.files.repository_path.clear();
    state.files.repository_id.clear();
    let record = Arc::new(
        Record::capture(
            &state,
            &"alice".parse().unwrap(),
            id("op", 3).parse().unwrap(),
            "first",
        )
        .unwrap(),
    );
    assert!(!record.can_lookup());
    state.files.mkdir.owner = Some((state.session_epoch, state.principal.clone().unwrap()));
    state.files.mkdir.records.push(Retained {
        record: Arc::clone(&record),
        phase: Phase::Settled,
    });
    let mut error = failure("local cleanup failed");
    error.request_id = Some("cleanup-source".into());
    assert!(!complete_cleanup(
        &mut state.files.mkdir,
        &record,
        Err(error)
    ));
    assert!(blocks_writes(&state));
    assert_eq!(state.files.mkdir.records.len(), 1);
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Settled));
    assert!(retry_record(&state, state.session_epoch, record.operation()).is_none());

    assert!(complete_cleanup(&mut state.files.mkdir, &record, Ok(())));
    assert!(state.files.mkdir.records.is_empty());
    assert!(
        blocks_writes(&state),
        "only the fresh scan may clear the prior error"
    );
    assert_eq!(
        state
            .files
            .mkdir
            .error
            .as_ref()
            .unwrap()
            .request_id
            .as_deref(),
        Some("cleanup-source")
    );
    assert!(
        state.files.operations.is_empty(),
        "local cleanup never resends a mutation"
    );
    state.files.mkdir.restore_records(Ok(Vec::new()));
    assert!(!blocks_writes(&state));
    assert!(state.files.mkdir.ready(&state));
}

#[test]
fn ordinary_cleanup_needs_no_extra_scan_and_preserves_other_originals() {
    let record = Arc::new(create());
    let pending = Arc::new(checkout());
    let mut state = state();
    state.files.mkdir.owner = Some((state.session_epoch, state.principal.clone().unwrap()));
    state.files.mkdir.records = vec![
        Retained {
            record: Arc::clone(&record),
            phase: Phase::Settled,
        },
        Retained {
            record: Arc::clone(&pending),
            phase: Phase::Pending,
        },
    ];
    assert!(!complete_cleanup(&mut state.files.mkdir, &record, Ok(())));
    assert_eq!(state.files.mkdir.records.len(), 1);
    assert!(Arc::ptr_eq(&state.files.mkdir.records[0].record, &pending));
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Pending));
    assert!(blocks_writes(&state));
    settle(
        &mut state.files.mkdir,
        &pending.operation().to_string(),
        SaveStatus::Published,
    );
    assert!(!complete_cleanup(&mut state.files.mkdir, &pending, Ok(())));
    assert!(!blocks_writes(&state));
    assert!(state.files.mkdir.ready(&state));
}

#[test]
fn cleanup_of_one_record_cannot_erase_an_unrelated_recovery_failure() {
    let record = Arc::new(create());
    let mut model = Model::default();
    model.records.push(Retained {
        record: Arc::clone(&record),
        phase: Phase::Settled,
    });
    model.error = Some(failure("another original record changed"));
    assert!(complete_cleanup(&mut model, &record, Ok(())));
    assert_eq!(
        model.error.as_ref().unwrap().message,
        "another original record changed"
    );
    assert!(model.records.is_empty());
    model.restore_records(Err(failure("another original record is still invalid")));
    assert_eq!(
        model.error.as_ref().unwrap().message,
        "another original record is still invalid"
    );
    let pending = Arc::new(checkout());
    model.restore_records(Ok(vec![Arc::clone(&pending)]));
    assert!(model.error.is_none());
    assert_eq!(model.records.len(), 1);
    assert!(Arc::ptr_eq(&model.records[0].record, &pending));
    assert!(matches!(model.records[0].phase, Phase::Pending));
}

#[test]
fn shared_encoding_keeps_each_original_shape_bytes_digest_and_request() {
    let folder = Record::capture(
        &state(),
        &"alice".parse().unwrap(),
        id("op", 3).parse().unwrap(),
        "new",
    )
    .unwrap();
    let mut moved = folder.stored;
    moved.format = Format::ObjectMove;
    moved.namespace = Namespace::ObjectMove {
        movement: storage_contract::file_relocation::ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
            source_entry_id: storage_contract::file_relocation::NamespaceEntryId::parse(&id(
                "entry", 2,
            ))
            .unwrap(),
            source_path: name_policy::LookupPath::parse("folder/new").unwrap(),
            target_slot_id: "P02".into(),
            target_repository_id: None,
            target_path: name_policy::PortablePath::parse("destination").unwrap(),
            replace: false,
            target_entry_id: None,
        },
    };
    let movement = Record::encode(moved, "object move is oversized").unwrap();
    let folder = Record::capture(
        &state(),
        &"alice".parse().unwrap(),
        id("op", 4).parse().unwrap(),
        "new",
    )
    .unwrap();
    for record in [create(), checkout(), historical_create(), folder, movement] {
        let expected = storage_contract::canonical_json(&record.stored).unwrap();
        let encoded = Record::encode(record.stored, "request is oversized").unwrap();
        assert_eq!(encoded.bytes.as_bytes(), expected);
        let restored =
            Record::decode(&encoded.key(), &"alice".parse().unwrap(), &encoded.bytes).unwrap();
        assert_eq!(restored.bytes, encoded.bytes);
        assert_eq!(restored.request(), encoded.request());
        assert_eq!(restored.digest, encoded.digest);
        assert_eq!(restored.relocation_digests, encoded.relocation_digests);
    }
}

#[test]
fn shared_encoding_keeps_the_exact_byte_limit_before_binding() {
    for extra in [0, 1] {
        let mut stored = create().stored;
        let overhead = storage_contract::canonical_json(&stored).unwrap().len()
            - stored.principal.as_str().len();
        stored.principal = "a".repeat(MAX_RECORD_BYTES - overhead + extra).parse().unwrap();
        assert_eq!(
            storage_contract::canonical_json(&stored).unwrap().len(),
            MAX_RECORD_BYTES + extra,
        );
        let error = match Record::encode(stored, "specific original request is oversized") {
            Ok(_) => panic!("the oversized account must not be admitted"),
            Err(error) => error,
        };
        if extra == 0 {
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert_eq!(error.message, "The folder recovery account is invalid.");
        } else {
            assert_eq!(error.code, ErrorCode::ResourceLimit);
            assert_eq!(error.message, "specific original request is oversized");
        }
    }
}

#[test]
fn shared_encoding_preserves_binding_errors_and_their_source() {
    let mut stored = create().stored;
    stored.slot = "../outside".into();
    let error = match Record::encode(stored, "request is oversized") {
        Ok(_) => panic!("an invalid slot must not be admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(matches!(
        error.source.as_deref(),
        Some(api::ErrorSource::FolderCreationRecovery(_))
    ));
}

const HISTORICAL_ORIGINAL: &str = concat!(
    "{\"format\":\"directory-branch-original-request\",",
    "\"namespace\":{\"kind\":\"directory_branch\",\"request\":{",
    "\"action\":\"create_historical\",\"name\":\"history-draft\",",
    "\"repository_path\":\"folder\",\"source\":{\"current\":{",
    "\"directory_branch_id\":\"dbr_00000000000000000000000000000003\",",
    "\"directory_id\":\"entry_00000000000000000000000000000002\",",
    "\"head_tree_id\":\"dtree_00000000000000000000000000000005\",",
    "\"index_generation\":\"9007199254740993\",",
    "\"namespace_head\":\"commit_00000000000000000000000000000006\",",
    "\"repository_id\":\"repo_00000000000000000000000000000001\",",
    "\"working_tree_id\":\"dtree_00000000000000000000000000000004\"},",
    "\"tree_id\":\"dtree_00000000000000000000000000000007\"}}},",
    "\"operation\":\"op_00000000000000000000000000000005\",",
    "\"path\":\"named/folder\",\"principal\":\"alice\",\"slot\":\"P01\"}",
);

#[test]
fn historical_registration_round_trip_keeps_canonical_bytes_and_the_distinct_tree() {
    let record = historical_create();
    let expected = HISTORICAL_ORIGINAL;
    assert_eq!(record.bytes.as_ref(), expected);
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), expected).unwrap();
    assert_eq!(restored.bytes, record.bytes);
    assert_eq!(restored.request(), record.request());
    assert_eq!(
        storage_contract::canonical_json(&restored.stored).unwrap(),
        expected.as_bytes()
    );
    let (method, url, body) = restored.request();
    assert_eq!(method, "POST");
    assert_eq!(
        url,
        "/api/file-manager/disks/P01/directory-branches/from-history"
    );
    assert_eq!(
        body,
        json!({
            "path":"named/folder", "name":"history-draft", "operation_id":id("op",5),
            "source":{"current":source(),"tree_id":id("dtree",7)},
        })
    );
    assert_eq!(body["source"]["current"], json!(source()));
    assert_eq!(
        body["source"]["current"]["index_generation"],
        "9007199254740993"
    );
    assert_eq!(body["source"]["tree_id"], id("dtree", 7));
    assert_ne!(
        body["source"]["tree_id"],
        body["source"]["current"]["working_tree_id"]
    );
    assert_ne!(
        body["source"]["tree_id"],
        body["source"]["current"]["head_tree_id"]
    );
    let digest = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"directory.git.branch-create-history", "principal":"alice",
            "body":{
                "repository_id":id("repo",1), "path":"folder", "name":"history-draft",
                "source":{"current":source(),"tree_id":id("dtree",7)},
            },
        }),
    )
    .unwrap();
    assert_eq!(restored.digest.as_deref(), Some(digest.as_str()));
    assert!(restored.can_lookup());
    assert!(!restored.is_directory_checkout());
}

#[test]
fn historical_root_requires_explicit_null_and_binds_current_namespace() {
    for display in ["", "named"] {
        let mut state = state();
        state.files.directory = display.into();
        state.files.repository_path.clear();
        let mut current = source();
        current.directory_id = current.repository_id.clone();
        let source = storage_contract::directory_git::HistoricalDirectoryBranchSource::new(
            current,
            id("dtree", 7).parse().unwrap(),
        )
        .unwrap();
        let record = Record::capture_historical_directory_branch(
            &state,
            &"alice".parse().unwrap(),
            id("op", 5).parse().unwrap(),
            source,
            "root-history".into(),
        )
        .unwrap();
        let restored =
            Record::decode(&record.key(), &"alice".parse().unwrap(), &record.bytes).unwrap();
        let stored: Value = json_codec::from_str(&record.bytes).unwrap();
        assert_eq!(stored["namespace"]["request"]["repository_path"], Value::Null);
        assert_eq!(restored.request().2["path"], display);
        assert_eq!(receipt(&restored)["result"]["input"]["path"], "");
        assert!(restored.matches_receipt(&receipt(&restored), ReceiptOrigin::Lookup));
    }
    for change in 0..4 {
        let mut state = state();
        match change {
            0 => state.files.repository_id = id("repo", 99),
            1 => state.files.namespace_head = id("commit", 99),
            2 => state.files.repository_path = "elsewhere".into(),
            3 => state.files.repository_path.clear(),
            _ => unreachable!(),
        }
        assert!(
            Record::capture_historical_directory_branch(
                &state,
                &"alice".parse().unwrap(),
                id("op", 5).parse().unwrap(),
                historical_source(),
                "history-draft".into(),
            )
            .is_err(),
            "{change}"
        );
    }
}

#[test]
fn historical_recovery_requires_the_complete_current_source() {
    let record = historical_create();
    for field in [
        "repository_id",
        "directory_id",
        "directory_branch_id",
        "working_tree_id",
        "head_tree_id",
        "index_generation",
        "namespace_head",
    ] {
        let mut stored: Value = json_codec::from_str(&record.bytes).unwrap();
        stored["namespace"]["request"]["source"]["current"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let bytes = json_codec::to_string(&stored).unwrap();
        assert!(
            Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).is_err(),
            "{field}"
        );
    }
}

#[test]
fn historical_recovery_rejects_malformed_path_and_unknown_fields() {
    let record = historical_create();
    for change in 0..13 {
        let mut stored: Value = json_codec::from_str(&record.bytes).unwrap();
        let request = &mut stored["namespace"]["request"];
        match change {
            0 => {
                request.as_object_mut().unwrap().remove("repository_path");
            }
            1 => request["repository_path"] = Value::Null,
            2 => request["repository_path"] = "outside".into(),
            3 => request["repository_path"] = "../outside".into(),
            4 => {
                request["source"].as_object_mut().unwrap().remove("tree_id");
            }
            5 => request["source"]["tree_id"] = id("tree", 7).into(),
            6 => request["source"]["current"]["index_generation"] = "0".into(),
            7 => request["source"]["current"]["index_generation"] = 2.into(),
            8 => request["source"]["current"]["namespace_head"] = "invalid".into(),
            9 => request["method"] = "GET".into(),
            10 => request["name"] = "../invalid".into(),
            11 => request["source"]["unknown"] = true.into(),
            12 => request["source"]["current"]["unknown"] = true.into(),
            _ => unreachable!(),
        }
        let bytes = json_codec::to_string(&stored).unwrap();
        assert!(
            Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).is_err(),
            "{change}"
        );
    }
    assert!(Record::decode(&record.key(), &"bob".parse().unwrap(), &record.bytes).is_err());
    assert_eq!(record.request(), historical_create().request());
}

#[test]
fn historical_receipt_binds_every_current_field_and_the_selected_historical_tree() {
    let record = historical_create();
    let valid = receipt(&record);
    for (field, changed) in [
        ("repository_id", json!(id("repo", 99))),
        ("directory_id", json!(id("entry", 99))),
        ("directory_branch_id", json!(id("dbr", 99))),
        ("working_tree_id", json!(id("dtree", 99))),
        ("head_tree_id", json!(id("dtree", 99))),
        ("index_generation", json!("9007199254740994")),
        ("namespace_head", json!(id("commit", 99))),
    ] {
        let mut wrong = valid.clone();
        wrong["result"]["input"]["source"]["current"][field] = changed;
        assert!(
            !record.matches_receipt(&wrong, ReceiptOrigin::Lookup),
            "{field}"
        );
    }
    for change in 0..11 {
        let mut wrong = valid.clone();
        match change {
            0 => wrong["result"]["input"]["source"]["tree_id"] = id("dtree", 99).into(),
            1 => wrong["result"]["head_tree_id"] = source().working_tree_id.into(),
            2 => wrong["result"]["head_tree_id"] = source().head_tree_id.into(),
            3 => wrong["result"]["directory_branch_id"] = source().directory_branch_id.into(),
            4 => wrong["result"]["selected"] = true.into(),
            5 => wrong["result"]["materialized"] = true.into(),
            6 => wrong["result"]["copied_files"] = "1".into(),
            7 => wrong["result"]["placeholder_files"] = "1".into(),
            8 => wrong["result"]["path"] = "elsewhere".into(),
            9 => wrong["result"]["name"] = "another".into(),
            10 => wrong["operation_kind"] = "directory.git.branch-create".into(),
            _ => unreachable!(),
        }
        assert!(
            !record.matches_receipt(&wrong, ReceiptOrigin::Lookup),
            "{change}"
        );
        assert!(!record.matches_receipt(&wrong, ReceiptOrigin::OriginalRequest));
    }
}

#[test]
fn historical_unknown_reload_and_session_change_keep_only_the_original_request() {
    let record = Arc::new(historical_create());
    let operation = record.operation();
    let original = record.request();
    let mut state = state();
    state.files.mkdir.owner = Some((7, "alice".parse().unwrap()));
    state.files.mkdir.restore_records(Ok(vec![Arc::clone(&record)]));
    settle(
        &mut state.files.mkdir,
        &operation.to_string(),
        SaveStatus::Unknown,
    );
    state.files.directory = "another".into();
    state.files.repository_id = id("repo", 99);
    state.files.mkdir.restore_records(Ok(vec![Arc::clone(&record)]));
    assert_eq!(state.files.mkdir.records.len(), 1);
    assert!(matches!(state.files.mkdir.records[0].phase, Phase::Pending));
    assert!(poll_allowed(&state, &operation.to_string()));
    assert!(!state.files.mutation_pending);
    assert!(state.files.operations.is_empty());
    assert!(state.files.default_operation.is_none());
    assert_eq!(retry_record(&state, 7, operation).unwrap().request(), original);
    state.session_epoch = 8;
    assert!(!poll_allowed(&state, &operation.to_string()));
    assert!(retry_record(&state, 7, operation).is_none());
    assert!(!receipt_matches(
        &state,
        &operation.to_string(),
        &receipt(&record)
    ));
    adopt_owner(&mut state, &(8, "alice".parse().unwrap()));
    assert_eq!(retry_record(&state, 8, operation).unwrap().request(), original);
    assert_eq!(state.files.mkdir.records[0].record.bytes, record.bytes);
    state.session_epoch = 9;
    state.principal = Some("bob".parse().unwrap());
    adopt_owner(&mut state, &(9, "bob".parse().unwrap()));
    assert!(state.files.mkdir.records.is_empty());
    assert_eq!(record.request(), original);
}

#[test]
fn historical_registration_never_clears_a_displayed_checkout_preview() {
    use crate::files::model::{DirectoryPreview, DirectoryPreviewSource};
    let record = Arc::new(historical_create());
    let operation = record.operation().to_string();
    let mut state = state();
    state.files.directory_preview = Some(DirectoryPreview {
        source: DirectoryPreviewSource::Branch(crate::files::test_entries::branch(json!({
            "directory_branch_id":id("dbr",3), "head_tree_id":id("dtree",7),
        }))),
        path: "named/folder".into(),
    });
    state.files.mkdir.records.push(Retained {
        record,
        phase: Phase::Pending,
    });
    assert!(!default_completion_applies(&state, &operation));
    settle(&mut state.files.mkdir, &operation, SaveStatus::Published);
    assert!(!default_completion_applies(&state, &operation));
    assert!(state.files.directory_preview.is_some());
    assert!(state.files.default_operation.is_none());
}

#[test]
fn historical_receipt_with_another_operation_cannot_settle_the_original() {
    let record = Arc::new(historical_create());
    let operation = record.operation().to_string();
    let mut published = receipt(&record);
    published["state"] = "completed".into();
    published["commit_state"] = json!(storage_contract::CommitState::Published);
    published["error"] = Value::Null;
    published["protection_at_publish"] =
        json!(storage_contract::ProtectionAtPublish::NotApplicable);
    published["publication_sequence"] = "19".into();
    assert!(record.matches_receipt(&published, ReceiptOrigin::Lookup));
    assert_eq!(
        FileOperation::from_receipt(&operation, &published).status(),
        SaveStatus::Published
    );
    published["operation_id"] = id("op", 99).into();
    let observed = FileOperation::from_receipt(&operation, &published);
    assert_eq!(observed.id(), operation);
    assert_eq!(observed.status(), SaveStatus::Unknown);
    let mut model = Model::default();
    model.restore_records(Ok(vec![Arc::clone(&record)]));
    settle(&mut model, &operation, observed.status());
    assert!(matches!(model.records[0].phase, Phase::Pending));
    assert_eq!(model.records[0].record.bytes, record.bytes);
}

#[test]
fn historical_recovery_byte_budget_keeps_the_exact_limit() {
    let record = historical_create();
    let mut bytes = record.bytes.to_string();
    bytes.push_str(&" ".repeat(MAX_RECORD_BYTES - bytes.len()));
    let restored = Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes).unwrap();
    assert_eq!(restored.bytes.as_ref(), bytes);
    assert_eq!(restored.request(), record.request());
    bytes.push(' ');
    let error = match Record::decode(&record.key(), &"alice".parse().unwrap(), &bytes) {
        Ok(_) => panic!("an oversized original must be refused"),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::ResourceLimit);
    assert_eq!(record.bytes.as_ref(), HISTORICAL_ORIGINAL);
}

#[test]
fn historical_recovery_record_budget_preserves_all_originals() {
    let originals = (1..=MAX_RECORDS)
        .map(|number| {
            Arc::new(
                Record::capture_historical_directory_branch(
                    &state(),
                    &"alice".parse().unwrap(),
                    id("op", u8::try_from(number).unwrap()).parse().unwrap(),
                    historical_source(),
                    "history-draft".into(),
                )
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut model = Model::default();
    model.restore_records(Ok(originals.clone()));
    assert!(model.error.is_none());
    assert_eq!(model.records.len(), MAX_RECORDS);
    let extra = Record::capture_historical_directory_branch(
        &state(),
        &"alice".parse().unwrap(),
        id("op", 99).parse().unwrap(),
        historical_source(),
        "history-draft".into(),
    )
    .unwrap();
    model.restore_records(Ok(vec![Arc::new(extra)]));
    assert_eq!(model.error.as_ref().unwrap().code, ErrorCode::ResourceLimit);
    assert_eq!(model.records.len(), MAX_RECORDS);
    for (retained, original) in model.records.iter().zip(originals) {
        assert!(Arc::ptr_eq(&retained.record, &original));
        assert!(matches!(retained.phase, Phase::Pending));
        assert_eq!(retained.record.request(), original.request());
    }
}

#[test]
fn principal_preserves_historical_record_bytes_and_exact_recovery_ownership() {
    for text in [" ", " e\u{301}/账号 ", "é", "e\u{301}"] {
        let principal: Principal = text.parse().unwrap();
        let bytes = HISTORICAL_ORIGINAL.replace("\"alice\"", &format!("\"{text}\""));
        let key = format!("{}{}", prefix(&principal), id("op", 5));
        let record = Record::decode(&key, &principal, &bytes).unwrap();
        assert_eq!(record.principal(), &principal);
        assert_eq!(record.bytes.as_ref(), bytes);
        assert_eq!(storage_contract::canonical_json(&record.stored).unwrap(), bytes.as_bytes());
        let mut state = state();
        state.principal = Some(principal.clone());
        state.files.mkdir.owner = Some((7, principal.clone()));
        state.files.mkdir.records.push(Retained {
            record: Arc::new(record),
            phase: Phase::Pending,
        });
        assert!(blocks_writes(&state));
        state.principal = None;
        assert!(!blocks_writes(&state));
        assert!(owner(&state).is_none());
        assert_eq!(state.files.mkdir.records[0].record.principal(), &principal);
        state.principal = Some(principal.clone());
        state.session_epoch = 8;
        assert!(!blocks_writes(&state));
        adopt_owner(&mut state, &(8, principal.clone()));
        assert!(blocks_writes(&state));
        let other: Principal = "another-account".parse().unwrap();
        state.principal = Some(other.clone());
        state.session_epoch = 9;
        adopt_owner(&mut state, &(9, other));
        assert!(state.files.mkdir.records.is_empty());
        assert_eq!(Record::decode(&key, &principal, &bytes).unwrap().bytes.as_ref(), bytes);
    }
}

#[test]
fn principal_recovery_admission_rejects_empty_missing_null_and_oversized_bytes() {
    let principal: Principal = "alice".parse().unwrap();
    let key = historical_create().key();
    let original: Value = json_codec::from_str(HISTORICAL_ORIGINAL).unwrap();
    for value in [json!(""), Value::Null, json!(3)] {
        let mut changed = original.clone();
        changed["principal"] = value;
        assert_eq!(
            Record::decode(&key, &principal, &changed.to_string()).err().unwrap().code,
            ErrorCode::InvalidRequest,
        );
    }
    let mut missing = original;
    missing.as_object_mut().unwrap().remove("principal");
    assert_eq!(
        Record::decode(&key, &principal, &missing.to_string()).err().unwrap().code,
        ErrorCode::InvalidRequest,
    );
    for length in [1024, 1025] {
        let text = "a".repeat(length);
        let principal = text.parse().unwrap();
        let bytes = HISTORICAL_ORIGINAL.replace("alice", &text);
        let key = format!("{}{}", prefix(&principal), id("op", 5));
        assert_eq!(Record::decode(&key, &principal, &bytes).is_ok(), length == 1024);
    }
    assert!(Record::decode(&key, &principal, HISTORICAL_ORIGINAL).is_ok());
}
