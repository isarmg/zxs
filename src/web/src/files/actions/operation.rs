//! Publication receipts and polling of unconfirmed operations.
use super::browse::reload_entries;
use crate::{
    api,
    files::model::{SaveStatus, operation_status, publication_notice},
    ui::{State, current_session, fail, text},
};
use json_codec::{Value, json};
use web_interface::prelude::*;
use web_interface::tasks::spawn_local;

/// Only a complete current receipt can settle its matching operation. Malformed
/// success responses retain the original ID, draft and write fence for polling.
pub(crate) fn record_receipt(state: RwSignal<State>, id: &str, value: Value) -> SaveStatus {
    let value = receipt(id, value);
    let status = operation_status(&value);
    record_operation(state, value);
    status
}
pub(super) fn receipt(id: &str, value: Value) -> Value {
    if !id.is_empty()
        && value["operation_id"].as_str() == Some(id)
        && storage_contract::client_protocol::operation_status(&value).is_some()
    {
        value
    } else {
        json!({"operation_id":id,"state":"in_doubt"})
    }
}

pub(crate) fn record_operation(state: RwSignal<State>, value: Value) {
    let id = text(&value, &["operation_id"]);
    let status = operation_status(&value);
    let mut reload_scope = false;
    state.update(|s| {
        if !id.is_empty() {
            s.files
                .operations
                .retain(|operation| text(operation, &["operation_id"]) != id);
            s.files.operations.insert(0, value);
            s.files.operations.truncate(100);
        }
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
        .filter(|operation| operation_status(operation) == SaveStatus::Unknown)
        .filter_map(|operation| {
            let id = operation["operation_id"]
                .as_str()
                .filter(|id| !id.is_empty())?;
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
                &format!("/api/operations/{}", api::segment(&id)),
                None,
            )
            .await;
            if !current_session(state, epoch) {
                return;
            }
            match result {
                Ok(value) => {
                    record_receipt(state, &id, value);
                }
                Err(error) => {
                    if error.status == 401 {
                        fail(state, error)
                    }
                }
            }
            state.update(|s| s.files.polling_operations.retain(|item| item != &id));
        });
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/operation/tests.rs"]
mod tests;
