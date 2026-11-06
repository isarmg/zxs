use super::*;

fn status(expected: &ForeignPlanPreview) -> ForeignPlanStatus {
    ForeignPlanStatus {
        preview: expected.clone(),
        validation: ForeignPlanValidation::Required,
    }
}

fn unknown(model: &mut Model) -> ForeignPlanSave {
    model.acknowledged = true;
    let original = model.save_request().unwrap();
    model.save_failed(failure(503));
    original
}

#[test]
fn status_reads_require_an_original_submitted_review_and_fence_duplicates() {
    for save in [
        SaveState::Unsubmitted,
        SaveState::Sending,
        SaveState::Rejected,
    ] {
        let mut model = reviewed_model();
        model.save = save;
        assert!(model.read_request(Read::Status).is_none());
        assert!(model.read_request(Read::Revalidate).is_none());
    }
    let mut model = reviewed_model();
    let original = unknown(&mut model);
    assert!(model.read_request(Read::Revalidate).is_none());
    let (generation, expected) = model.read_request(Read::Status).unwrap();
    assert_eq!(expected, original.preview);
    assert_eq!(generation, model.generation);
    assert!(model.reading && model.blocks_edit());
    assert!(model.read_request(Read::Status).is_none());
    assert!(model.read_request(Read::Revalidate).is_none());
    assert!(model.save_request().is_none());
}

#[test]
fn a_late_status_after_navigation_cannot_release_a_newer_original_record_query() {
    let mut model = reviewed_model();
    let original = unknown(&mut model);
    let (old_generation, _) = model.read_request(Read::Status).unwrap();
    model.leave();
    model.invalidate();
    let (new_generation, expected) = model.read_request(Read::Status).unwrap();
    assert!(new_generation > old_generation);
    model.finish_read(old_generation, Ok(status(&original.preview)));
    assert_eq!(model.save, SaveState::Unknown);
    assert!(model.reading);
    assert!(model.stored.is_none());
    model.finish_read(old_generation, Err(failure(404)));
    assert!(model.reading && model.error.is_none());
    let result = read_status(
        json!(status(&expected)),
        &expected,
        ForeignPlanValidation::Required,
    );
    model.finish_read(new_generation, result);
    assert_eq!(model.save, SaveState::Stored);
    assert!(!model.reading);
    assert_eq!(model.stored, Some(status(&original.preview)));
}

#[test]
fn failed_status_reads_keep_all_origins_missing_bodies_and_the_original_retry() {
    for (http_status, code) in [
        (404, storage_contract::ErrorCode::OperationNotFound),
        (409, storage_contract::ErrorCode::PlanStale),
        (503, storage_contract::ErrorCode::DiskUnavailable),
    ] {
        let mut storage = selected_review();
        let original = unknown(&mut storage.foreign.plan);
        storage.foreign.invalidate();
        assert!(storage.foreign.preview.is_none());
        super::super::super::super::wizard::choose(&mut storage, "/mnt/source");
        let model = &mut storage.foreign.plan;
        let (generation, expected) = model.read_request(Read::Status).unwrap();
        assert_eq!(expected, original.preview);
        let mut error = failure(http_status);
        error.code = code;
        model.finish_read(generation, Err(error));
        assert_eq!(model.save, SaveState::Unknown);
        assert!(model.stored.is_none());
        assert_eq!(model.error.as_ref().unwrap().code, code);
        assert_eq!(
            model.error.as_ref().unwrap().request_id.as_deref(),
            Some("original-request")
        );
        assert_eq!(model.save_request(), Some(original));
    }
}

