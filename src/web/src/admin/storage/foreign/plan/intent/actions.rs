use super::*;

pub(super) fn preview(state: RwSignal<State>) {
    let Some(operation) = crate::ui::operation_id(state) else {
        return;
    };
    let captured = state
        .try_update(|state| {
            if !state.session_active() || !state.admin_console.storage.editable() {
                return None;
            }
            let plan = &mut state.admin_console.storage.foreign.plan;
            let stored = plan.stored.as_ref()?.preview.clone();
            let generation = plan.intent.begin_preview()?;
            let request = ForeignIntentRequest {
                operation_id: operation
                    .parse()
                    .expect("invariant: browser operation ID is canonical"),
                plan: ForeignPlanSelection {
                    operation_id: stored.request.operation_id,
                    review_digest: stored.review_digest.clone(),
                },
            };
            Some((
                state.session_epoch,
                state.principal.clone(),
                generation,
                request,
                stored,
            ))
        })
        .flatten();
    let Some((epoch, principal, generation, request, plan)) = captured else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancel) => cancel,
        Err(error) => {
            state.update(|state| {
                state
                    .admin_console
                    .storage
                    .foreign
                    .plan
                    .intent
                    .finish_preview(generation, Err(error))
            });
            return;
        }
    };
    state.update(|state| {
        state.admin_console.storage.foreign.plan.intent.cancellation = Some(cancellation.clone())
    });
    spawn_local(async move {
        let result = api::cancellable_request(
            "POST",
            "/api/admin/storage/foreign/execution-intents/preview",
            Some(json!(request)),
            cancellation,
        )
        .await
        .and_then(|value| response::preview(value, &request, &plan, &principal));
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            state
                .admin_console
                .storage
                .foreign
                .plan
                .intent
                .finish_preview(generation, result);
        });
    });
}

pub(super) fn save(state: RwSignal<State>) {
    let captured = state
        .try_update(|state| {
            if !state.session_active() {
                return None;
            }
            let storage = &mut state.admin_console.storage;
            if storage.foreign.plan.intent.save != SaveState::Unknown && !storage.editable() {
                return None;
            }
            let intent = &mut storage.foreign.plan.intent;
            if intent.preview.as_ref()?.principal != state.principal {
                return None;
            }
            Some((state.session_epoch, intent.begin_save()?))
        })
        .flatten();
    let Some((epoch, submission)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request(
            "POST",
            "/api/admin/storage/foreign/execution-intents/save",
            Some(json!(submission.request)),
            |value| response::status(value, &submission.request.preview),
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            state
                .admin_console
                .storage
                .foreign
                .plan
                .intent
                .finish_save(&submission, result)
        });
    });
}

pub(super) fn status(state: RwSignal<State>) {
    let captured = state
        .try_update(|state| {
            if !state.session_active() {
                return None;
            }
            let intent = &mut state.admin_console.storage.foreign.plan.intent;
            if intent.preview.as_ref()?.principal != state.principal {
                return None;
            }
            let (generation, expected) = intent.begin_status()?;
            Some((state.session_epoch, generation, expected))
        })
        .flatten();
    let Some((epoch, generation, expected)) = captured else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancel) => cancel,
        Err(error) => {
            state.update(|state| {
                state
                    .admin_console
                    .storage
                    .foreign
                    .plan
                    .intent
                    .finish_status(generation, Err(error))
            });
            return;
        }
    };
    state.update(|state| {
        state.admin_console.storage.foreign.plan.intent.cancellation = Some(cancellation.clone())
    });
    spawn_local(async move {
        let result = api::cancellable_request(
            "POST",
            "/api/admin/storage/foreign/execution-intents/status",
            Some(json!(query(&expected))),
            cancellation,
        )
        .await
        .and_then(|value| response::status(value, &expected));
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            state
                .admin_console
                .storage
                .foreign
                .plan
                .intent
                .finish_status(generation, result)
        });
    });
}
