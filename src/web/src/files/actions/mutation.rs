//! Namespace, branch and note transactions; writes never bypass the pending-save gate.
use super::{
    disk_path, history_write_allowed,
    operation::{record_operation, record_receipt},
    write_allowed,
};
use crate::{
    api,
    files::model::{FileOperation, NamespaceAction, join_path},
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
    let id = crate::ui::file_operation_id(state)?;
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
) -> Option<String> {
    if !state.with_untracked(|state| {
        write_allowed(state)
            && state.files.directory_branches_supported
            && state.files.directory == path
    }) {
        return None;
    }
    let operation_id = crate::ui::file_operation_id(state)?;
    state.update(|s| {
        s.notices.operation_success(t(
            "目录分支组合已登记，尚未切换。",
            "The folder branch combination was registered without switching to it.",
        ));
    });
    let slot = state.with_untracked(|s| s.files.slot.clone());
    let endpoint = format!("{}/directory-branches", disk_path(&slot));
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

pub(crate) fn select_default_branch(state: RwSignal<State>, branch_id: String) {
    let request = state.with_untracked(|s| {
        if !super::default_write_allowed(s)
            || !s.files.loaded
            || !s.files.directory_branches_supported
        {
            return None;
        }
        let preview = s.files.directory_preview.as_ref()?;
        let branch = preview.branch()?;
        if !preview.at_directory(&s.files.directory) || branch.id() != branch_id {
            return None;
        }
        Some((
            s.files.repository_id.clone(),
            branch.head_tree_id().to_owned(),
            s.files.namespace_head.clone(),
        ))
    });
    let Some((repository, tree, head)) = request else {
        return;
    };
    publish_default(
        state,
        "PUT",
        format!(
            "/api/repositories/{}/default-branch",
            api::segment(&repository)
        ),
        json!({"directory_branch_id":branch_id,"expected_tree_id":tree,"expected_head":head}),
    );
}

fn publish_default(
    state: RwSignal<State>,
    method: &'static str,
    endpoint: String,
    mut body: Value,
) {
    if !state.with_untracked(super::default_write_allowed) {
        return;
    }
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    body["operation_id"] = id.clone().into();
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.files.default_operation = Some(id.clone());
        s.files.mutation_pending = true;
        s.notices.operation_success(t(
            "默认读取视图已更新；已启用的 SMB 共享使用该视图。",
            "The default view is updated. Enabled SMB shares use this view.",
        ));
    });
    spawn_local(async move {
        let result = api::request(method, &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                record_receipt(state, &id, value);
            }
            Err(error) => record_failure(state, epoch, &id, error),
        }
        state.update(|s| s.files.mutation_pending = false);
    });
}

pub(crate) fn initialize_directory_git(
    state: RwSignal<State>,
    repository: String,
    path: String,
    expected_namespace_head: String,
) -> Option<String> {
    record_mutation(
        state,
        "POST",
        format!(
            "/api/repositories/{}/directory-git/initialize",
            api::segment(&repository)
        ),
        json!({"path":path,"expected_namespace_head":expected_namespace_head}),
    )
}
pub(crate) fn stage_directory_child(
    state: RwSignal<State>,
    repository: String,
    directory: String,
    branch: String,
    expected_index_generation: u64,
    child_directory: String,
    expected_child_head: String,
) -> Option<String> {
    record_mutation(
        state,
        "POST",
        format!(
            "/api/repositories/{}/directory-git/children/stage",
            api::segment(&repository)
        ),
        json!({"directory_id":directory,"directory_branch_id":branch,"expected_index_generation":expected_index_generation.to_string(),
            "child_directory_id":child_directory,"expected_child_head_tree_id":expected_child_head}),
    )
}

pub(crate) fn stage_directory_owned(
    state: RwSignal<State>,
    repository: String,
    directory: String,
    branch: String,
    tree: String,
    expected_index_generation: u64,
) -> Option<String> {
    record_mutation(
        state,
        "POST",
        format!(
            "/api/repositories/{}/directory-git/stage",
            api::segment(&repository)
        ),
        json!({"directory_id":directory,"directory_branch_id":branch,"expected_tree_id":tree,"expected_index_generation":expected_index_generation.to_string()}),
    )
}

