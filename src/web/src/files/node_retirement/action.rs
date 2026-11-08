use super::*;

pub(super) fn preview(state: RwSignal<State>) {
    let captured = state.with_untracked(|snapshot| {
        if !actions::history_write_allowed(snapshot) { return None; }
        let model = &snapshot.files.node_retirement;
        let target = Target::capture(snapshot)?;
        if model.loading || model.attempt.is_some() || model.target.as_ref() != Some(&target) { return None; }
        Some((target, model.selection()?))
    });
    let Some((target, selection)) = captured else { return; };
    let generation = state.try_update(|s| {
        let model = &mut s.files.node_retirement;
        model.generation += 1; model.loading = true; model.preview = None; model.confirmed = false; model.error = None;
        model.generation
    }).expect("invariant: mounted detail owns its workflow");
    spawn_local(async move {
        let result = api::request("POST", &format!("{}/preview", selection.route()), Some(json!({
            "history_root": selection.history_root(), "version_ids": selection.version_ids()
        }))).await.and_then(|value| {
            let preview: Preview = json_codec::from_value(value).map_err(DecodeError::Wire)?;
            preview.validate(&selection).map_err(DecodeError::Name)?;
            Ok(preview)
        });
        if !crate::ui::accept_read_response(state, target.epoch, &result) { return; }
        state.update(|s| {
            if Target::capture(s).as_ref() != Some(&target) { return; }
            let model = &mut s.files.node_retirement;
            if model.generation != generation || model.target.as_ref() != Some(&target) { return; }
            model.loading = false;
            match result { Ok(preview) => model.preview = Some(preview), Err(error) => model.error = Some(error) }
        });
    });
}

pub(super) fn submit(state: RwSignal<State>) {
    let request = state.with_untracked(|snapshot| {
        if !actions::history_write_allowed(snapshot) { return None; }
        let model = &snapshot.files.node_retirement;
        let target = Target::capture(snapshot)?;
        let selection = model.selection()?;
        let preview = model.preview.as_ref()?;
        if model.target.as_ref() != Some(&target) || model.loading || model.attempt.is_some() || !model.confirmed
            || preview.validate(&selection).is_err() || !preview.affected_branch_heads.is_empty() || !preview.frozen_files.is_empty() { return None; }
        Some((target, selection))
    });
    let Some((target, selection)) = request else { return; };
    let Some(operation) = crate::ui::file_operation_id(state) else { return; };
    let attempt = Attempt { target, selection, operation: operation.clone(), in_flight: false, uncertain: false, cleanup: None, separation: None };
    state.update(|s| {
        s.files.node_retirement.attempt = Some(attempt); s.files.node_retirement.confirmed = false;
        s.notices.operation_success(t("节点退役已发布；正文清理请查看逐副本结果，旧认证证据仍保留。", "Node retirement was published. See per-replica body cleanup results; old authentication evidence remains."));
    });
    actions::record_operation(state, FileOperation::unknown(&operation));
    send(state, false, true);
}

