//! First storage activation is independent from administrator account creation.
//! Submitted activation survives navigation and can only settle from real evidence.
use crate::{
    api,
    ui::{State, accept_read_response},
};
use data_serialization::{Deserialize, de::DeserializeOwned};
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::storage_directories::{
    CandidateSelection, DirectoryListing, DiscoveryRootSettings, InspectRequest,
};
use web_interface::{components::ErrorState, i18n::t, prelude::*, tasks::spawn_local};

mod addition;
pub(super) mod lifecycle;
mod response;
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
    if (snapshot.state == StorageState::TerminalManagement) != snapshot.terminal.is_some()
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
    discovery_root: String,
    discovery_input: String,
    discovery_unknown: bool,
    browse_path: String,
    listing: Option<DirectoryListing>,
    mount_path: String,
    root_path: String,
    inspected: Option<CandidateSelection>,
    selections: Vec<CandidateSelection>,
    development: bool,
    confirmed: bool,
    addition: addition::Model,
    lifecycle: lifecycle::Model,
}
impl Model {
    pub(super) fn reload_blocked(&self) -> bool {
        self.mutation
            || self.unknown
            || self.discovery_unknown
            || (self.addition.blocks_edit() || self.lifecycle.blocks_edit())
    }
    fn editable(&self) -> bool {
        !self.busy
            && !(self.addition.blocks_edit() || self.lifecycle.blocks_edit())
            && !self.unknown
            && !self.discovery_unknown
            && self.success.is_none()
            && self.snapshot.as_ref().is_some_and(|s| {
                matches!(
                    s.state,
                    StorageState::Pending | StorageState::Active | StorageState::TerminalManagement
                )
            })
    }
    fn can_set_root(&self) -> bool {
        !self.busy
            && !(self.addition.blocks_edit() || self.lifecycle.blocks_edit())
            && !self.unknown
            && !self.discovery_unknown
            && self.success.is_none()
            && self.snapshot.as_ref().is_some_and(|s| {
                matches!(
                    s.state,
                    StorageState::Pending | StorageState::Active | StorageState::TerminalManagement
                )
            })
            && !self.discovery_root.is_empty()
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
    fn invalidate_inspection(&mut self) {
        self.inspected = None;
        self.confirmed = false;
        self.error = None;
        self.addition.invalidate();
    }
}

fn invalid(source: ResponseError) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::InvalidResponse,
        message: t(
            "存储管理响应未确认，请刷新状态核对。",
            "The storage management response is unconfirmed. Refresh status to check.",
        ),
        retryable: false,
        status: 200,
        request_id: None,
        source: Some(Arc::new(api::ErrorSource::StorageManagement(source))),
    }
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
    state.admin_console.storage.busy
}
pub(super) fn poll(state: RwSignal<State>) {
    addition::poll(state);
    lifecycle::poll(state);
    let needed = state.with_untracked(|s| {
        let model = &s.admin_console.storage;
        s.session_active()
            && !model.busy
            && !model
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.completion_available)
            && (model.discovery_unknown
                || model.unknown
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
        let settings = api::validated_request(
            "GET",
            "/api/admin/storage/discovery-root",
            None,
            decode::<DiscoveryRootSettings>,
        )
        .await;
        if !accept_read_response(state, epoch, &settings) {
            return;
        }
        let load_disks = state.try_update(|s| {
            if !current(s, epoch, generation) {
                return false;
            }
            finish_load(&mut s.admin_console.storage, snapshot, settings)
        }) == Some(true);
        if load_disks {
            super::disks::load(state);
            addition::load(state);
            lifecycle::load(state);
        }
    });
}

fn finish_load(
    model: &mut Model,
    snapshot: Result<Snapshot, api::Error>,
    settings: Result<DiscoveryRootSettings, api::Error>,
) -> bool {
    model.busy = false;
    match settings {
        Ok(settings) => {
            if model.discovery_root != settings.discovery_root {
                model.listing = None;
                model.inspected = None;
                model.selections.clear();
                if !model.addition.blocks_edit() {
                    addition::selected(model);
                }
                model.browse_path = settings.discovery_root.clone();
            }
            model.discovery_input = settings.discovery_root.clone();
            model.discovery_root = settings.discovery_root;
            model.discovery_unknown = false;
        }
        Err(error) => model.error = Some(error),
    }
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
                    model.inspected = None;
                }
            }
            if !snapshot.allow_development {
                model.development = false;
            }
            let load_disks = snapshot.state != StorageState::Pending;
            model.snapshot = Some(snapshot);
            model.confirmed = false;
            load_disks
        }
        Err(error) => {
            model.error = Some(error);
            // Activation/settings diagnostics are not the disk-list authority.
            // Existing pools retain their own authenticated observation path.
            true
        }
    }
}