pub(crate) fn commit_directory(
    state: RwSignal<State>,
    repository: String,
    branch: String,
    expected_tree: String,
    expected_index_generation: u64,
    message: String,
) -> Option<String> {
    if message.trim().is_empty() || message.len() > storage_contract::MAX_NOTE_BYTES {
        return None;
    }
    record_mutation(
        state,
        "POST",
        format!(
            "/api/repositories/{}/directory-branches/{}/commits",
            api::segment(&repository),
            api::segment(&branch)
        ),
        json!({"expected_tree_id":expected_tree,"expected_index_generation":expected_index_generation.to_string(),"message":message}),
    )
}

pub(crate) fn rename_directory_branch(
    state: RwSignal<State>,
    repository: String,
    branch_id: String,
    name: String,
) -> Option<String> {
    if !state.with_untracked(|state| {
        write_allowed(state)
            && state.files.directory_branches_supported
            && state.files.repository_id == repository
    }) || repository.is_empty()
        || branch_id.is_empty()
        || name.len() > storage_contract::MAX_NOTE_BYTES
    {
        return None;
    }
    if let Err(source) = crate::files::model::validate_name(&name) {
        let error = crate::files::naming::Error::new(&name, source);
        set_notice(state, error.message());
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
    {
        return None;
    }
    if let Err(source) = crate::files::model::validate_name(&name) {
        let error = crate::files::naming::Error::new(&name, source);
        set_notice(state, error.message());
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
    let operation_id = crate::ui::file_operation_id(state)?;
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
    let operation_id = crate::ui::file_operation_id(state)?;
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
/// File branch selection changes the inspected history node only.
pub(crate) fn select_branch(state: RwSignal<State>, branch: String) {
    let target = state.with_untracked(|s| {
        if !super::history_navigation_ready(s) || super::history_aux_loading(s) {
            return None;
        }
        s.files
            .file_history
            .branches()
            .iter()
            .chain(s.files.file_history.current_branch())
            .find(|item| item.id() == branch)
            .cloned()
    });
    let Some(target) = target else {
        return;
    };
    let head = target.head().map(str::to_owned);
    state.update(|s| {
        s.files.file_history.browse_branch(&target);
        s.files.file_preview = Some(target);
    });
    if let Some(head) = head {
        super::anchor_history_node(state, head);
    }
}

pub(crate) fn set_default_file_branch(state: RwSignal<State>) {
    let request = state.with_untracked(|s| {
        if !super::default_write_allowed(s)
            || !s.files.file_history.ready()
            || s.files.history_loading
            || s.files.directory_preview.is_some()
        {
            return None;
        }
        let choice = s
            .files
            .file_preview
            .as_ref()
            .filter(|choice| choice.writable())?;
        let node = choice.head()?;
        let entry = s
            .files
            .selected
            .as_ref()
            .filter(|entry| !entry.read_only())?;
        let (repository, path) = entry.context()?;
        Some((
            entry.file_id().to_owned(),
            choice.id().to_owned(),
            repository.to_owned(),
            path.to_owned(),
            node.to_owned(),
            s.files.namespace_head.clone(),
        ))
    });
    let Some((file, branch, repository, path, node, head)) = request else {
        return;
    };
    publish_default(
        state,
        "PUT",
        format!(
            "/api/file-manager/files/{}/default-branch",
            api::segment(&file)
        ),
        json!({"branch_id":branch,"repository_id":repository,"path":path,"expected_node_id":node,"expected_head":head}),
    );
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
    let id = crate::ui::file_operation_id(state)?;
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
        state.update(|s| s.notices.clear_operation_notice(id));
    }
    fail_in(state, epoch, ErrorContext::Operation(id.to_owned()), error);
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/actions/mutation/tests.rs"]
mod tests;
