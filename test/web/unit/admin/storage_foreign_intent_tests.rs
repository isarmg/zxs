use super::*;

fn operation(n: u8) -> OperationId {
    format!("op_{n:032x}").parse().unwrap()
}
fn preview() -> ForeignIntentPreview {
    let plan = super::super::tests::review();
    ForeignIntentPreview {
        request: ForeignIntentRequest {
            operation_id: operation(2),
            plan: ForeignPlanSelection {
                operation_id: plan.request.operation_id,
                review_digest: plan.review_digest.clone(),
            },
        },
        plan,
        principal: "admin".into(),
        intent_digest: "5".repeat(64),
        effect: ForeignIntentEffect::SaveExecutionIntent,
        execution: ForeignExecutionCapability::Unavailable,
    }
}
fn status(expected: &ForeignIntentPreview) -> ForeignIntentStatus {
    ForeignIntentStatus {
        address: query(expected),
        plan: expected.request.plan.clone(),
        stage: ForeignIntentStage::IntentSaved,
        histories: 2,
        temporarily_unavailable_versions: 2,
        execution: ForeignExecutionCapability::Unavailable,
    }
}
fn model() -> Model {
    let mut model = Model::default();
    let generation = model.begin_preview().unwrap();
    model.finish_preview(generation, Ok(preview()));
    model
}
fn error(status: u16) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::OperationNotFound,
        message: "original cause".into(),
        retryable: false,
        status,
        request_id: Some("original-request".into()),
        source: None,
    }
}
fn unknown(model: &mut Model) -> Submission {
    model.acknowledged = true;
    let submission = model.begin_save().unwrap();
    model.finish_save(&submission, Err(error(503)));
    submission
}

#[test]
fn preview_binds_the_complete_review_actor_request_and_closed_effect() {
    let expected = preview();
    assert_eq!(
        response::preview(
            json!(expected),
            &expected.request,
            &expected.plan,
            &"admin".parse().unwrap(),
        )
        .unwrap(),
        expected
    );
    for field in 0..8 {
        let mut value = json!(expected);
        match field {
            0 => value["request"]["operation_id"] = json!(operation(3)),
            1 => value["plan"]["target"]["identity"]["binding_generation"] = json!("10"),
            2 => value["principal"] = json!("another-principal"),
            3 => value["intent_digest"] = json!("NOT-A-DIGEST"),
            4 => value["effect"] = json!("publish_namespace"),
            5 => value["execution"] = json!("available"),
            6 => {
                value["plan"]["source"]["histories"][0]["temporarily_unavailable_versions"] =
                    json!([])
            }
            _ => value["unexpected"] = json!(true),
        }
        assert!(
            response::preview(
                value,
                &expected.request,
                &expected.plan,
                &"admin".parse().unwrap(),
            )
            .is_err(),
            "case {field}"
        );
    }
}

#[test]
fn status_binds_exact_address_plan_and_complete_history_counts_without_success_coercion() {
    let expected = preview();
    assert_eq!(
        response::status(json!(status(&expected)), &expected).unwrap(),
        status(&expected)
    );
    for field in 0..9 {
        let mut value = json!(status(&expected));
        match field {
            0 => value["address"]["operation_id"] = json!(operation(9)),
            1 => value["address"]["target"]["disk_id"] = json!(format!("disk_{:032x}", 99)),
            2 => value["address"]["intent_digest"] = json!("a".repeat(64)),
            3 => value["plan"]["review_digest"] = json!("a".repeat(64)),
            4 => value["histories"] = json!(1),
            5 => value["temporarily_unavailable_versions"] = json!(0),
            6 => value["stage"] = json!("completed"),
            7 => value["execution"] = json!("available"),
            _ => value["target_reserved"] = json!(true),
        }
        assert!(response::status(value, &expected).is_err(), "case {field}");
    }
}

#[test]
fn saving_requires_fresh_explicit_confirmation_and_fences_duplicate_actions() {
    let mut model = model();
    assert!(model.begin_save().is_none());
    model.acknowledged = true;
    let submitted = model.begin_save().unwrap();
    assert!(submitted.request.confirm_save_intent);
    assert_eq!(submitted.request.preview, preview());
    assert!(model.blocks_edit());
    assert!(model.begin_save().is_none());
    assert!(model.begin_preview().is_none());
    assert!(model.begin_status().is_none());
}

#[test]
fn unknown_save_preserves_original_identity_across_not_found_and_rejected_retry() {
    let mut model = model();
    let original = unknown(&mut model);
    model.leave();
    assert!(model.blocks_edit());
    assert!(model.begin_preview().is_none());
    let (generation, expected) = model.begin_status().unwrap();
    assert_eq!(*expected, original.request.preview);
    model.finish_status(generation, Err(error(404)));
    assert_eq!(model.save, SaveState::Unknown);
    assert_eq!(
        model.error.as_ref().unwrap().request_id.as_deref(),
        Some("original-request")
    );
    let retry = model.begin_save().unwrap();
    assert_eq!(retry.request, original.request);
    model.finish_save(&retry, Err(error(403)));
    assert_eq!(model.save, SaveState::Unknown);
    assert!(model.blocks_edit());
    assert_eq!(model.begin_save().unwrap().request, original.request);
}

