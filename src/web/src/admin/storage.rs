//! First storage activation is independent from administrator account creation.
//! Submitted activation survives navigation and can only settle from real evidence.
use crate::{
    api,
    ui::{State, accept_read_response},
};
use data_serialization::{Deserialize, de::DeserializeOwned};
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::storage_directories::CandidateSelection;
use web_interface::{components::ErrorState, i18n::t, prelude::*, tasks::spawn_local};

mod addition;
mod conversion;
mod foreign;
pub(super) mod lifecycle;
pub(super) mod readmission;
mod response;
mod wizard;
#[derive(Debug, error_definition::Error)]
pub enum ResponseError {
    #[error("invalid storage directory response: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("storage activation response was not confirmed: {0}")]
    Activation(#[source] response::ResponseError),
    #[error("storage response identity differs from the request")]
    Identity,
}
use response::Success;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum StorageState {
    Pending,
    Active,
    RecoveryRequired,
    Unavailable,
    TerminalManagement,
}

#[derive(Clone, Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Snapshot {
    pub state: StorageState,
    #[serialization(rename = "disk_ids")]
    _disk_ids: Vec<String>,
    pub completion_available: bool,
    pub allow_development: bool,
    #[serialization(deserialize_with = "nullable_terminal")]
    pub terminal: Option<storage_contract::storage_lifecycle::TerminalStorageStatus>,
}
fn nullable_terminal<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<storage_contract::storage_lifecycle::TerminalStorageStatus>, D::Error> {
    Option::deserialize(deserializer)
}
fn storage_snapshot(value: Value) -> Result<Snapshot, api::Error> {
    let snapshot: Snapshot = decode(value)?;
    let terminal_state_matches = match &snapshot.terminal {
        Some(terminal) => {
            snapshot.state
                == if terminal.pending_retirement.is_some() {
                    StorageState::RecoveryRequired
                } else {
                    StorageState::TerminalManagement
                }
        }
        None => snapshot.state != StorageState::TerminalManagement,
    };
    if !terminal_state_matches
        || snapshot.terminal.as_ref().is_some_and(|terminal| {
            terminal.original_metadata_members == 0
                || storage_contract::validate_id(&terminal.pool_id, "pool").is_err()
                || terminal.authority_hash.len() != 64
                || !terminal
                    .authority_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || terminal.original_metadata_quorum != terminal.original_metadata_members / 2 + 1
                || terminal.registered_directories != snapshot._disk_ids.len()
                || (terminal.registrations
                    == storage_contract::storage_lifecycle::TerminalRegistrations::EmptyRegistry)
                    != (terminal.registered_directories == 0)
                || terminal.pending_retirement.as_ref().is_some_and(|pending| {
                    terminal.completed_retirement == Some(pending.operation_id)
                        || pending.pool_id != terminal.pool_id
                        || !snapshot._disk_ids.contains(&pending.disk_id)
                        || pending.slot_id.is_empty()
                        || pending.binding_generation == 0
                        || pending.principal.is_empty()
                        || pending.intent_digest.len() != 64
                        || !pending
                            .intent_digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                || snapshot.completion_available
        })
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(snapshot)
}

#[derive(Clone, Default)]
pub(super) struct Model {
    snapshot: Option<Snapshot>,
    generation: u64,
    busy: bool,
    mutation: bool,
    unknown: bool,
    success: Option<Success>,
    error: Option<api::Error>,
    activation_error: Option<api::Error>,
    selections: Vec<CandidateSelection>,
    development: bool,
    confirmed: bool,
    wizard: wizard::Model,
    addition: addition::Model,
    conversion: conversion::Model,
    foreign: foreign::Model,
    lifecycle: lifecycle::Model,
    readmission: readmission::Model,
}
impl Model {
    pub(super) fn reload_blocked(&self) -> bool {
        self.mutation
            || self.unknown
            || (self.addition.blocks_edit()
                || self.lifecycle.blocks_edit()
                || self.readmission.blocks_edit()
                || self.conversion.blocks_edit()
                || self.foreign.blocks_edit())
    }
    fn editable(&self) -> bool {
        !self.busy
            && !self.wizard.busy()
            && !self.wizard.failed()
            && !(self.addition.blocks_edit()
                || self.lifecycle.blocks_edit()
                || self.readmission.blocks_edit()
                || self.conversion.blocks_edit()
                || self.foreign.blocks_edit())
            && !self.unknown
            && self.success.is_none()
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                !snapshot.completion_available
                    && matches!(
                        snapshot.state,
                        StorageState::Pending
                            | StorageState::Active
                            | StorageState::TerminalManagement
                    )
            })
    }
    fn can_activate(&self) -> bool {
        self.editable()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.state == StorageState::Pending)
            && self.confirmed
            && !self.selections.is_empty()
            && self.selections.len() <= 32
    }
    fn can_complete(&self) -> bool {
        !self.busy
            && self.success.is_none()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.completion_available)
    }
}

