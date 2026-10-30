//! Publication receipts and polling of unconfirmed operations.
use super::browse::reload_entries;
use crate::{
    api,
    files::{
        FileOperation,
        model::{SaveStatus, publication_notice},
    },
    ui::{ErrorContext, State, current_session, fail_in},
};
use json_codec::Value;
use web_interface::prelude::*;
use web_interface::tasks::spawn_local;

/// Only a complete current receipt can settle its matching operation. Malformed
/// success responses retain the original ID, draft and write fence for polling.
pub(crate) fn record_receipt(state: RwSignal<State>, id: &str, value: Value) -> SaveStatus {
    let operation = FileOperation::from_receipt(id, &value);
    let status = operation.status();
    record_operation(state, operation);
    status
}

pub(crate) fn record_operation(state: RwSignal<State>, operation: FileOperation) {
    let id = operation.id().to_owned();
    let status = operation.status();
    let mut reload_scope = false;
    state.update(|s| {
        s.files.operations.retain(|operation| operation.id() != id);
        s.files.operations.insert(0, operation);
        s.files.operations.truncate(100);
        reload_scope = crate::files::batch::settle_operation(&mut s.files, &id, status);
        crate::files::review::settle(&mut s.files, &id, status);
        s.notice = publication_notice(status);
    });
    if matches!(status, SaveStatus::Published | SaveStatus::Degraded) {
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
        state.update(|s| s.files.polling_operations.push(id.clone()));
        spawn_local(async move {
            let result = api::request(
                "GET",
                &format!("/api/operations/{}/outcome", api::segment(&id)),
                None,
            )
            .await;
            complete_poll(state, epoch, &id, result);
        });
    }
}

fn complete_poll(state: RwSignal<State>, epoch: u64, id: &str, result: Result<Value, api::Error>) {
    if !current_session(state, epoch) {
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