fn save_discovery_root(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            let model = &s.admin_console.storage;
            if !model.can_set_root()
                || model.discovery_input.is_empty()
                || model.discovery_input == model.discovery_root
            {
                return None;
            }
            let expected = model.discovery_input.clone();
            let body =
                json!({"expected_discovery_root":model.discovery_root,"discovery_root":expected});
            let (epoch, generation) = begin(s, true)?;
            Some((epoch, generation, body, expected))
        })
        .flatten();
    let Some((epoch, generation, body, expected)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request(
            "PUT",
            "/api/admin/storage/discovery-root",
            Some(body),
            move |value| {
                let settings: DiscoveryRootSettings = decode(value)?;
                if settings.discovery_root != expected {
                    return Err(invalid(ResponseError::Identity));
                }
                Ok(settings)
            },
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let settled = state.try_update(|s| {
            if !current(s, epoch, generation) {
                return false;
            }
            let model = &mut s.admin_console.storage;
            model.busy = false;
            model.mutation = false;
            match result {
                Ok(settings) => {
                    model.discovery_root = settings.discovery_root.clone();
                    model.discovery_input = settings.discovery_root.clone();
                    model.browse_path = settings.discovery_root;
                    model.listing = None;
                    model.inspected = None;
                    model.selections.clear();
                    model.confirmed = false;
                    model.discovery_unknown = false;
                    true
                }
                Err(error) => {
                    model.discovery_unknown = error.mutation_outcome_unknown();
                    model.error = Some(error);
                    model.discovery_unknown
                }
            }
        }) == Some(true);
        if settled {
            load(state);
        }
    });
}

