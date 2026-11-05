use super::*;

pub(super) fn edit(state: RwSignal<State>, change: impl FnOnce(&mut Draft)) {
    state.update(|state| {
        if editable(state) {
            state.admin_console.storage.foreign.plan.edit(change);
        }
    });
}
pub(super) fn preview(state: RwSignal<State>) {
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    let operation = id
        .parse()
        .expect("invariant: browser operation IDs are canonical");
    let captured = state
        .try_update(|state| {
            if !editable(state) {
                return None;
            }
            let foreign = &mut state.admin_console.storage.foreign;
            let source = foreign
                .preview
                .as_ref()
                .expect("invariant: editable review has source evidence")
                .clone();
            let model = &mut foreign.plan;
            let request = match model
                .draft
                .request(&source, &model.known_primary, operation)
            {
                Ok(request) => request,
                Err(error) => {
                    model.error = Some(input_error(error));
                    return None;
                }
            };
            model.previewing = true;
            model.acknowledged = false;
            model.review = None;
            model.stored = None;
            model.error = None;
            model.generation += 1;
            Some((state.session_epoch, model.generation, request, source))
        })
        .flatten();
    let Some((epoch, generation, request, source)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request(
            "POST",
            "/api/admin/storage/foreign/plans/preview",
            Some(json!(request)),
            |value| read_preview(value, &request, &source),
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            let foreign = &mut state.admin_console.storage.foreign;
            if state.session_epoch != epoch
                || foreign.plan.generation != generation
                || foreign.preview.as_ref() != Some(&source)
            {
                return;
            }
            let model = &mut foreign.plan;
            model.previewing = false;
            match result {
                Ok(review) => model.accept_preview(review),
                Err(error) => model.error = Some(error),
            }
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
            if storage.foreign.plan.save != SaveState::Unknown && !storage.editable() {
                return None;
            }
            let model = &mut storage.foreign.plan;
            let request = model.save_request()?;
            model.generation += 1;
            Some((state.session_epoch, model.generation, request))
        })
        .flatten();
    let Some((epoch, generation, request)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request(
            "POST",
            "/api/admin/storage/foreign/plans/save",
            Some(json!(request)),
            |value| read_status(value, &request.preview, ForeignPlanValidation::Required),
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if state.session_epoch != epoch
                || state.admin_console.storage.foreign.plan.generation != generation
            {
                return;
            }
            let model = &mut state.admin_console.storage.foreign.plan;
            match result {
                Ok(status) => model.accept_saved(status),
                Err(error) => model.save_failed(error),
            }
        });
    });
}
#[derive(Clone, Copy)]
pub(super) enum Read {
    Status,
    Revalidate,
}
pub(super) fn load(state: RwSignal<State>, action: Read) {
    let captured = state
        .try_update(|state| {
            if !state.session_active() {
                return None;
            }
            let model = &mut state.admin_console.storage.foreign.plan;
            if model.reading
                || model.previewing
                || model.save == SaveState::Sending
                || !matches!(model.save, SaveState::Unknown | SaveState::Stored)
                || (matches!(action, Read::Revalidate) && model.save != SaveState::Stored)
            {
                return None;
            }
            let expected = model.review.clone()?;
            model.reading = true;
            model.generation += 1;
            model.error = None;
            Some((state.session_epoch, model.generation, expected))
        })
        .flatten();
    let Some((epoch, generation, expected)) = captured else {
        return;
    };
    spawn_local(async move {
        let selection = ForeignPlanSelection {
            operation_id: expected.request.operation_id,
            review_digest: expected.review_digest.clone(),
        };
        let (method, path, body, validation) = match action {
            Read::Status => (
                "GET",
                format!(
                    "/api/admin/storage/foreign/plans/status?operation_id={}",
                    selection.operation_id
                ),
                None,
                ForeignPlanValidation::Required,
            ),
            Read::Revalidate => (
                "POST",
                "/api/admin/storage/foreign/plans/revalidate".into(),
                Some(json!(selection)),
                ForeignPlanValidation::MatchedAtObservation,
            ),
        };
        let result = api::validated_request(method, &path, body, |value| {
            read_status(value, &expected, validation)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if state.session_epoch != epoch
                || state.admin_console.storage.foreign.plan.generation != generation
            {
                return;
            }
            let model = &mut state.admin_console.storage.foreign.plan;
            model.reading = false;
            match result {
                Ok(status) => model.accept_saved(status),
                // NotFound after an uncertain save is not proof of non-creation.
                Err(error) => model.read_failed(error),
            }
        });
    });
}