#[test]
fn exact_status_recovers_after_leaving_source_and_reselecting_without_claiming_import() {
    let mut storage = selected_review();
    let original = unknown(&mut storage.foreign.plan);
    storage.foreign.invalidate();
    let model = &mut storage.foreign.plan;
    let (generation, expected) = model.read_request(Read::Status).unwrap();
    model.finish_read(
        generation,
        read_status(
            json!(status(&expected)),
            &expected,
            ForeignPlanValidation::Required,
        ),
    );
    assert!(
        storage.foreign.preview.is_none(),
        "query does not reread or invent source evidence"
    );
    for _ in 0..2 {
        super::super::super::super::wizard::choose(&mut storage, "/mnt/source");
    }
    let model = &storage.foreign.plan;
    assert_eq!(model.save, SaveState::Stored);
    assert_eq!(model.stored, Some(status(&original.preview)));
    assert!(!model.can_save());
    assert_eq!(
        model.review.as_ref().unwrap().execution,
        ForeignExecutionCapability::Unavailable
    );
    assert_eq!(
        model.review.as_ref().unwrap().source.histories,
        original.preview.source.histories
    );
}

#[test]
fn old_revalidation_cannot_restore_an_observation_after_navigation_and_editing() {
    let mut model = reviewed_model();
    model.accept_saved(status(&review()));
    let (generation, expected) = model.read_request(Read::Revalidate).unwrap();
    model.leave();
    model.edit(|draft| draft.parent = "another-empty-parent".into());
    let before = model.generation;
    let mut matched = status(&expected);
    matched.validation = ForeignPlanValidation::MatchedAtObservation;
    model.finish_read(generation, Ok(matched));
    assert_eq!(model.generation, before);
    assert_eq!(model.save, SaveState::Unsubmitted);
    assert!(model.review.is_none() && model.stored.is_none());
    assert!(!model.acknowledged && !model.can_save());
}

#[test]
fn malformed_success_never_settles_an_unknown_save_or_replaces_its_history() {
    for change in 0..5 {
        let mut model = reviewed_model();
        let original = unknown(&mut model);
        let (generation, expected) = model.read_request(Read::Status).unwrap();
        let mut response = status(&expected);
        match change {
            0 => response.preview.request.operation_id = operation(9),
            1 => response.preview.source.histories.swap(0, 1),
            2 => response.preview.source.histories[0]
                .temporarily_unavailable_versions
                .clear(),
            3 => response.preview.target.histories[0].destination = "substitute".into(),
            _ => response.validation = ForeignPlanValidation::MatchedAtObservation,
        }
        let result = read_status(json!(response), &expected, ForeignPlanValidation::Required);
        assert!(result.is_err());
        model.finish_read(generation, result);
        assert_eq!(model.save, SaveState::Unknown);
        assert!(model.stored.is_none());
        assert_eq!(
            model.error.as_ref().unwrap().code,
            storage_contract::ErrorCode::InvalidResponse
        );
        assert_eq!(model.save_request(), Some(original));
    }
}

#[test]
fn same_inode_identity_cannot_substitute_changed_evidence_or_prune_missing_bodies() {
    let expected = review();
    for change in 0..6 {
        let mut response = expected.clone();
        match change {
            0 => response.source.histories[0].manifest_hash = "9".repeat(64),
            1 => response.source.review_digest = "9".repeat(64),
            2 => response.source.histories[0].temporarily_unavailable_versions[0] = id("ver", 99),
            3 => response.source.histories[0].origin = response.source.histories[1].origin.clone(),
            4 => response.source.checkpoint.sequence += 1,
            _ => {
                response.source.histories[0]
                    .temporarily_unavailable_versions
                    .clear();
                response.source.histories[0].verified_local_versions += 1;
            }
        }
        assert_eq!(response.source.source, expected.source.source);
        assert!(read_preview(json!(response), &expected.request, &expected.source).is_err());
    }
}

#[test]
fn writer_intent_or_copy_status_cannot_be_decoded_as_an_import_or_review_receipt() {
    let expected = review();
    for (field, value) in [
        ("state", json!("intent_saved")),
        ("state", json!("completed")),
        ("copy_operation_id", json!(operation(10))),
        ("publication", json!({"state":"published"})),
    ] {
        let mut response = json!(status(&expected));
        response[field] = value;
        assert!(read_status(response, &expected, ForeignPlanValidation::Required).is_err());
    }
    for execution in ["intent_saved", "copy_sealed", "completed"] {
        let mut response = json!(status(&expected));
        response["preview"]["execution"] = execution.into();
        assert!(read_status(response, &expected, ForeignPlanValidation::Required).is_err());
    }
}
