//! Explicit re-protection of the selected current files with captured disk names.
mod capture;
mod view;
use crate::{
    api,
    files::{actions, model::file_context, review},
    ui::{State, accept_read_response, fail},
};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use std::collections::BTreeMap;
pub(crate) use view::view;
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone)]
pub(crate) struct Draft {
    id: String,
    repository: String,
    path: String,
    expected_scope: Option<String>,
    load: Load,
}
#[derive(Clone)]
enum Load {
    Pending,
    Ready(Box<Response>),
    Failed,
}
#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireResponse {
    configuration: Configuration,
    capture: capture::Capture,
}
#[derive(Clone)]
struct Response {
    configuration: Configuration,
    capture: capture::Capture,
    prefixes: BTreeMap<String, capture::Prefix>,
}
impl Response {
    fn prefixes(&self, copies: u32) -> BTreeMap<String, String> {
        self.prefixes
            .iter()
            .filter(|(_, prefix)| prefix.copies <= copies)
            .map(|(slot, prefix)| (slot.clone(), prefix.path.clone()))
            .collect()
    }
}
// Only the fields needed for this editor are projected from the configuration.
// The capture, unlike this read projection, is decoded and resent in full.
#[derive(Clone, Deserialize)]
struct Configuration {
    scope: Scope,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    inherited: bool,
    target_copies: u32,
    #[serialization(deserialize_with = "super::read_routes")]
    backup_routes: Vec<Vec<String>>,
    max_copies: usize,
    #[serialization(with = "storage_contract::u64_string")]
    file_count: u64,
    backup_paths: BTreeMap<String, String>,
}
#[derive(Clone, Deserialize)]
struct Scope {
    id: String,
    path: String,
}
impl Draft {
    fn response(&self) -> Option<&Response> {
        match &self.load {
            Load::Ready(response) => Some(response),
            Load::Pending | Load::Failed => None,
        }
    }
}
impl Configuration {
    fn capacity(&self) -> usize {
        self.max_copies
            .min(self.backup_routes.iter().map(Vec::len).min().unwrap_or(0))
    }
}
fn invalid(message: &str) -> api::Error {
    super::configuration_error(
        <json_codec::Error as data_serialization::de::Error>::custom(message),
    )
}
fn read_response(value: Value, draft: &Draft) -> Result<Response, api::Error> {
    let response = WireResponse::deserialize(value).map_err(super::configuration_error)?;
    let scope = &response.configuration.scope;
    if scope.id.is_empty()
        || draft
            .expected_scope
            .as_ref()
            .is_some_and(|id| id != &scope.id)
        || (!scope.path.is_empty() && name_policy::validate_path(&scope.path).is_err())
        || name_policy::path_key(&scope.path) != name_policy::path_key(&draft.path)
    {
        return Err(invalid(
            "recursive backup response belongs to another scope",
        ));
    }
    // Check all available routes before using their names in the review. A
    // reduced count can still be selected when the old count exceeds capacity.
    let prefixes = response
        .capture
        .prefixes(&response.configuration.backup_routes)
        .map_err(invalid)?;
    Ok(Response {
        configuration: response.configuration,
        capture: response.capture,
        prefixes,
    })
}
pub(crate) fn start_entry(state: RwSignal<State>, entry: crate::files::page::Entry) {
    if let Some((repository, path)) = file_context(&entry) {
        start(state, repository, path, Some(entry_scope(&entry).into()));
    }
}
fn entry_scope(entry: &crate::files::page::Entry) -> &str {
    match entry.kind() {
        storage_contract::NamespaceEntryKind::File => entry.file_id(),
        storage_contract::NamespaceEntryKind::Directory => entry.entry_id(),
    }
}
pub(crate) fn start_current(state: RwSignal<State>) {
    let (repository, path) = state.with_untracked(|s| {
        (
            s.files.repository_id.clone(),
            s.files.repository_path.clone(),
        )
    });
    if !repository.is_empty() {
        start(state, repository, path, None);
    }
}
fn start(state: RwSignal<State>, repository: String, path: String, expected_scope: Option<String>) {
    if state.with_untracked(|s| !s.session_active() || actions::pending(s)) {
        return;
    }
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let draft = Draft {
        id: id.clone(),
        repository,
        path,
        expected_scope,
        load: Load::Pending,
    };
    review::reset(state, review::Kind::Backup);
    state.update(|s| {
        s.files.backup = None;
        s.files.recursive_backup = Some(draft.clone());
    });
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            &format!(
                "/api/repositories/{}/recursive-backup-configuration?path={}",
                api::segment(&draft.repository),
                api::segment(&draft.path)
            ),
            None,
            |value| read_response(value, &draft),
        )
        .await;
        complete_load(state, epoch, &id, result);
    });
}
fn complete_load(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    result: Result<Response, api::Error>,
) {
    // A current-session 401 still revokes the session if the panel was closed.
    if !accept_read_response(state, epoch, &result)
        || !state.with_untracked(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .is_some_and(|draft| draft.id == id)
        })
    {
        return;
    }
    match result {
        Ok(response) => state.update(|s| {
            if let Some(draft) = s.files.recursive_backup.as_mut() {
                draft.load = Load::Ready(Box::new(response));
            }
        }),
        Err(error) => {
            state.update(|s| {
                if let Some(draft) = s.files.recursive_backup.as_mut() {
                    draft.load = Load::Failed;
                }
            });
            review::error(state, review::Kind::Backup, error.to_string());
            fail(state, error);
        }
    }
}
fn close(state: RwSignal<State>) {
    if state.with_untracked(actions::pending) {
        return;
    }
    state.update(|s| s.files.recursive_backup = None);
    review::reset(state, review::Kind::Backup);
}
fn reload(state: RwSignal<State>) {
    if let Some(draft) = state.with_untracked(|s| s.files.recursive_backup.clone()) {
        start(state, draft.repository, draft.path, draft.expected_scope);
    }
}
fn select_copies(state: RwSignal<State>, value: String) {
    if !state.with_untracked(actions::write_allowed) {
        return;
    }
    let Ok(copies) = value.parse::<u32>() else {
        return;
    };
    state.update(|s| {
        if let Some(Draft {
            load: Load::Ready(response),
            ..
        }) = s.files.recursive_backup.as_mut()
            && copies > 0
            && copies as usize <= response.configuration.capacity()
        {
            response.configuration.target_copies = copies;
        }
    });
}
fn request(draft: &Draft) -> Result<Value, String> {
    let Some(response) = draft.response() else {
        return Err(t(
            "先读取当前来源与路径，再保存。",
            "Load the current sources and paths before saving.",
        ));
    };
    let configuration = &response.configuration;
    if configuration.file_count == 0 {
        return Err(t(
            "此范围没有文件；空目录独立备份尚未提供。",
            "This scope has no files. Standalone empty-directory backups are not yet supported.",
        ));
    }
    if configuration.target_copies == 0
        || configuration.target_copies as usize > configuration.capacity()
    {
        return Err(t(
            "所选份数超出这条备份路径的能力。",
            "The copy count exceeds this backup route’s capacity.",
        ));
    }
    Ok(
        json!({"path":draft.path,"target_copies":configuration.target_copies,"expected_generation":configuration.generation.to_string(),"expected_scope_id":configuration.scope.id,"capture":response.capture}),
    )
}
fn submit(state: RwSignal<State>) {
    if !state.with_untracked(actions::write_allowed) {
        return;
    }
    let Some(draft) = state.with_untracked(|s| s.files.recursive_backup.clone()) else {
        return;
    };
    match request(&draft) {
        Ok(body) => review::submit(
            state,
            review::Kind::Backup,
            "PUT",
            format!(
                "/api/repositories/{}/recursive-backup-configuration",
                api::segment(&draft.repository)
            ),
            body,
        ),
        Err(error) => review::error(state, review::Kind::Backup, error),
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/backup/recursive_tests.rs"]
mod tests;
