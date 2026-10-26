//! Explicit bootstrap authorization stays in component memory, never URLs or storage.
use crate::api;
use json_codec::{Value, json};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Action {
    #[default]
    Wizard,
    Preset,
    Complete,
}
use web_interface::{
    components::{ErrorState, Select, SelectOption},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Clone, Copy, Default)]
enum Status {
    #[default]
    Unknown,
    Empty,
    Existing,
    RecoveryRequired,
    Unavailable,
}
impl Status {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("empty") => Self::Empty,
            Some("existing") => Self::Existing,
            Some("recovery_required") => Self::RecoveryRequired,
            Some("unavailable") | None => Self::Unavailable,
            Some(_) => Self::Unknown,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    key: String,
    action: Action,
    completion_available: bool,
    presets: Vec<String>,
    preset_preview: Value,
    preset_digest: String,
    preset_id: String,
    csrf: String,
    allow_development: bool,
    development: bool,
    username: String,
    password: String,
    disks: Vec<String>,
    selected: Vec<String>,
    empty: bool,
    pending: bool,
    confirmed: bool,
    unknown: bool,
    complete: bool,
    restart_required: bool,
    generation: u64,
    failure: Option<String>,
    status: Status,
}
fn valid_selection(model: &Model) -> bool {
    !model.selected.is_empty()
        && model.selected.len() <= 32
        && model
            .selected
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == model.selected.len()
        && model.selected.iter().all(|id| model.disks.contains(id))
}
fn check(state: RwSignal<Model>) {
    let Some((key, generation)) = state
        .try_update(|s| {
            if s.pending || s.complete {
                return None;
            }
            s.pending = true;
            s.empty = false;
            s.completion_available = false;
            s.presets.clear();
            s.preset_id.clear();
            s.preset_preview = Value::Null;
            s.preset_digest.clear();
            s.action = Action::Wizard;
            s.disks.clear();
            s.selected.clear();
            s.confirmed = false;
            s.failure = None;
            s.generation += 1;
            Some((s.key.clone(), s.generation))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::setup_request("GET", "/api/setup/status", None, &key, None).await;
        let _ = state.try_update(|s| finish_check(s, generation, &key, result));
    });
}
/// A root-owned read may outlive the authorization form. Settle its loading
/// state, but never restore authorization after cleanup cleared its key.
fn finish_current_read(s: &mut Model, generation: u64, key: &str) -> bool {
    if s.generation != generation {
        return false;
    }
    s.pending = false;
    s.key == key
}
fn finish_check(s: &mut Model, generation: u64, key: &str, result: Result<Value, api::Error>) {
    if !finish_current_read(s, generation, key) {
        return;
    }
    match result {
        Ok(value) if !key.is_empty() => accept_status(s, &value),
        Ok(_) => {}
        Err(error) => s.failure = Some(error.to_string()),
    }
}
fn accept_status(s: &mut Model, value: &Value) {
    s.status = Status::from_wire(value["state"].as_str());
    s.allow_development = value["allow_development"] == true;
    s.development = false;
    s.csrf = value["csrf_token"].as_str().unwrap_or_default().into();
    s.empty = matches!(s.status, Status::Empty) && !s.csrf.is_empty();
    s.completion_available = value["completion_available"] == true && !s.csrf.is_empty();
    s.presets = value["config_presets"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if s.empty {
        s.disks = value["disk_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
    }
}
fn preset_reviewed(s: &Model) -> bool {
    !s.preset_id.is_empty()
        && s.presets.contains(&s.preset_id)
        && s.preset_preview.is_object()
        && !s.preset_digest.is_empty()
}
fn can_initialize(s: &Model) -> bool {
    !s.pending
        && !s.complete
        && s.confirmed
        && !s.username.trim().is_empty()
        && !s.password.is_empty()
        && !s.csrf.is_empty()
        && match s.action {
            Action::Wizard => s.empty && !s.unknown && valid_selection(s),
            Action::Preset => s.empty && !s.unknown && preset_reviewed(s),
            // Only a newly authorized server proof can admit this separate,
            // bounded completion operation after an uncertain initialization.
            Action::Complete => s.completion_available,
        }
}
fn submission(s: &Model, password: String) -> (&'static str, Value) {
    match s.action {
        Action::Wizard => (
            "/api/setup/initialize",
            json!({"disk_ids":s.selected,"mode":if s.allow_development && s.development {"development"} else {"production"},"username":s.username,"password":password}),
        ),
        Action::Preset => (
            "/api/setup/initialize-preset",
            json!({"preset_id":s.preset_id,"config_digest":s.preset_digest,"username":s.username,"password":password}),
        ),
        Action::Complete => (
            "/api/setup/complete-initialization",
            json!({"username":s.username,"password":password}),
        ),
    }
}
fn choose(s: &mut Model, action: Action) {
    s.action = action;
    s.confirmed = false;
}

fn initialize(state: RwSignal<Model>) {
    let Some((key, csrf, path, body, generation)) = state
        .try_update(|s| {
            if !can_initialize(s) {
                return None;
            }
            s.pending = true;
            s.generation += 1;
            s.failure = None;
            let key = std::mem::take(&mut s.key);
            let password = std::mem::take(&mut s.password);
            let (path, body) = submission(s, password);
            s.completion_available = false;
            Some((key, std::mem::take(&mut s.csrf), path, body, s.generation))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::setup_request("POST", path, Some(body), &key, Some(&csrf)).await;
        if !state
            .try_with_untracked(|s| s.generation == generation)
            .unwrap_or(false)
        {
            return;
        }
        state.update(|s| finish_initialization(s, result));
    });
}
fn finish_initialization(s: &mut Model, result: Result<Value, api::Error>) {
    s.pending = false;
    s.empty = false;
    s.confirmed = false;
    match result {
        Ok(value) if value["accepted"] == true || value["initialized"] == true => {
            s.complete = true;
            // A successful authorized completion resolves the earlier unknown
            // initialization. Keep complete set to prevent fresh initialization.
            s.unknown = false;
            s.restart_required = value["restart_required"] == true;
        }
        Ok(_) => {
            s.unknown = true;
            s.failure = Some(t(
                "设置响应未确认。请在服务器本地检查，勿重复初始化。",
                "Setup was not confirmed. Check locally on the server; do not initialize again.",
            ));
        }
        Err(error) => {
            s.unknown |= error.mutation_outcome_unknown();
            s.failure = Some(error.to_string());
        }
    }
}
fn preview_preset(state: RwSignal<Model>) {
    let Some((key, csrf, id, generation)) = state
        .try_update(|s| {
            if s.pending
                || !s.empty
                || s.unknown
                || s.complete
                || !s.presets.contains(&s.preset_id)
                || s.csrf.is_empty()
            {
                return None;
            }
            s.pending = true;
            s.confirmed = false;
            s.preset_digest.clear();
            s.preset_preview = Value::Null;
            s.failure = None;
            s.generation += 1;
            Some((
                s.key.clone(),
                s.csrf.clone(),
                s.preset_id.clone(),
                s.generation,
            ))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::setup_request(
            "POST",
            "/api/setup/preset-preview",
            Some(json!({"preset_id":id})),
            &key,
            Some(&csrf),
        )
        .await;
        let _ = state.try_update(|s| finish_preset_preview(s, generation, &key, &id, result));
    });
}
fn finish_preset_preview(
    s: &mut Model,
    generation: u64,
    key: &str,
    id: &str,
    result: Result<Value, api::Error>,
) {
    if !finish_current_read(s, generation, key) || s.preset_id != id {
        return;
    }
    match result {
        Ok(value)
            if !key.is_empty()
                && value["preset_id"] == id
                && value["configuration"].is_object()
                && value["config_digest"]
                    .as_str()
                    .is_some_and(|digest| !digest.is_empty()) =>
        {
            s.preset_preview = value["configuration"].clone();
            s.preset_digest = value["config_digest"]
                .as_str()
                .expect("invariant: preset digest was validated")
                .into();
        }
        Ok(_) => {
            s.failure = Some(t(
                "预置方案摘要不完整，禁止初始化。",
                "The preset preview is incomplete; initialization is blocked.",
            ))
        }
        Err(error) => s.failure = Some(error.to_string()),
    }
}
fn preset_view(state: RwSignal<Model>) -> impl IntoView {
    let options = Signal::derive(move || {
        state.with(|s| {
            s.presets
                .iter()
                .map(|id| SelectOption::new(id.clone(), id.clone()))
                .collect::<Vec<_>>()
        })
    });
    let value = Signal::derive(move || state.with(|s| s.preset_id.clone()));
    view! {
        <div class="zxs-stack">
            <Select label=t("本地预置方案 ID","Local preset ID") options=options value=value disabled=Signal::derive(move ||state.with(|s|s.pending)) on_change=Callback::new(move |id:String|state.update(|s|{if !s.pending {s.preset_id=id;s.preset_digest.clear();s.preset_preview=Value::Null;s.confirmed=false;}}))/>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending||!s.presets.contains(&s.preset_id)) on:click=move |_|preview_preset(state)>{t("查看预置方案摘要","Review preset summary")}</button>
            <Show when=move ||state.with(preset_reviewed)>
                <p>{t("以下是服务器本地方案的只读摘要，磁盘以允许的候选 ID 表示；不提交文件路径或任意配置。初始化绑定此摘要，方案改变后必须重新检查。","This is a read-only preview of the local preset, with disks represented by allowed candidate IDs. No file paths or arbitrary configuration are submitted. Initialization is bound to this preview; changed presets require a new review.")}</p>
                <pre data-setup-preset-summary="">{move ||state.with(|s|json_codec::to_string_pretty(&s.preset_preview).expect("invariant: JSON values serialize"))}</pre>
            </Show>
        </div>
    }
}

impl Model {
    pub(crate) fn reload_blocked(&self) -> bool {
        self.pending || self.unknown
    }
}

fn clear_authorization(s: &mut Model) {
    s.key.clear();
    s.password.clear();
    s.csrf.clear();
}

pub(crate) fn view(state: RwSignal<Model>) -> impl IntoView {
    // Keep only recovery state when the login/setup view disappears. Moving
    // the model to the app owner must not extend the lifetime of its secrets.
    on_cleanup(move || {
        state.update(clear_authorization);
    });
    view! {
        <details class="zxs-content-panel zxs-stack"><summary>{t("受保护的首次设置","Protected first-time setup")}</summary>
        <p>{t("仅用于全新安装。现有配置、缺失挂载或认证恢复不会被当作新安装。先在服务器本地配置授权密钥和允许的硬盘；这里只接受允许的硬盘或方案 ID。","For new installations only. Existing configuration, missing mounts and authentication recovery are never treated as a new installation. Provision an authorization key and allowed disks locally first; this form accepts approved disk or preset IDs only.")}</p>
        <form class="zxs-stack" on:submit=move |event|{event.prevent_default();initialize(state);}>
            <label>{t("本地授权密钥","Local authorization key")}<input class="zxs-input" data-setup-key="" type="password" autocomplete="off" prop:value=move ||state.with(|s|s.key.clone()) disabled=move ||state.with(|s|s.pending||s.complete) on:input=move |event|state.update(|s|{s.key=event_target_value(&event);s.empty=false;s.completion_available=false;s.confirmed=false;})/></label>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending||s.complete) on:click=move |_|check(state)>{t("验证授权并检查磁盘","Authorize and check disks")}</button>
            <p role="status">{move ||state.with(|s|match s.status{Status::Empty=>t("已确认空白安装。至少选择一个存储盘；备份盘可选，也可选择已允许的本地预置方案。","Empty installation confirmed. Select at least one storage disk; backup disks are optional. You may also choose an allowed local preset."),Status::Existing=>t("已存在安装；禁止初始化。","An installation already exists; initialization is blocked."),Status::RecoveryRequired=>t("需要本地恢复；禁止初始化。","Local recovery is required; initialization is blocked."),Status::Unavailable=>t("磁盘不可用；禁止初始化。","Disks are unavailable; initialization is blocked."),Status::Unknown=>String::new()})}</p>
            <Show when=move ||state.with(|s|s.completion_available&&!s.complete)><p role="status">{t("已验证的中断初始化可以受保护补全，请明确选择下方操作。","Verified interrupted initialization can be completed with local authorization. Explicitly select the completion action below.")}</p></Show>
            <Show when=move ||state.with(|s|!s.complete&&((s.empty&&!s.unknown)||s.completion_available))>
                <div class="zxs-flow">
                    <Show when=move ||state.with(|s|s.empty&&!s.unknown)><button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(|s|choose(s,Action::Wizard))>{t("存储盘设置向导","Storage disk setup wizard")}</button>
                    <Show when=move ||state.with(|s|!s.presets.is_empty())><button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(|s|choose(s,Action::Preset))>{t("使用本地预置方案","Use local configuration preset")}</button></Show></Show>
                    <Show when=move ||state.with(|s|s.completion_available)><button data-setup-completion="" type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(|s|choose(s,Action::Complete))>{t("完成中断的初始化","Complete interrupted initialization")}</button></Show>
                </div>
                <Show when=move ||state.with(|s|s.action==Action::Complete)><p role="status">{t("服务器已验证原始授权对应的首次初始化中断证明。此操作仅补全尚未建立认证权威的初始化，不重置已有管理员；权威标记已存在但凭据缺失时仍禁止补全。","The server verified proof of interrupted first-time initialization bound to the original authorization. This only completes initialization before authentication authority exists; it never resets an existing administrator. A missing primary with an existing authority marker remains blocked.")}</p></Show>
                <Show when=move ||state.with(|s|s.action==Action::Preset)>{preset_view(state)}</Show>
                <Show when=move ||state.with(|s|s.action==Action::Wizard&&s.empty&&!s.unknown)>
                <p>{t("最少一个存储盘，不需要配置二级或后续备份。单盘仅保存一份，没有磁盘冗余；该盘损坏可能导致数据丢失，但系统盘上的 Web 管理仍可使用。多盘向导按选择顺序建立备份链，自定义布局请用本地预置方案。","One storage disk is sufficient; second-level and later backups are optional. A single disk holds one copy with no disk redundancy: its failure can lose data, while system-disk Web management remains accessible. Multiple selected disks form a backup chain in selection order; use local presets for custom layouts.")}</p>
                <fieldset disabled=move ||state.with(|s|s.pending)><legend>{t("允许的硬盘 ID","Allowed disk IDs")}</legend>
                {move ||state.with(|s|s.disks.clone()).into_iter().map(move |id|{let label=id.clone();let selected_id=id.clone();view!{<label><input data-setup-disk="" type="checkbox" prop:checked=move ||state.with(|s|s.selected.contains(&selected_id)) on:change=move |event|{let checked=event_target_checked(&event);state.update(|s|{s.selected.retain(|value|value!=&id);if checked{s.selected.push(id.clone());}s.confirmed=false;});}/>{label}</label>}}).collect_view()}
                </fieldset>
                <Show when=move ||state.with(|s|s.allow_development)><label><input data-setup-development="" type="checkbox" prop:checked=move ||state.with(|s|s.development) disabled=move ||state.with(|s|s.pending) on:change=move |event|state.update(|s|{s.development=event_target_checked(&event);s.confirmed=false;})/>{t("使用开发模拟磁盘（非生产）","Use development simulation disks (not production)")}</label></Show>
                </Show>
                <label>{t("管理员用户名","Administrator username")}<input class="zxs-input" data-setup-user="" autocomplete="username" prop:value=move ||state.with(|s|s.username.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.username=event_target_value(&event);s.confirmed=false;})/></label>
                <label>{t("管理员密码","Administrator password")}<input class="zxs-input" data-setup-password="" type="password" autocomplete="new-password" prop:value=move ||state.with(|s|s.password.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.password=event_target_value(&event);s.confirmed=false;})/></label>
                <label><input data-setup-confirm="" type="checkbox" prop:checked=move ||state.with(|s|s.confirmed) disabled=move ||state.with(|s|s.pending) on:change=move |event|state.update(|s|s.confirmed=event_target_checked(&event))/>{move ||state.with(|s|match s.action {
                    Action::Wizard => t("我确认这些是新安装的空白磁盘，并授权按所选模式创建存储池和管理员。","I confirm these are empty disks for a new installation and authorize the selected mode and administrator."),
                    Action::Preset => t("我已核对所选本地方案的摘要，授权按原方案初始化，不修改其盘数或参数。","I reviewed the selected local preset summary and authorize initialization without changing its disk count or parameters."),
                    Action::Complete => t("我确认使用原始本地授权，仅补全服务器已验证的这次中断初始化，不创建替代旧管理员的重置操作。","Using the original local authorization, I confirm completion of this verified interrupted initialization only, not an existing administrator reset."),
                })}</label>
                <button type="submit" class="zxs-button" disabled=move ||state.with(|s|!can_initialize(s))>{move ||state.with(|s|if s.action==Action::Complete {t("确认补全初始化","Confirm initialization completion")} else {t("初始化新安装","Initialize new installation")})}</button>
            </Show>
            <Show when=move ||state.with(|s|s.complete)><p role="status">{move ||state.with(|s|if s.restart_required {t("初始化完成。请在服务器本地重启服务，然后使用新管理员登录。","Initialization completed. Restart the service locally, then sign in with the new administrator.")} else {t("设置已接受，服务正在打开存储。请检查管理状态后使用新管理员登录；不要再次初始化。","Setup accepted; the service is opening storage. Check management status, then sign in with the new administrator. Do not initialize again.")})}</p></Show>
            <Show when=move ||state.with(|s|s.unknown)><p role="alert">{t("结果未知，已禁用重复初始化。请重新验证授权或在服务器本地核实状态；仅服务器确认的中断补全可以继续。","Outcome unknown. Repeated initialization is disabled. Recheck authorization or verify locally; only server-proven interrupted completion may proceed.")}</p></Show>
            {move ||state.with(|s|s.failure.clone()).map(|message|view!{<ErrorState message=message/>})}
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(|s|{let generation=s.generation+1;let unknown=s.unknown;let complete=s.complete;let restart_required=s.restart_required;*s=Model{generation,unknown,complete,restart_required,..Default::default()};})>{t("清除设置输入","Clear setup inputs")}</button>
        </form></details>
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/setup/tests.rs"]
mod tests;