pub(super) fn retry(state: RwSignal<State>) {
    let resume = state.with_untracked(|s| {
        let attempt = s.files.node_retirement.attempt.as_ref()?;
        if !attempt.target.same_file(s) || !s.management.storage.writable() || s.files.mutation_pending || attempt.in_flight { return None; }
        let status = status(s, attempt);
        (status == SaveStatus::Unknown || published(status)).then_some(published(status))
    });
    if let Some(resume) = resume { send(state, resume, false); }
}
fn send(state: RwSignal<State>, resume: bool, first: bool) {
    let attempt = state.try_update(|s| {
        let attempt = s.files.node_retirement.attempt.as_mut()?;
        if attempt.in_flight { return None; }
        attempt.in_flight = true;
        s.files.mutation_pending = true;
        s.files.node_retirement.error = None;
        Some(attempt.clone())
    }).flatten();
    let Some(attempt) = attempt else { return; };
    // A new observation replaces the previous interval observation. In particular,
    // an unreachable retry must not leave an old absence result looking current.
    state.update(|s| {
        if let Some(attempt) = &mut s.files.node_retirement.attempt {
            attempt.cleanup = None;
        }
    });
    let mut endpoint = attempt.selection.route();
    let body = if resume {
        endpoint.push_str("/resume");
        json!({"operation_id": attempt.operation})
    } else {
        json!({"history_root": attempt.selection.history_root(), "version_ids": attempt.selection.version_ids(),
            "operation_id": attempt.operation, "confirmation": wire::CONFIRMATION})
    };
    spawn_local(async move {
        let result = api::request("POST", &endpoint, Some(body)).await;
        if !current_session(state, attempt.target.epoch) { return; }
        if !state.with_untracked(|s| s.files.node_retirement.attempt.as_ref().is_some_and(|current| current.operation == attempt.operation)) { return; }
        match result {
            Ok(value) => receive(state, &attempt, value),
            Err(error) => {
                // A rejected retry cannot prove an earlier uncertain attempt
                // did not publish. Never mint a new ID or clear that fence.
                let uncertain = !first || attempt.uncertain || error.mutation_outcome_unknown();
                if !resume {
                    actions::record_operation(state, if uncertain { FileOperation::unknown(&attempt.operation) } else { FileOperation::rejected(&attempt.operation) });
                }
                state.update(|s| {
                    let model = &mut s.files.node_retirement;
                    model.error = Some(error.clone());
                    if let Some(attempt) = &mut model.attempt { attempt.uncertain |= uncertain; }
                });
                if first && !uncertain { state.update(|s| s.notices.clear_operation_notice(&attempt.operation)); }
                fail_in(state, attempt.target.epoch, ErrorContext::Operation(attempt.operation.clone()), error);
            }
        }
        state.update(|s| {
            s.files.mutation_pending = false;
            if let Some(attempt) = &mut s.files.node_retirement.attempt { attempt.in_flight = false; }
        });
        if state.with_untracked(|s| s.files.node_retirement.attempt.as_ref().is_some_and(|a| published(status(s, a)))) {
            state.update(|s| s.files.file_history.invalidate());
            actions::load_file_history(state);
        }
    });
}
fn receive(state: RwSignal<State>, attempt: &Attempt, value: Value) {
    // Retain a valid publication even if the separate cleanup response is bad.
    let receipt = value.get("operation").cloned().unwrap_or(Value::Null);
    let response_status = FileOperation::from_receipt(&attempt.operation, &receipt).status();
    let response_matches = state.with_untracked(|s| receipt_matches(s, &attempt.operation, &receipt));
    actions::record_receipt(state, &attempt.operation, receipt);
    if !response_matches || !published(response_status) {
        state.update(|s| {
            s.files.node_retirement.error = Some(malformed("Publication receipt did not confirm this exact original operation. Keep the original operation ID and retry; do not submit a new selection."));
            if let Some(attempt) = &mut s.files.node_retirement.attempt { attempt.uncertain = true; }
        });
        return;
    }
    let result = json_codec::from_value::<wire::Outcome>(value).map_err(|source| api::Error::from(DecodeError::Wire(source)))
        .and_then(|outcome| {
            if outcome.file_id != *attempt.selection.file_id() { return Err(malformed("Retirement cleanup names another file")); }
            outcome.cleanup.validate_for_receipt(attempt.selection.version_ids(), &outcome.operation).map_err(DecodeError::Name)?;
            outcome.separation.validate(&attempt.selection).map_err(DecodeError::Name)?;
            Ok((outcome.cleanup, outcome.separation))
        });
    state.update(|s| {
        let model = &mut s.files.node_retirement;
        match result {
            Ok((cleanup, separation)) => {
                if let Some(attempt) = &mut model.attempt {
                    attempt.cleanup = Some(cleanup);
                    attempt.separation = Some(separation);
                }
                model.error = None;
            }
            Err(error) => model.error = Some(error),
        }
        model.preview = None; model.selected.clear(); model.confirmed = false;
    });
}
pub(super) fn dismiss(state: RwSignal<State>) {
    let can_close = state.with_untracked(|s| s.files.node_retirement.attempt.as_ref().is_some_and(|attempt| {
        !attempt.in_flight && (status(s, attempt) == SaveStatus::Rejected
            || published(status(s, attempt)) && attempt.cleanup.as_ref().is_some_and(Cleanup::all_required_bindings_absent))
    }));
    if can_close { state.update(|s| { s.files.node_retirement.attempt = None; s.files.node_retirement.error = None; }); }
}
