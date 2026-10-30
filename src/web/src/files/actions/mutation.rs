//! Namespace, branch and note transactions; writes never bypass the pending-save gate.
use super::{
    disk_path,
    operation::{record_operation, record_receipt},
    history_write_allowed, write_allowed,
};
use crate::{
    api,
    files::model::{FileOperation, NamespaceAction, SaveStatus, join_path},
    ui::{ErrorContext, State, current_session, fail_in, set_notice},
};
use json_codec::{Value, json};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn namespace(
    state: RwSignal<State>,
    action: NamespaceAction,
    name: String,
) -> Option<String> {
    let (mut body, endpoint) = state.with_untracked(|snapshot| {
        if !write_allowed(snapshot) {
            return None;
        }
        let body = match action {
            NamespaceAction::CreateDirectory => {
                json!({"kind":"mkdir","path":join_path(&snapshot.files.directory,&name)})
            }
        };
        Some((
            body,
            format!("{}/namespace", disk_path(&snapshot.files.slot)),
        ))
    })?;
    let id = crate::ui::operation_id(state)?;
    body["operation_id"] = id.clone().into();
    state.update(|s| {
        s.files.mutation_pending = true;
    });
    let epoch = state.with_untracked(|s| s.session_epoch);
    let submitted = id.clone();
    spawn_local(async move {
        let result = api::request("POST", &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                record_receipt(state, &id, value);
            }
            Err(error) => {
                record_failure(state, epoch, &id, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
    Some(submitted)
}
pub(crate) fn directory_branch(
    state: RwSignal<State>,
    path: String,
    name: String,
    select: bool,
) -> Option<String> {
    if !state.with_untracked(write_allowed) {
        return None;
    }
    let operation_id = crate::ui::operation_id(state)?;
    let slot = state.with_untracked(|s| s.files.slot.clone());
    let action = if select {
        "directory-select"
    } else {
        "directory-branches"
    };
    let endpoint = format!("{}/{action}", disk_path(&slot));
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| s.files.mutation_pending = true);
    let submitted = operation_id.clone();
    spawn_local(async move {
        let result = api::request(
            "POST",
            &endpoint,
            Some(json!({"path":path,"name":name,"operation_id":operation_id})),
        )
        .await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                let status = record_receipt(state, &operation_id, value);
                if matches!(status, SaveStatus::Published | SaveStatus::Degraded) {
                    set_notice(
                        state,
                        if select {
                            t(
                                "已恢复此目录的文件分支组合，目录外文件保持不变。",
                                "Restored this folder’s file branch combination; files outside the folder were left unchanged.",
                            )
                        } else {
                            t(
                                "目录分支组合已登记，尚未切换。",
                                "The folder branch combination was registered without switching to it.",
                            )
                        },
                    );
                }
            }
            Err(error) => {
                record_failure(state, epoch, &operation_id, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
    Some(submitted)
}

pub(crate) fn select_default_branch(state: RwSignal<State>, branch_id: String) {
    let repository = state.with_untracked(|s| s.files.repository_id.clone());
    if repository.is_empty() || branch_id.is_empty() {
        return;
    }
    let _ = record_mutation(
        state,
        "PUT",
        format!(
            "/api/repositories/{}/default-branch",
            api::segment(&repository)
        ),
        json!({"directory_branch_id":branch_id}),
    );
}
pub(crate) fn rename_directory_branch(
    state: RwSignal<State>,
    repository: String,
    branch_id: String,
    name: String,
) -> Option<String> {
    if repository.is_empty()
        || branch_id.is_empty()
        || name.len() > storage_contract::MAX_NOTE_BYTES
        || !crate::files::model::valid_name(&name)
    {
        return None;
    }
    record_mutation(
        state,
        "PATCH",
        format!(
            "/api/repositories/{}/directory-branches/{}",
            api::segment(&repository),
            api::segment(&branch_id)
        ),
        json!({"name":name}),
    )
}
pub(crate) fn rename_file_branch(
    state: RwSignal<State>,
    file: String,
    branch_id: String,
    name: String,
) -> Option<String> {
    if !state.with_untracked(history_write_allowed)
        || file.is_empty()
        || branch_id.is_empty()
        || name.len() > storage_contract::MAX_NOTE_BYTES
        || !crate::files::model::valid_name(&name)
    {
        return None;
    }
    record_mutation(
        state,
        "PATCH",
        format!(
            "/api/file-manager/files/{}/branches/{}",
            api::segment(&file),
            api::segment(&branch_id)
        ),
        json!({"name":name}),
    )
}
fn record_mutation(
    state: RwSignal<State>,
    method: &'static str,
    endpoint: String,
    mut body: Value,
) -> Option<String> {
    if !state.with_untracked(write_allowed) {
        return None;
    }
    let operation_id = crate::ui::operation_id(state)?;
    body["operation_id"] = operation_id.clone().into();
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| s.files.mutation_pending = true);
    let submitted = operation_id.clone();
    spawn_local(async move {
        let result = api::request(method, &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                record_receipt(state, &operation_id, value);
            }
            Err(error) => {
                record_failure(state, epoch, &operation_id, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
    Some(submitted)
}

pub(crate) fn update_note(
    state: RwSignal<State>,
    version: Option<String>,
    note: String,
) -> Option<String> {
    if storage_contract::validate_note(&note).is_err() {
        return None;
    }
    let file = state.with_untracked(|s| {
        s.files
            .selected
            .as_ref()
            .map(|entry| entry.file_id().to_owned())
            .unwrap_or_default()
    });
    let (collection, id) = match version {
        Some(version) => ("versions", version),
        None => ("files", file),
    };
    if id.is_empty() || !state.with_untracked(history_write_allowed) {
        return None;
    }
    let operation_id = crate::ui::operation_id(state)?;
    let endpoint = format!(
        "/api/file-manager/{collection}/{}/metadata",
        api::segment(&id)
    );
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.files.mutation_pending = true;
    });
    let submitted = operation_id.clone();
    spawn_local(async move {
        let result = api::request(
            "POST",
            &endpoint,
            Some(json!({"note":note,"operation_id":operation_id})),
        )
        .await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                record_receipt(state, &operation_id, value);
            }
            Err(error) => {
                record_failure(state, epoch, &operation_id, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
    Some(submitted)
}

pub(crate) fn update_node_note(
    state: RwSignal<State>,
    node: String,
    note: String,
) -> Option<String> {
    if !state.with_untracked(history_write_allowed)
        || node.is_empty()
        || storage_contract::validate_note(&note).is_err()
    {
        return None;
    }
    let file = state.with_untracked(|s| {
        s.files
            .selected
            .as_ref()
            .map(|entry| entry.file_id().to_owned())
            .unwrap_or_default()
    });
    if file.is_empty() {
        return None;
    }
    record_mutation(
        state,
        "PATCH",
        format!(
            "/api/file-manager/files/{}/nodes/{}",
            api::segment(&file),
            api::segment(&node)
        ),
        json!({"note":note}),
    )
}
pub(crate) fn create_branch(
    state: RwSignal<State>,
    version: String,
    node: Option<String>,
    name: String,
) -> Option<String> {
    if version.is_empty() {
        return None;
    }
    let file = state.with_untracked(|s| {
        s.files
            .selected
            .as_ref()
            .map(|entry| entry.file_id().to_owned())
            .unwrap_or_default()
    });
    if file.is_empty() {
        return None;
    }
    let mut body = json!({"source_version_id":version,"name":name});
    if let Some(node) = node.filter(|id| !id.is_empty()) {
        body["source_node_id"] = node.into();
    }
    file_mutation(state, &file, "branches", body)
}
pub(crate) fn select_branch(state: RwSignal<State>, branch: String) {
    let file = state.with_untracked(|s| {
        s.files
            .selected
            .as_ref()
            .map(|entry| entry.file_id().to_owned())
            .unwrap_or_default()
    });
    if file.is_empty() {
        return;
    }
    let _ = file_mutation(state, &file, "select", json!({"branch_id":branch}));
}
fn file_mutation(
    state: RwSignal<State>,
    file: &str,
    action: &str,
    mut body: Value,
) -> Option<String> {
    if !state.with_untracked(history_write_allowed) {
        return None;
    }
    let id = crate::ui::operation_id(state)?;
    if action == "select"
        && let Some((repository, path)) = state.with_untracked(|s| {
            s.files
                .selected
                .as_ref()
                .and_then(crate::files::model::file_context)
        })
    {
        body["repository_id"] = repository.into();
        body["path"] = path.into();
    }
    body["operation_id"] = id.clone().into();
    let endpoint = format!("/api/file-manager/files/{}/{action}", api::segment(file));
    state.update(|s| s.files.mutation_pending = true);
    let epoch = state.with_untracked(|s| s.session_epoch);
    let submitted = id.clone();
    spawn_local(async move {
        let result = api::request("POST", &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                record_receipt(state, &id, value);
            }
            Err(error) => {
                record_failure(state, epoch, &id, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
    Some(submitted)
}

/// Definite HTTP rejection releases only this operation; ambiguous transport or
/// malformed-success responses stay tied to the original ID for polling.
fn record_failure(state: RwSignal<State>, epoch: u64, id: &str, error: api::Error) {
    let unknown = error.mutation_outcome_unknown();
    let operation = if unknown {
        FileOperation::unknown(id)
    } else {
        FileOperation::rejected(id)
    };
    record_operation(state, operation);
    if !unknown {
        // This is a rejected HTTP request, not a server staging receipt.
        // Present its actual error without claiming any content was retained.
        set_notice(state, String::new());
    }
    fail_in(state, epoch, ErrorContext::Operation(id.to_owned()), error);
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/mutation/tests.rs"]
mod tests;
