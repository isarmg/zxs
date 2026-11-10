use super::*;
use storage_contract::storage_addition::*;

fn preview() -> OrdinaryDiskConversionPreview {
    let selection = super::super::tests::candidate("/mnt/new");
    OrdinaryDiskConversionPreview {
        operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
        import_operation_id: "op_00000000000000000000000000000002".parse().unwrap(),
        source: DirectorySource {
            path: "/mnt/source".into(),
            discovery_identity: selection.discovery_identity.clone(),
            identity: selection.root_identity.clone(),
        },
        source_preview: OrdinarySourcePreview {
            digest: "a".repeat(64),
            files: 3,
            directories: 2,
            content_bytes: 9007199254740993,
            temporary_bytes: 9007199254740993,
        },
        target: AdditionPreview {
            authority: AdditionAuthority::ActiveMembership,
            request: AdditionRequest {
                operation_id: "op_00000000000000000000000000000003".parse().unwrap(),
                additions: vec![DirectoryAddition {
                    selection,
                    slot_id: "N01".into(),
                    level: 1,
                    upstream_slot_id: None,
                    metadata_member: false,
                }],
            },
            pool_id: "pool_00000000000000000000000000000001".into(),
            configuration_digest: "b".repeat(64),
            preview_digest: "c".repeat(64),
            original_metadata_members: 1,
            target_metadata_members: 1,
            added_directories: 1,
        },
        preview_digest: "d".repeat(64),
    }
}
fn receipt(operation_id: OperationId, kind: &str) -> storage_contract::OperationResult {
    storage_contract::OperationResult {
        operation_id,
        kind: kind.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: "e".repeat(64),
        sequence: 3,
        result: json!({"generation":"f".repeat(64)}),
    }
}
fn status(phase: OrdinaryDiskConversionPhase) -> OrdinaryDiskConversionStatus {
    let preview = preview();
    let registration =
        (phase != OrdinaryDiskConversionPhase::Registering).then(|| AdditionStatus {
            preview: preview.target.clone(),
            operation_id: preview.target.request.operation_id,
            state: AdditionState::Completed,
            phase: AdditionPhase::Completed,
            continuation: AdditionContinuation::Add,
            result: Some(receipt(
                preview.target.request.operation_id,
                "membership-add",
            )),
            failure: None,
        });
    OrdinaryDiskConversionStatus {
        result: (phase == OrdinaryDiskConversionPhase::Completed)
            .then(|| receipt(preview.import_operation_id, "workspace.commit")),
        preview,
        phase,
        registration,
        workspace_id: (phase == OrdinaryDiskConversionPhase::Completed)
            .then(|| "ws_00000000000000000000000000000001".into()),
        root_operation_id: None,
        stop_operation_id: None,
        stop_result: None,
        failure: None,
    }
}
fn request() -> OrdinaryDiskConversionRequest {
    let preview = preview();
    let target = preview.target.request.additions[0].clone();
    OrdinaryDiskConversionRequest {
        operation_id: preview.operation_id,
        source: preview.source,
        target: target.selection,
        slot_id: target.slot_id,
        metadata_member: target.metadata_member,
    }
}
#[test]
fn ordinary_conversion_binds_exact_new_primary_and_preserves_large_byte_counts() {
    let preview = preview();
    assert_eq!(read_preview(json!(preview), &request()).unwrap(), preview);
    for change in 0..8 {
        let mut changed = preview.clone();
        match change {
            0 => changed.target.request.additions[0].level = 2,
            1 => changed.target.request.additions[0].upstream_slot_id = Some("P01".into()),
            2 => changed.source.path = "/mnt/new".into(),
            3 => changed.operation_id = changed.import_operation_id,
            4 => changed.import_operation_id = changed.target.request.operation_id,
            5 => changed.preview_digest = "short".into(),
            6 => changed.source_preview.digest = "G".repeat(64),
            7 => changed
                .target
                .request
                .additions
                .push(changed.target.request.additions[0].clone()),
            _ => unreachable!(),
        }
        assert!(validate_preview(&changed).is_err(), "change {change}");
    }
    let mut changed = preview.clone();
    changed.target.request.additions[0]
        .selection
        .root_identity
        .inode += 1;
    assert!(read_preview(json!(changed), &request()).is_err());
    let mut changed = preview.clone();
    changed.source.identity.inode += 1;
    assert!(read_preview(json!(changed), &request()).is_err());
    let mut wire = json!(preview);
    wire["source_preview"]["content_bytes"] = json!(9007199254740993_u64);
    assert!(read_preview(wire, &request()).is_err());
}
#[test]
fn registration_or_wrong_import_receipt_never_proves_conversion_complete() {
    for phase in [
        OrdinaryDiskConversionPhase::Registering,
        OrdinaryDiskConversionPhase::Importing,
        OrdinaryDiskConversionPhase::Completed,
        OrdinaryDiskConversionPhase::RecoveryRequired,
    ] {
        assert!(validate_status(&status(phase), None, None).is_ok());
    }
    let completed = status(OrdinaryDiskConversionPhase::Completed);
    for change in 0..8 {
        let mut changed = completed.clone();
        match change {
            0 => changed.result = None,
            1 => changed.registration = None,
            2 => changed.workspace_id = None,
            3 => changed.workspace_id = Some("unbound".into()),
            4 => {
                changed.result.as_mut().unwrap().operation_id =
                    changed.preview.target.request.operation_id
            }
            5 => changed.result.as_mut().unwrap().kind = "membership-add".into(),
            6 => changed.result.as_mut().unwrap().commit_state = CommitState::Prepared,
            7 => changed.phase = OrdinaryDiskConversionPhase::Importing,
            _ => unreachable!(),
        }
        assert!(
            validate_status(&changed, None, None).is_err(),
            "change {change}"
        );
    }
    let mut other = preview();
    other.source.path = "/mnt/another".into();
    assert!(validate_status(&completed, Some(&other), None).is_err());
}
#[test]
fn unknown_outcome_and_early_missing_record_keep_original_intent() {
    let original = preview();
    let mut model = Model {
        submitted: true,
        preview: Some(original.clone()),
        reading: true,
        ..Default::default()
    };
    assert!(!finish_read(
        &mut model,
        Ok(OrdinaryDiskConversionStatusResponse { operation: None })
    ));
    assert!(model.blocks_edit());
    assert_eq!(model.preview, Some(original.clone()));
    assert!(model.checked);
    assert!(resumable(&model));
    model.sending = true;
    assert!(!resumable(&model));
    model.rejected = true;
    finish_read(
        &mut model,
        Ok(OrdinaryDiskConversionStatusResponse { operation: None }),
    );
    assert!(model.submitted);
    model.sending = false;
    finish_read(
        &mut model,
        Ok(OrdinaryDiskConversionStatusResponse { operation: None }),
    );
    assert!(!model.submitted);
    assert!(model.preview.is_none());
}
#[test]
fn retained_workspace_requires_original_recovery_not_another_import() {
    let mut record = status(OrdinaryDiskConversionPhase::RecoveryRequired);
    let mut model = Model {
        checked: true,
        ..Default::default()
    };
    assert!(!accept(&mut model, record.clone()));
    assert!(model.blocks_edit());
    assert!(resumable(&model));
    record.workspace_id = Some("ws_00000000000000000000000000000001".into());
    accept(&mut model, record.clone());
    assert!(!resumable(&model));
    record.workspace_id = None;
    record.root_operation_id = Some("op_00000000000000000000000000000004".parse().unwrap());
    accept(&mut model, record);
    assert!(!resumable(&model));
}
#[test]
fn only_exact_completed_import_releases_fence_and_late_reads_cannot_regress_it() {
    let mut model = Model::default();
    assert!(!accept(
        &mut model,
        status(OrdinaryDiskConversionPhase::Importing)
    ));
    assert!(model.blocks_edit());
    assert!(accept(
        &mut model,
        status(OrdinaryDiskConversionPhase::Completed)
    ));
    assert!(!model.blocks_edit());
    assert!(!accept(
        &mut model,
        status(OrdinaryDiskConversionPhase::Registering)
    ));
    assert_eq!(
        model.status.unwrap().phase,
        OrdinaryDiskConversionPhase::Completed
    );
}
#[test]
fn preview_edits_invalidate_acknowledgment_but_cannot_clear_submitted_review() {
    let mut model = Model {
        preview: Some(preview()),
        acknowledged: true,
        ..Default::default()
    };
    model.invalidate();
    assert!(model.preview.is_none());
    assert!(!model.acknowledged);
    model.preview = Some(preview());
    model.submitted = true;
    model.acknowledged = true;
    model.invalidate();
    assert!(model.preview.is_some());
    assert!(model.acknowledged);
    let storage = super::super::Model {
        conversion: model,
        ..Default::default()
    };
    assert!(storage.reload_blocked());
    assert!(!storage.editable());
}
#[test]
fn conversion_form_has_no_free_source_path_or_backup_level_selector() {
    let source = include_str!("../../../../src/web/src/admin/storage/conversion/view.rs");
    assert!(source.contains("data-conversion-target"));
    assert!(source.contains("data-conversion-slot"));
    assert!(source.contains("data-conversion-confirm"));
    assert!(source.contains("data-conversion-status-refresh"));
    assert!(!source.contains("type=\"number\""));
    assert!(!source.contains("data-conversion-source"));
}