pub(super) fn disk_candidates(
    state: &State,
) -> Vec<storage_contract::storage_directories::DiscoveredDisk> {
    state.admin_console.storage.wizard.candidates()
}
pub(super) fn can_choose_disk(state: &State) -> bool {
    state.session_active() && state.admin_console.storage.editable()
}
pub(super) fn chosen_disk(state: &State, path: &str) -> bool {
    state.admin_console.storage.wizard.selected(path)
}
pub(super) fn candidate_kind(
    state: &storage_contract::storage_directories::DiscoveredDiskState,
) -> String {
    wizard::kind(state)
}
pub(super) fn choose_disk(state: RwSignal<State>, path: &str) {
    let chosen = state.try_update(|s| {
        if !can_choose_disk(s) {
            return false;
        }
        wizard::choose(&mut s.admin_console.storage, path);
        chosen_disk(s, path)
    }) == Some(true);
    if chosen {
        focus_wizard();
    }
}
fn focus_wizard() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("zxs-disk-wizard"))
        {
            let element: web_sys::HtmlElement = element.unchecked_into();
            element.scroll_into_view();
            if let Err(error) = element.focus() {
                web_sys::console::warn_2(&"Could not focus the disk wizard".into(), &error);
            }
        }
    }
}

fn invalid(source: ResponseError) -> api::Error {
    api::Error::invalid_response(
        t(
            "存储管理响应未确认，请刷新状态核对。",
            "The storage management response is unconfirmed. Refresh status to check.",
        ),
        api::ErrorSource::StorageManagement(source),
    )
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, api::Error> {
    json_codec::from_value(value).map_err(|source| invalid(ResponseError::Decode(source)))
}
fn begin(state: &mut State, mutation: bool) -> Option<(u64, u64)> {
    if !state.session_active() || state.admin_console.storage.busy {
        return None;
    }
    let model = &mut state.admin_console.storage;
    model.busy = true;
    model.mutation = mutation;
    model.error = None;
    model.generation += 1;
    Some((state.session_epoch, model.generation))
}
fn current(state: &State, epoch: u64, generation: u64) -> bool {
    state.session_active()
        && state.session_epoch == epoch
        && state.admin_console.storage.generation == generation
}

pub(super) fn refreshing(state: &State) -> bool {
    state.admin_console.storage.busy || state.admin_console.storage.wizard.busy()
}
pub(super) fn poll(state: RwSignal<State>) {
    addition::poll(state);
    conversion::poll(state);
    lifecycle::poll(state);
    readmission::poll(state);
    let needed = state.with_untracked(|s| {
        let model = &s.admin_console.storage;
        s.session_active()
            && !model.busy
            && !model
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.completion_available)
            && (model.unknown
                || (model.success == Some(Success::Recovering)
                    && !model
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.state == StorageState::Active)))
    });
    if needed {
        load(state);
    }
}
pub(super) fn can_load(state: &State) -> bool {
    state.session_active() && !refreshing(state)
}
pub(super) fn load(state: RwSignal<State>) {
    let Some((epoch, generation)) = state.try_update(|s| begin(s, false)).flatten() else {
        return;
    };
    spawn_local(async move {
        let snapshot =
            api::validated_request("GET", "/api/admin/storage/status", None, storage_snapshot)
                .await;
        if !accept_read_response(state, epoch, &snapshot) {
            return;
        }
        let load_disks = state.try_update(|s| {
            if !current(s, epoch, generation) {
                return false;
            }
            finish_load(&mut s.admin_console.storage, snapshot)
        }) == Some(true);
        wizard::load(state);
        if load_disks {
            super::disks::load(state);
            addition::load(state);
            conversion::load(state);
            lifecycle::load(state);
            readmission::load(state);
        }
    });
}

