use super::*;

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
    state.files.slot = "P01".into();
    state.files.directory = "named/folder".into();
    state.files.repository_path = "folder".into();
    state.files.repository_id = id("repo", 1);
    state.files.namespace_head = id("commit", 6);
    state
}
fn folder(operation: u8) -> Record {
    Record::capture(
        &state(),
        &"alice".parse().unwrap(),
        id("op", operation).parse().unwrap(),
        "new",
    )
    .unwrap()
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
    let record = Arc::new(folder(1));
    let pending = Arc::new(folder(2));
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
    let record = Arc::new(folder(1));
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
    let pending = Arc::new(folder(2));
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
    for record in [folder, movement] {
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
        let mut stored = folder(1).stored;
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
    let mut stored = folder(1).stored;
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

#[test]
fn navigation_retry_uses_the_retained_request_and_is_fenced_by_owner_and_phase() {
    for record in [folder(1), folder(2)] {
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
    let record = Arc::new(folder(1));
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
    let original = Arc::new(folder(1));
    let duplicate = format!("{{\"principal\":\"alice\",{}", &original.bytes[1..]);
    assert!(Record::decode(&original.key(), &"alice".parse().unwrap(), &duplicate).is_err());
    let changed = Arc::new(
        Record::capture(
            &state(),
            &"alice".parse().unwrap(),
            original.operation(),
            "different-name",
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
fn namespace_recovery_rejects_retired_branch_formats_without_reinterpreting_them() {
    let record = folder(1);
    let mut value: Value = json_codec::from_str(&record.bytes).unwrap();
    value["format"] = "directory-branch-original-request".into();
    let bytes = value.to_string();
    assert!(Record::decode(&record.key(), record.principal(), &bytes).is_err());
    assert_eq!(record.request().2["kind"], "mkdir");
    let mut model = Model::default();
    model.restore_records(Err(failure("unsupported original request was preserved")));
    assert!(model.error.is_some());
    assert!(model.records.is_empty());
}

#[test]
fn namespace_recovery_binds_account_operation_path_and_exact_bytes() {
    let record = folder(1);
    let original: Value = json_codec::from_str(&record.bytes).unwrap();
    for change in 0..7 {
        let mut changed = original.clone();
        match change {
            0 => {
                changed.as_object_mut().unwrap().remove("path");
            }
            1 => changed["path"] = Value::Null,
            2 => changed["operation"] = id("op", 99).into(),
            3 => changed["principal"] = "".into(),
            4 => changed["principal"] = Value::Null,
            5 => {
                changed.as_object_mut().unwrap().remove("principal");
            }
            6 => changed["namespace"]["method"] = "GET".into(),
            _ => unreachable!(),
        }
        assert!(Record::decode(&record.key(), record.principal(), &changed.to_string()).is_err());
    }
    assert!(Record::decode(&record.key(), &"bob".parse().unwrap(), &record.bytes).is_err());
    for text in [" ", " e\u{301}/账号 ", "é", "e\u{301}"] {
        let principal: Principal = text.parse().unwrap();
        let bytes = record.bytes.replace("alice", text);
        let key = format!("{}{}", prefix(&principal), record.operation());
        let restored = Record::decode(&key, &principal, &bytes).unwrap();
        assert_eq!(restored.principal(), &principal);
        assert_eq!(restored.bytes.as_ref(), bytes);
    }
    for length in [1024, 1025] {
        let text = "a".repeat(length);
        let principal = text.parse().unwrap();
        let bytes = record.bytes.replace("alice", &text);
        let key = format!("{}{}", prefix(&principal), record.operation());
        assert_eq!(Record::decode(&key, &principal, &bytes).is_ok(), length == 1024);
    }
}

#[test]
fn namespace_recovery_record_budget_preserves_all_originals() {
    let originals = (0..MAX_RECORDS)
        .map(|index| Arc::new(folder(index as u8)))
        .collect::<Vec<_>>();
    let mut model = Model::default();
    model.restore_records(Ok(originals.clone()));
    assert!(model.error.is_none());
    assert_eq!(model.records.len(), MAX_RECORDS);
    model.restore_records(Ok(vec![Arc::new(folder(99))]));
    assert_eq!(model.error.as_ref().unwrap().code, ErrorCode::ResourceLimit);
    assert_eq!(model.records.len(), MAX_RECORDS);
    for (retained, original) in model.records.iter().zip(originals) {
        assert!(Arc::ptr_eq(&retained.record, &original));
        assert!(matches!(retained.phase, Phase::Pending));
        assert_eq!(retained.record.request(), original.request());
    }
}

#[test]
fn mkdir_receipts_keep_exact_paths_digests_and_first_root_request_origin() {
    let record = folder(1);
    let receipt = json!({
        "operation_kind":"file.namespace",
        "request_digest":record.digest,
        "result":{"action":"mkdir","path":"folder/new"},
    });
    assert!(record.matches_receipt(&receipt, ReceiptOrigin::Lookup));
    for change in 0..3 {
        let mut wrong = receipt.clone();
        match change {
            0 => wrong["request_digest"] = "other".into(),
            1 => wrong["result"]["path"] = "other".into(),
            2 => wrong["operation_kind"] = "directory.git.checkout".into(),
            _ => unreachable!(),
        }
        assert!(!record.matches_receipt(&wrong, ReceiptOrigin::OriginalRequest));
    }
    let mut state = state();
    state.files.directory.clear();
    state.files.repository_path.clear();
    state.files.repository_id.clear();
    let root = Record::capture(&state, record.principal(), record.operation(), "first").unwrap();
    let receipt = json!({
        "operation_kind":"file.namespace",
        "result":{"action":"mkdir","path":"first"},
    });
    assert!(!root.can_lookup());
    assert!(!root.matches_receipt(&receipt, ReceiptOrigin::Lookup));
    assert!(root.matches_receipt(&receipt, ReceiptOrigin::OriginalRequest));
}