fn stopping() -> OrdinaryDiskConversionStatus {
    let mut record = status(OrdinaryDiskConversionPhase::RecoveryRequired);
    record.phase = OrdinaryDiskConversionPhase::Stopping;
    record.workspace_id = Some("ws_00000000000000000000000000000001".into());
    record.stop_operation_id = Some("op_00000000000000000000000000000005".parse().unwrap());
    record
}
fn stopped() -> OrdinaryDiskConversionStatus {
    let mut record = stopping();
    record.phase = OrdinaryDiskConversionPhase::Stopped;
    let mut result = receipt(record.stop_operation_id.unwrap(), "directory.import.stop");
    result.result = json!({
        "timestamp":"1793887680","membership_epoch":"2","version_protection":[],
        "import_operation_id":record.preview.import_operation_id,
        "target":{"pool_id":record.preview.target.pool_id,"disk_id":"disk_00000000000000000000000000000001","slot_id":"N01","binding_generation":"1"},
        "root_operation_id":record.root_operation_id,"workspace_id":record.workspace_id,
        "workspace_retention":"aborted","source_preserved":true,"target_preserved":true,
    });
    record.stop_result = Some(result);
    record
}
#[test]
fn stop_requires_reviewed_original_operation_and_retained_state_confirmation() {
    let record = status(OrdinaryDiskConversionPhase::RecoveryRequired);
    let original = record.preview.clone();
    let mut storage = super::super::Model::default();
    accept(&mut storage.conversion, record);
    assert!(
        !stoppable(&storage.conversion),
        "must check original status first"
    );
    storage.conversion.checked = true;
    assert!(stoppable(&storage.conversion));
    assert!(
        prepare_submit(&mut storage, Submit::Stop).is_none(),
        "explicit retention acknowledgment is mandatory"
    );
    storage.conversion.stop_acknowledged = true;
    let (review, path, body) = prepare_submit(&mut storage, Submit::Stop).unwrap();
    assert_eq!(review, original);
    assert_eq!(path, "/api/admin/storage/conversion/stop");
    let request: OrdinaryDiskConversionStop = json_codec::from_value(body).unwrap();
    assert_eq!(request.operation_id, original.operation_id);
    assert!(request.acknowledge_retained_state);
    assert!(storage.conversion.submitted && storage.conversion.stop_requested);
    assert!(!storage.conversion.stop_acknowledged);
    assert!(!resumable(&storage.conversion));
    assert!(
        prepare_submit(&mut storage, Submit::Stop).is_none(),
        "double click is fenced"
    );
    storage.conversion.sending = false;
    storage.conversion.checked = true;
    assert!(
        prepare_submit(&mut storage, Submit::Resume).is_none(),
        "stop intent cannot turn into import"
    );
}
#[test]
fn stopped_requires_distinct_exact_permanent_receipt_and_retains_data_references() {
    let stopped = stopped();
    assert!(validate_status(&stopped, Some(&preview()), stopped.stop_operation_id).is_ok());
    for change in 0..9 {
        let mut changed = stopped.clone();
        match change {
            0 => changed.stop_result = None,
            1 => changed.stop_operation_id = None,
            2 => {
                changed.stop_result.as_mut().unwrap().operation_id =
                    changed.preview.import_operation_id
            }
            3 => changed.stop_result.as_mut().unwrap().kind = "workspace.commit".into(),
            4 => changed.stop_result.as_mut().unwrap().commit_state = CommitState::Prepared,
            5 => {
                changed.result = Some(receipt(
                    changed.preview.import_operation_id,
                    "workspace.commit",
                ))
            }
            6 => changed.registration = None,
            7 => {
                changed.failure = Some(AdditionFailure {
                    code: storage_contract::ErrorCode::RecoveryRequired,
                    message: "unconfirmed".into(),
                })
            }
            8 => changed.phase = OrdinaryDiskConversionPhase::Completed,
            _ => unreachable!(),
        }
        assert!(
            validate_status(&changed, Some(&preview()), None).is_err(),
            "change {change}"
        );
    }
    let other = "op_00000000000000000000000000000006".parse().unwrap();
    assert!(validate_status(&stopped, None, Some(other)).is_err());
    let mut model = Model::default();
    assert!(accept(&mut model, stopped.clone()));
    assert!(!model.blocks_edit());
    assert!(!stoppable(&model));
    assert!(!resumable(&model));
    assert_eq!(model.status, Some(stopped));
    assert!(
        !accept(&mut model, stopping()),
        "late read cannot regress a permanent stop"
    );
}
#[test]
fn stop_unknown_or_missing_record_never_releases_fence_or_restarts_import() {
    let mut model = Model {
        checked: true,
        ..Default::default()
    };
    accept(&mut model, stopping());
    assert!(model.stop_requested && model.submitted);
    assert!(!resumable(&model));
    model.rejected = true;
    finish_read(
        &mut model,
        Ok(OrdinaryDiskConversionStatusResponse { operation: None }),
    );
    assert!(model.blocks_edit());
    assert!(model.preview.is_some());
    finish_read(
        &mut model,
        Err(api::error(
            storage_contract::ErrorCode::OperationNotFound,
            "temporarily absent",
        )),
    );
    assert!(model.blocks_edit());
    assert!(!resumable(&model));
    let mut recovering = stopping();
    recovering.phase = OrdinaryDiskConversionPhase::RecoveryRequired;
    let mut reloaded = Model::default();
    accept(&mut reloaded, recovering);
    reloaded.checked = true;
    assert!(reloaded.stop_requested);
    assert!(!resumable(&reloaded));
    assert!(stoppable(&reloaded));
}
#[test]
fn stopped_does_not_replace_an_already_published_original_import() {
    let mut completed = status(OrdinaryDiskConversionPhase::Completed);
    completed.stop_operation_id = stopping().stop_operation_id;
    assert!(validate_status(&completed, None, completed.stop_operation_id).is_ok());
    let mut model = Model::default();
    accept(&mut model, stopping());
    assert!(accept(&mut model, completed));
    assert!(!model.blocks_edit());
    assert!(model.status.unwrap().result.is_some());
}
#[test]
fn stop_fields_are_required_nullable_and_initial_registration_cannot_be_stopped() {
    for field in ["stop_operation_id", "stop_result"] {
        let mut value = json!(status(OrdinaryDiskConversionPhase::Completed));
        value.as_object_mut().unwrap().remove(field);
        assert!(
            decode::<OrdinaryDiskConversionStatus>(value).is_err(),
            "{field} is required even when null"
        );
    }
    let mut model = Model::default();
    accept(&mut model, status(OrdinaryDiskConversionPhase::Registering));
    model.checked = true;
    assert!(!stoppable(&model));
    let mut changed = stopping();
    changed.registration = None;
    assert!(validate_status(&changed, None, None).is_err());
}