fn finish_load(model: &mut Model, snapshot: Result<Snapshot, api::Error>) -> bool {
    model.busy = false;
    match snapshot {
        Ok(snapshot) => {
            if snapshot.state == StorageState::Active {
                if model.success == Some(Success::Recovering) {
                    model.success = None;
                }
                model.unknown = false;
                model.activation_error = None;
                if model
                    .snapshot
                    .as_ref()
                    .is_some_and(|previous| previous.state == StorageState::Pending)
                {
                    model.selections.clear();
                    model.wizard.clear_choice();
                }
            }
            if !snapshot.allow_development {
                model.development = false;
            }
            model.confirmed = false;
            let load_disks = snapshot.state != StorageState::Pending;
            model.snapshot = Some(snapshot);
            load_disks
        }
        Err(error) => {
            model.error = Some(error);
            true
        }
    }
}

fn activate(state: RwSignal<State>, completing: bool) {
    let captured = state
        .try_update(|s| {
            let model = &s.admin_console.storage;
            if !(if completing {
                model.can_complete()
            } else {
                model.can_activate()
            }) {
                return None;
            }
            let body = if completing {
                json!({})
            } else {
                json!({"selections":model.selections,"mode":if model.development {"development"} else {"production"}})
            };
            let (epoch, generation) = begin(s, true)?;
            s.admin_console.storage.confirmed = false;
            Some((epoch, generation, body))
        })
        .flatten();
    let Some((epoch, generation, body)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = if completing {
            "/api/admin/storage/complete-activation"
        } else {
            "/api/admin/storage/activate"
        };
        let result = api::validated_request("POST", path, Some(body), |value| {
            response::activation(value).map_err(|error| invalid(ResponseError::Activation(error)))
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let settled = state.try_update(|s| {
            if !current(s, epoch, generation) {
                return false;
            }
            finish_activation(&mut s.admin_console.storage, result);
            true
        }) == Some(true);
        if settled {
            crate::management::load(state);
            load(state);
        }
    });
}
fn finish_activation(model: &mut Model, result: Result<Success, api::Error>) {
    model.busy = false;
    model.mutation = false;
    match result {
        Ok(success) => {
            model.success = Some(success);
            model.unknown = false;
            model.activation_error = None;
        }
        Err(error) => {
            model.unknown |= error.mutation_outcome_unknown();
            model.activation_error = Some(error);
        }
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-stack zxs-storage-activation" aria-label=t("存储状态", "Storage status")>
        <h2 class="zxs-row">{t("存储状态", "Storage status")}</h2>
        <p class="zxs-row" role="status">{move ||state.with(|s|match s.admin_console.storage.snapshot.as_ref().map(|s|s.state) {
            None => t("请刷新存储状态。", "Refresh the storage status."),
            Some(StorageState::Pending) => t("管理账户已就绪，尚无可用的 ZXS 存储。", "Management is ready; no ZXS storage is available."),
            Some(StorageState::Active) => t("存储已就绪。", "Storage is ready."),
            Some(StorageState::RecoveryRequired) => t("存储需要恢复，请先核对原有操作结果。", "Storage needs recovery. Check the original operation outcome first."),
            Some(StorageState::TerminalManagement) => t("管理态保留历史与当前登记，普通存储读写已关闭。", "Management retains history and registrations; ordinary storage access is closed."),
            Some(StorageState::Unavailable) => t("存储状态不可用，不会重新初始化已有目录。", "Storage status is unavailable. Existing directories will not be reinitialized."),
        })}</p>
        <p class="zxs-row">{t("存储发现目录由 systemd 启动参数指定。只读取该目录的直接子目录；选择候选并完成磁盘向导后才能加入，不递归发现、不自动挂载或格式化。", "The discovery directory comes from systemd startup configuration. Only immediate directory children are inspected. Select a candidate and complete the disk wizard before admission; discovery is not recursive and never mounts or formats storage.")}</p>
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!can_load(s)) on:click=move |_|load(state)>{t("刷新存储状态", "Refresh storage status")}</button>
        {move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().and_then(|snapshot|snapshot.terminal.clone())).map(|terminal|view!{
            <p class="zxs-row">{format!("{}{} · {}{}/{}",t("保留登记目录：", "Retained registered directories: "),terminal.registered_directories,t("原固定元数据 N/Q：", "Original fixed metadata N/Q: "),terminal.original_metadata_members,terminal.original_metadata_quorum)}</p>
            <p class="zxs-row">{t("磁盘内容与历史没有清理。新的元数据组需要基于这份已认证历史接续，不能使用首次激活或普通添加替代。", "Disk contents and history have not been cleared. A new metadata group must resume this authenticated history; initial activation and ordinary addition cannot replace that process.")}</p>
            {terminal.completed_retirement.map(|operation|view!{
                <p class="zxs-row" role="alert">{format!("{}{} · {}", t("终态最近登记移除已发布：", "Latest terminal registration retirement published: "), operation, t("后续移除须另行选择准确剩余登记；仍不支持后继元数据恢复，保留全部历史，不能从旧配置重开。", "Further retirements require separate exact remaining-member selection. Successor metadata resumption is still unsupported; preserve history and never reopen from old configuration."))}</p>
            })}
            {terminal.pending_retirement.map(|pending|view!{
                <p class="zxs-row" role="alert">{format!("{}{} · {} · {}", t("终态注销原决定未发布：", "Terminal retirement decision is unfinished: "), pending.slot_id, pending.operation_id, t("当前登记尚未移除，元数据恢复被阻断；保留原决定，不要清理文件或另换操作。", "The registration remains and metadata resumption is fenced. Preserve the original decision; do not clean files or replace the operation."))}</p>
            })}
        })}
        {wizard::view(state)}
        <Show when=move ||state.with(|s|!s.admin_console.storage.conversion.blocks_edit())>{addition::view(state)}</Show>
        {conversion::view(state)}
        {foreign::planning(state)}
        <Show when=move ||state.with(|s|s.admin_console.storage.unknown)><p class="zxs-row" role="alert">{t("激活结果未知，禁止重新提交。刷新状态核对；只有服务器验证的原激活可以继续。关闭页面不会取消服务器操作。", "Activation outcome is unknown; resubmission is blocked. Refresh status to check. Only the server-verified original activation can be completed. Closing this page does not cancel the server operation.")}</p></Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.can_complete())><p class="zxs-row">{t("服务器已验证原激活的中断证明。继续只完成原目录与当前账户的绑定，不重建管理员或选择新目录。", "The server verified the interrupted activation. Continuing only completes the original directories' binding to the current account; it does not recreate the administrator or select new directories.")}</p><button type="button" class="zxs-button" data-storage-complete="" on:click=move |_|activate(state, true)>{t("继续原激活", "Complete original activation")}</button></Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.success.is_some())><p class="zxs-row" role="status">{move ||state.with(|s|if s.admin_console.storage.success==Some(Success::RestartRequired){t("存储已激活，服务需要本地重启。不要再次添加。", "Storage was activated; the service needs a local restart. Do not add it again.")}else{t("存储已激活，正在打开。请刷新状态核对。", "Storage was activated and is opening. Refresh status to check.")})}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.activation_error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
        {move ||state.with(|s|s.admin_console.storage.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    </section>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/storage_tests.rs"]
mod tests;