fn change(state: RwSignal<State>, field: &'static str, value: String) {
    state.update(|s| {
        if !s.session_active() {
            return;
        }
        let model = &mut s.admin_console.storage;
        if !model.editable() {
            return;
        }
        match field {
            "mount" => model.mount_path = value,
            "root" => model.root_path = value,
            "browse" => model.browse_path = value,
            _ => unreachable!("invariant: storage input has a declared field"),
        }
        model.invalidate_inspection();
    });
}
fn browse(state: RwSignal<State>, path: String) {
    let captured = state
        .try_update(|s| {
            if !s.admin_console.storage.editable() || path.is_empty() {
                return None;
            }
            let request = begin(s, false)?;
            s.admin_console.storage.browse_path = path.clone();
            Some(request)
        })
        .flatten();
    let Some((epoch, generation)) = captured else {
        return;
    };
    spawn_local(async move {
        let expected = path.clone();
        let result = api::validated_request(
            "GET",
            &format!(
                "/api/admin/storage/directories?path={}",
                api::segment(&path)
            ),
            None,
            move |value| {
                let listing: DirectoryListing = decode(value)?;
                if listing.path != expected {
                    return Err(invalid(ResponseError::Identity));
                }
                Ok(listing)
            },
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        state.update(|s| {
            if !current(s, epoch, generation) {
                return;
            }
            let model = &mut s.admin_console.storage;
            model.busy = false;
            match result {
                Ok(listing) => model.listing = Some(listing),
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn use_directory(state: RwSignal<State>) {
    state.update(|s| {
        if !s.session_active() {
            return;
        }
        let model = &mut s.admin_console.storage;
        if !model.editable() {
            return;
        }
        let Some(listing) = &model.listing else {
            return;
        };
        model.root_path = listing.path.clone();
        model.mount_path = listing.mount_path.clone();
        model.invalidate_inspection();
    });
}
fn inspect(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            let model = &s.admin_console.storage;
            if !model.editable() || model.mount_path.is_empty() || model.root_path.is_empty() {
                return None;
            }
            let request = InspectRequest {
                mount_path: model.mount_path.clone(),
                root_path: model.root_path.clone(),
            };
            let (epoch, generation) = begin(s, false)?;
            s.admin_console.storage.inspected = None;
            Some((epoch, generation, request))
        })
        .flatten();
    let Some((epoch, generation, request)) = captured else {
        return;
    };
    spawn_local(async move {
        let body = json_codec::to_value(&request)
            .expect("invariant: directory inspection requests serialize");
        let result = api::validated_request(
            "POST",
            "/api/admin/storage/directories/inspect",
            Some(body),
            move |value| {
                let candidate: CandidateSelection = decode(value)?;
                if candidate.mount_path != request.mount_path
                    || candidate.root_path != request.root_path
                    || candidate.selection_id.is_empty()
                {
                    return Err(invalid(ResponseError::Identity));
                }
                Ok(candidate)
            },
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        state.update(|s| {
            if !current(s, epoch, generation) {
                return;
            }
            let model = &mut s.admin_console.storage;
            model.busy = false;
            match result {
                Ok(candidate) => model.inspected = Some(candidate),
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn add_selection(state: RwSignal<State>) {
    state.update(|s| {
        if !s.session_active() {
            return;
        }
        let model = &mut s.admin_console.storage;
        if !model.editable() || model.selections.len() == 32 {
            return;
        }
        let Some(candidate) = model.inspected.take() else {
            return;
        };
        if !model
            .selections
            .iter()
            .any(|item| item.selection_id == candidate.selection_id)
        {
            model.selections.push(candidate);
        }
        model.confirmed = false;
        addition::selected(model);
    });
}
fn remove_selection(state: RwSignal<State>, id: &str) {
    state.update(|s| {
        if s.session_active() && s.admin_console.storage.editable() {
            s.admin_console
                .storage
                .selections
                .retain(|item| item.selection_id != id);
            s.admin_console.storage.confirmed = false;
            addition::selected(&mut s.admin_console.storage);
        }
    });
}
fn activate(state: RwSignal<State>, completing: bool) {
    let captured = state.try_update(|s| {
        let model = &s.admin_console.storage;
        if !(if completing { model.can_complete() } else { model.can_activate() }) { return None; }
        let body = if completing { json!({}) } else {
            json!({"selections":model.selections,"mode":if model.development {"development"} else {"production"}})
        };
        let (epoch, generation) = begin(s, true)?;
        s.admin_console.storage.confirmed = false;
        Some((epoch, generation, body))
    }).flatten();
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

fn input(state: RwSignal<State>, field: &'static str, label: String) -> impl IntoView {
    view! {<label class="zxs-form-field"><span>{label}</span><input class="zxs-input" data-storage-field=field type="text" autocomplete="off" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:value=move ||state.with(|s|match field { "mount" => s.admin_console.storage.mount_path.clone(), "root" => s.admin_console.storage.root_path.clone(), "browse" => s.admin_console.storage.browse_path.clone(), _ => unreachable!("invariant: storage input field") }) on:input=move |event|change(state,field,event_target_value(&event))/></label>}
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-stack zxs-storage-activation" aria-label=t("添加存储", "Add storage")>
        <h2 class="zxs-row">{t("添加存储", "Add storage")}</h2>
        <p class="zxs-row" role="status">{move ||state.with(|s|match s.admin_console.storage.snapshot.as_ref().map(|s|s.state) {
            None => t("请刷新存储状态。", "Refresh the storage status."),
            Some(StorageState::Pending) => t("管理账户已就绪，尚未添加存储。选择发现目录内已挂载磁盘上的空子目录。", "Management is ready; storage has not been added. Choose an empty subdirectory on a mounted disk within the discovery directory."),
            Some(StorageState::Active) => t("存储池已激活。", "The storage pool is active."),
            Some(StorageState::RecoveryRequired) => t("存储需要恢复，请先核对原有激活结果。", "Storage needs recovery. Check the original activation outcome first."),
            Some(StorageState::TerminalManagement) => t("最后一个元数据成员已退出。管理态保留历史与当前登记，普通存储读写已关闭；这不是首次初始化。", "The final metadata member has exited. Management retains history and current registrations; ordinary storage access is closed. This is not first-time initialization."),
            Some(StorageState::Unavailable) => t("存储状态不可用，不会重新初始化已有目录。", "Storage status is unavailable. Existing directories will not be reinitialized."),
        })}</p>
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!can_load(s)) on:click=move |_|load(state)>{t("刷新存储状态", "Refresh storage status")}</button>
        <p class="zxs-row">{t("只发现存储发现目录内的目录，默认 /zxs/data/disks，可指定其他目录。修改此设置不会移动已有存储或挂载磁盘。", "Only directories within the discovery directory are listed. The default is /zxs/data/disks; you can choose another directory. Changing it does not move existing storage or mount disks.")}</p>
        <label class="zxs-form-field"><span>{t("存储发现目录", "Storage discovery directory")}</span><input class="zxs-input" data-storage-discovery-root="" type="text" autocomplete="off" disabled=move ||state.with(|s|!s.admin_console.storage.can_set_root()) prop:value=move ||state.with(|s|s.admin_console.storage.discovery_input.clone()) on:input=move |event|state.update(|s|{if s.session_active() && s.admin_console.storage.can_set_root(){s.admin_console.storage.discovery_input=event_target_value(&event);}})/></label>
        <button type="button" class="zxs-button" data-storage-save-discovery-root="" disabled=move ||state.with(|s|!s.admin_console.storage.can_set_root()||s.admin_console.storage.discovery_input==s.admin_console.storage.discovery_root||s.admin_console.storage.discovery_input.is_empty()) on:click=move |_|save_discovery_root(state)>{t("保存发现目录", "Save discovery directory")}</button>
        <Show when=move ||state.with(|s|s.admin_console.storage.discovery_unknown)><p class="zxs-row" role="alert">{t("目录设置结果未知，请刷新状态核对后再继续。", "The directory setting outcome is unknown. Refresh status before continuing.")}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().and_then(|snapshot|snapshot.terminal.clone())).map(|terminal|view!{
            <p class="zxs-row">{format!("{}{} · {}{}/{}",t("保留登记目录：", "Retained registered directories: "),terminal.registered_directories,t("原固定元数据 N/Q：", "Original fixed metadata N/Q: "),terminal.original_metadata_members,terminal.original_metadata_quorum)}</p>
            <p class="zxs-row">{t("磁盘内容与历史没有清理。新的元数据组需要基于这份已认证历史接续，不能使用首次激活或普通添加替代。", "Disk contents and history have not been cleared. A new metadata group must resume this authenticated history; initial activation and ordinary addition cannot replace that process.")}</p>
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().is_some_and(|s|matches!(s.state,StorageState::Pending|StorageState::Active|StorageState::TerminalManagement))&&s.admin_console.storage.success.is_none())>
            <p class="zxs-row">{t("挂载目录与实际存储目录分开设置。仅使用当前已挂载文件系统和服务已有权限，不格式化、不自动挂载、不修改权限。", "Set the mount directory and actual storage directory separately. Only mounted filesystems and existing service permissions are used. No formatting, automatic mounting, or permission changes occur.")}</p>
            <div class="zxs-form-grid">{input(state,"browse",t("浏览目录", "Browse directory"))}</div>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:click=move |_|{let path=state.with_untracked(|s|s.admin_console.storage.browse_path.clone());browse(state,if path.is_empty(){state.with_untracked(|s|s.admin_console.storage.discovery_root.clone())}else{path})}>{t("浏览", "Browse")}</button>
            {move ||state.with(|s|s.admin_console.storage.listing.clone()).map(|listing|view!{
                <p class="zxs-row">{format!("{} · {}",listing.path,listing.mount_path)}</p>
                <div class="zxs-flow">{listing.entries.into_iter().map(move |entry|{let path=entry.path;view!{<button type="button" class="zxs-button" disabled=move ||!entry.can_browse||state.with(|s|!s.admin_console.storage.editable()) on:click=move |_|browse(state,path.clone())>{entry.name}</button>}}).collect_view()}</div>
                <Show when=move ||listing.truncated><p class="zxs-row">{t("目录较大，列表已截断；可直接输入完整子目录路径。", "This directory is large; the list was truncated. You can enter a full subdirectory path.")}</p></Show>
                <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:click=move |_|use_directory(state)>{t("使用此目录", "Use this directory")}</button>
            })}
            <div class="zxs-form-grid">{input(state,"mount",t("挂载目录", "Mount directory"))}{input(state,"root",t("实际存储目录", "Actual storage directory"))}</div>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) data-storage-inspect="" on:click=move |_|inspect(state)>{t("检查目录", "Check directory")}</button>
            <Show when=move ||state.with(|s|s.admin_console.storage.inspected.is_some())>
                <p class="zxs-row">{t("目录检查通过。加入下方待添加列表后再次核对，提交时会重新验证目录和挂载身份。", "The directory passed inspection. Add it to the list below and review it. Directory and mount identities are verified again when submitted.")}</p>
                <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) data-storage-add-selection="" on:click=move |_|add_selection(state)>{t("加入待添加列表", "Add to selection")}</button>
            </Show>
            <For each=move ||state.with(|s|s.admin_console.storage.selections.clone()) key=|item|item.selection_id.clone() children=move |item|{let id=item.selection_id;view!{<div class="zxs-flow"><p class="zxs-row">{format!("{} → {}",item.mount_path,item.root_path)}</p><button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:click=move |_|remove_selection(state,&id)>{t("移除待添加项", "Remove selection")}</button></div>}}/>
            <Show when=move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().is_some_and(|s|s.state==StorageState::Pending))>
            <Show when=move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().is_some_and(|s|s.allow_development))><label class="zxs-row"><input data-storage-development="" type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.development) disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.development=event_target_checked(&event);s.admin_console.storage.confirmed=false;}})/>{t("开发模拟目录（非生产）", "Development simulation directories (not production)")}</label></Show>
            <p class="zxs-row">{t("列表第一项为 P01 主存储，后续项按顺序建立逐级备份链，全部作为元数据成员；正常写入需要所有成员在线。单目录只有一份副本，同一设备上的多个目录不提供磁盘故障冗余。", "The first selection is P01 primary storage. Later selections form a tiered backup chain; all are metadata members and must be online for normal writes. One directory holds one copy. Multiple directories on one device do not protect against disk failure.")}</p>
            <p class="zxs-row">{move ||state.with(|s|{let members=s.admin_console.storage.selections.len();if members==0 {String::new()}else{format!("{}{}{}{}{}",t("元数据成员：", "Metadata members: "),members,t("；系统恢复至少需要原成员中的 ", "; system recovery requires at least "),members/2+1,t(" 个一致成员。", " consistent original members."))}})}</p>
            <label class="zxs-row"><input data-storage-confirm="" type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.confirmed) disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.confirmed=event_target_checked(&event);}})/>{t("确认将列表中的空目录用于新存储池，创建存储身份与元数据。", "Use the selected empty directories for the new storage pool and create their storage identities and metadata.")}</label>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.can_activate()) data-storage-activate="" on:click=move |_|activate(state,false)>{t("添加并激活存储", "Add and activate storage")}</button>
            </Show>
        </Show>
        {addition::view(state)}
        <Show when=move ||state.with(|s|s.admin_console.storage.unknown)><p class="zxs-row" role="alert">{t("激活结果未知，禁止重新提交。刷新状态核对；只有服务器验证的原激活可以继续。关闭页面不会取消服务器操作。", "Activation outcome is unknown; resubmission is blocked. Refresh status to check. Only the server-verified original activation can be completed. Closing this page does not cancel the server operation.")}</p></Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.can_complete())><p class="zxs-row">{t("服务器已验证原激活的中断证明。继续只完成原目录与当前账户的绑定，不重建管理员或选择新目录。", "The server verified the interrupted activation. Continuing only completes the original directories' binding to the current account; it does not recreate the administrator or select new directories.")}</p><button type="button" class="zxs-button" data-storage-complete="" on:click=move |_|activate(state,true)>{t("继续原激活", "Complete original activation")}</button></Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.success.is_some())><p class="zxs-row" role="status">{move ||state.with(|s|if s.admin_console.storage.success==Some(Success::RestartRequired){t("存储已激活，服务需要本地重启。不要再次添加。", "Storage was activated; the service needs a local restart. Do not add it again.")}else{t("存储已激活，正在打开。请刷新状态核对。", "Storage was activated and is opening. Refresh status to check.")})}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.activation_error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
        {move ||state.with(|s|s.admin_console.storage.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    </section>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/storage_tests.rs"]
mod tests;