#[test]
fn permanent_stop_receipt_binds_original_import_target_and_retention_evidence() {
    let original = stopped();
    for change in 0..9 {
        let mut changed = original.clone();
        let evidence = &mut changed.stop_result.as_mut().unwrap().result;
        match change {
            0 => evidence["import_operation_id"] = json!("op_00000000000000000000000000000077"),
            1 => evidence["root_operation_id"] = json!("op_00000000000000000000000000000078"),
            2 => evidence["workspace_id"] = json!("ws_00000000000000000000000000000002"),
            3 => evidence["target"]["slot_id"] = json!("N02"),
            4 => evidence["target"]["pool_id"] = json!("pool_00000000000000000000000000000002"),
            5 => evidence["target"]["binding_generation"] = json!("2"),
            6 => evidence["source_preserved"] = json!(false),
            7 => evidence["target_preserved"] = json!(false),
            8 => {
                evidence.as_object_mut().unwrap().remove("workspace_id");
            }
            _ => unreachable!(),
        }
        assert!(
            validate_status(&changed, None, None).is_err(),
            "change {change}"
        );
    }
    let mut valid = original;
    for state in [CommitState::PublishedDegraded, CommitState::Acked] {
        valid.stop_result.as_mut().unwrap().commit_state = state;
        assert!(validate_status(&valid, None, None).is_ok());
    }
}