#[test]
fn explicit_first_rejection_requires_new_preview_and_confirmation() {
    let mut model = model();
    model.acknowledged = true;
    let submission = model.begin_save().unwrap();
    model.finish_save(&submission, Err(error(409)));
    assert_eq!(model.save, SaveState::Rejected);
    assert!(!model.acknowledged && !model.can_save());
    assert!(model.begin_preview().is_some());
    assert!(model.preview.is_none());
}

#[test]
fn navigation_does_not_cancel_an_already_submitted_target_save() {
    let mut model = model();
    model.acknowledged = true;
    let submitted = model.begin_save().unwrap();
    model.leave();
    assert_eq!(model.generation, submitted.generation);
    model.finish_save(&submitted, Ok(status(&submitted.request.preview)));
    assert_eq!(model.save, SaveState::Stored);
    assert!(!model.blocks_edit() && !model.can_save());
    assert!(!model.can_preview());
    assert_eq!(
        model.stored.as_ref().unwrap().stage,
        ForeignIntentStage::IntentSaved
    );
    assert_eq!(
        model.stored.as_ref().unwrap().execution,
        ForeignExecutionCapability::Unavailable
    );
}

#[test]
fn stale_preview_cannot_reenter_after_navigation_or_a_parent_reset() {
    let mut model = Model::default();
    let old = model.begin_preview().unwrap();
    model.leave();
    model.reset();
    let current = model.begin_preview().unwrap();
    assert!(current > old);
    model.finish_preview(old, Ok(preview()));
    assert!(model.preview.is_none() && model.previewing);
    model.finish_preview(current, Ok(preview()));
    assert!(model.preview.is_some() && !model.previewing);
}

#[test]
fn stale_status_cannot_settle_a_newer_query_or_erase_its_busy_fence() {
    let mut model = model();
    unknown(&mut model);
    let (old, expected) = model.begin_status().unwrap();
    model.leave();
    let (current, _) = model.begin_status().unwrap();
    model.finish_status(old, Ok(status(&expected)));
    assert!(model.reading && model.save == SaveState::Unknown);
    model.finish_status(current, Ok(status(&expected)));
    assert!(!model.reading && model.save == SaveState::Stored);
}

#[test]
fn malformed_success_keeps_unknown_save_and_exact_retry_material() {
    let mut model = model();
    model.acknowledged = true;
    let original = model.begin_save().unwrap();
    let mut value = json!(status(&original.request.preview));
    value["stage"] = json!("published");
    let result = response::status(value, &original.request.preview);
    assert!(result.is_err());
    model.finish_save(&original, result);
    assert_eq!(model.save, SaveState::Unknown);
    assert_eq!(model.begin_save().unwrap().request, original.request);
}

#[test]
fn unknown_intent_blocks_parent_edits_and_survives_source_panel_invalidation() {
    let review = preview().plan;
    let mut parent = super::super::Model::default();
    parent.accept_preview(review.clone());
    parent.accept_saved(ForeignPlanStatus {
        preview: review.clone(),
        validation: ForeignPlanValidation::Required,
    });
    parent.intent = model();
    let original = unknown(&mut parent.intent);
    parent.edit(|draft| draft.slot = "P88".into());
    assert!(parent.blocks_edit());
    assert!(parent.read_request(Read::Status).is_none());
    parent.leave();
    parent.invalidate();
    assert_eq!(parent.stored.unwrap().preview, review);
    assert_eq!(
        parent.intent.begin_save().unwrap().request,
        original.request
    );
}

#[test]
fn changed_parent_input_drops_only_unsubmitted_intent_and_keeps_generation_monotonic() {
    let mut parent = super::super::Model {
        intent: model(),
        ..Default::default()
    };
    let old = parent.intent.generation;
    parent.edit(|draft| draft.parent = "another-parent".into());
    assert!(parent.intent.generation > old);
    assert!(parent.intent.preview.is_none());
    assert!(!parent.intent.acknowledged);
}

#[test]
fn unknown_target_save_reaches_the_application_storage_reload_fence() {
    let mut storage = crate::admin::storage::Model::default();
    storage.foreign.plan.intent = model();
    unknown(&mut storage.foreign.plan.intent);
    assert!(storage.reload_blocked());
    assert!(!storage.editable());
    storage.foreign.invalidate();
    assert!(storage.reload_blocked());
    assert_eq!(storage.foreign.plan.intent.save, SaveState::Unknown);
}

#[test]
fn response_session_guard_rejects_replacement_sessions_and_disposed_owners() {
    let owner = Owner::new();
    let state = owner.with(|| {
        RwSignal::new(State {
            authenticated: true,
            principal: Some("admin".parse().unwrap()),
            session_epoch: 8,
            ..Default::default()
        })
    });
    let result: Result<ForeignIntentStatus, api::Error> = Ok(status(&preview()));
    assert!(accept_read_response(state, 8, &result));
    state.update(|state| {
        state.session_epoch += 1;
        state.principal = Some("replacement".parse().unwrap());
    });
    assert!(!accept_read_response(state, 8, &result));
    owner.cleanup();
    assert!(!accept_read_response(state, 9, &result));
}
