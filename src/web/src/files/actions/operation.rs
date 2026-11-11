//! Publication receipts and polling of unconfirmed operations.
use super::browse::reload_entries;
use crate::{
    api,
    files::{FileOperation, model::SaveStatus},
    ui::{ErrorContext, State, current_session, fail_in},
};
use json_codec::Value;
use web_interface::prelude::*;
use web_interface::tasks::spawn_local;

/// Only a complete current receipt can settle its matching operation. Malformed
/// success responses retain the original ID, draft and write fence for polling.
pub(crate) fn record_receipt(state: RwSignal<State>, id: &str, value: Value) -> SaveStatus {
    let operation = if state.with_untracked(|s| {
        crate::files::review::receipt_kind_matches(s, id, &value)
            && crate::files::node_retirement::receipt_matches(s, id, &value)
            && crate::files::deletion::receipt_matches(s, id, &value)
            && crate::files::mkdir::receipt_matches(s, id, &value)
    }) {
        FileOperation::from_receipt(id, &value)
    } else {
        FileOperation::unknown(id)
    };
    record_operation(state, operation);
    state.with_untracked(|s| {
        s.files
            .operations
            .iter()
            .find(|operation| operation.id() == id)
            .expect("invariant: recording retains the observed operation")
            .status()
    })
}

pub(crate) fn record_operation(state: RwSignal<State>, operation: FileOperation) {
    let id = operation.id().to_owned();
    let status = operation.status();
    let mut reload_scope = false;
    let mut recorded = false;
    state.update(|s| {
        if s.files
            .operations
            .iter()
            .any(|previous| previous.id() == id && previous.status() != SaveStatus::Unknown)
        {
            return;
        }
        recorded = true;
        s.files.operations.retain(|operation| operation.id() != id);
        s.files.operations.insert(0, operation);
        s.files.operations.truncate(100);
        reload_scope = crate::files::batch::settle_operation(&mut s.files, &id, status);
        crate::files::review::settle(&mut s.files, &id, status);
        crate::files::deletion::settle(&mut s.files, &id, status);
        crate::files::mkdir::settle(&mut s.files.mkdir, &id, status);
        s.notices.record_operation(s.session_epoch, &id, status);
        if s.files.default_operation.as_deref() == Some(&id) {
            if matches!(status, SaveStatus::Published | SaveStatus::Degraded)
                && crate::files::mkdir::default_completion_applies(s, &id)
            {
                s.files.directory_preview = None;
                s.files.file_preview = None;
            }
            if status != SaveStatus::Unknown {
                s.files.default_operation = None;
            }
        }
    });
    if recorded && status != SaveStatus::Unknown {
        crate::files::deletion::finish_recovery(state, &id);
        crate::files::mkdir::finish(state, &id);
    }
    if recorded && matches!(status, SaveStatus::Published | SaveStatus::Degraded) {
        reload_entries(state);
    }
    if reload_scope {
        crate::files::batch::reload_registered_scope(state);
    }
}
/// Project only scheduling identities; never clone histories, drafts or State.
fn poll_work(state: &State) -> Option<(u64, Vec<String>)> {
    if !state.session_active() {
        return None;
    }
    let ids = state
        .files
        .operations
        .iter()
        .filter(|operation| operation.status() == SaveStatus::Unknown)
        .filter(|operation| {
            !crate::files::review::tag_attempt_in_flight(state, operation.id())
                && !crate::files::node_retirement::in_flight(state, operation.id())
                && !crate::files::deletion::in_flight(state, operation.id())
                && crate::files::mkdir::poll_allowed(state, operation.id())
        })
        .filter_map(|operation| {
            let id = operation.id();
            (!state
                .files
                .polling_operations
                .iter()
                .any(|pending| pending == id))
            .then(|| id.to_owned())
        })
        .collect();
    Some((state.session_epoch, ids))
}

pub(crate) fn poll(state: RwSignal<State>) {
    let Some((epoch, operations)) = state.with_untracked(poll_work) else {
        return;
    };
    for id in operations {
        let endpoint = state.with_untracked(|s| {
            crate::files::deletion::outcome_endpoint(s, &id)
                .unwrap_or_else(|| format!("/api/operations/{}/outcome", api::segment(&id)))
        });
        state.update(|s| s.files.polling_operations.push(id.clone()));
        spawn_local(async move {
            let result = api::request("GET", &endpoint, None).await;
            complete_poll(state, epoch, &id, result);
        });
    }
}

fn complete_poll(state: RwSignal<State>, epoch: u64, id: &str, result: Result<Value, api::Error>) {
    if !current_session(state, epoch) {
        return;
    }
    let pending = state.with_untracked(|s| {
        s.files
            .operations
            .iter()
            .any(|operation| operation.id() == id && operation.status() == SaveStatus::Unknown)
    });
    if !pending {
        if let Err(error) = result
            && error.status == 401
        {
            fail_in(state, epoch, ErrorContext::Operation(id.to_owned()), error);
        }
        state.update(|s| s.files.polling_operations.retain(|item| item != id));
        return;
    }
    match result {
        Ok(value) => {
            record_receipt(state, id, value);
        }
        Err(error) => fail_in(state, epoch, ErrorContext::Operation(id.to_owned()), error),
    }
    state.update(|s| s.files.polling_operations.retain(|item| item != id));
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/operation/tests.rs"]
mod tests;