#[test]
fn stop_native_envelope_is_required_and_does_not_allow_prepared_versions() {
    let original = stopped();
    for field in ["timestamp", "membership_epoch", "version_protection"] {
        let mut changed = original.clone();
        changed
            .stop_result
            .as_mut()
            .unwrap()
            .result
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            validate_status(&changed, None, None).is_err(),
            "missing {field}"
        );
    }
    for field in ["timestamp", "membership_epoch"] {
        let mut changed = original.clone();
        changed.stop_result.as_mut().unwrap().result[field] = 1.into();
        assert!(
            validate_status(&changed, None, None).is_err(),
            "integer {field}"
        );
    }
    let mut changed = original.clone();
    changed.stop_result.as_mut().unwrap().result["version_protection"] = json!([null]);
    assert!(validate_status(&changed, None, None).is_err());
    let mut changed = original;
    changed.stop_result.as_mut().unwrap().result["unexpected"] = true.into();
    assert!(validate_status(&changed, None, None).is_err());
}

#[test]
fn real_server_stopped_status_passes_browser_receipt_validation() {
    let status: OrdinaryDiskConversionStatus = json_codec::from_str(include_str!(
        "../../fixtures/ordinary-conversion-stopped.json"
    ))
    .unwrap();
    assert_eq!(status.phase, OrdinaryDiskConversionPhase::Stopped);
    validate_status(&status, Some(&status.preview), status.stop_operation_id).unwrap();
    assert!(status.workspace_id.is_some());
    let mut model = Model::default();
    assert!(accept(&mut model, status));
    assert!(!model.blocks_edit());
}
