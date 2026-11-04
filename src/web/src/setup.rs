//! First-time management setup never selects or initializes data storage.
//! The one-time account endpoint is open only while no account or prior authority exists.
use crate::api;
use data_serialization::Deserialize;
use json_codec::{Value, json};
use web_interface::{components::ErrorState, i18n::t, prelude::*, tasks::spawn_local};

mod response;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Status {
    #[default]
    Unknown,
    Empty,
    Existing,
    ManagementReady,
    RecoveryRequired,
    Unavailable,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct SetupStatus {
    state: Status,
}

#[derive(Clone, Debug, error_definition::Error)]
enum Failure {
    #[error("{0}")]
    Request(#[source] api::Error),
    #[error(
        "{}",
        t(
            "设置响应未确认。请检查状态或尝试登录，勿重复初始化。",
            "Setup was not confirmed. Check status or try signing in; do not initialize again."
        )
    )]
    Response(#[source] std::sync::Arc<response::ResponseError>),
    #[error(
        "{}",
        t(
            "设置状态响应无效，请重新检查。",
            "The setup status response is invalid. Check again."
        )
    )]
    Status(#[source] std::sync::Arc<json_codec::Error>),
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    username: String,
    password: String,
    pending: bool,
    confirmed: bool,
    unknown: bool,
    complete: bool,
    generation: u64,
    failure: Option<Failure>,
    status: Status,
}

fn can_initialize(s: &Model) -> bool {
    !s.pending
        && !s.complete
        && !s.unknown
        && s.status == Status::Empty
        && s.confirmed
        && !s.username.trim().is_empty()
        && !s.password.is_empty()
}

fn check(state: RwSignal<Model>) {
    let Some(generation) = state
        .try_update(|s| {
            if s.pending || s.complete {
                return None;
            }
            s.pending = true;
            s.status = Status::Unknown;
            s.confirmed = false;
            s.failure = None;
            s.generation += 1;
            Some(s.generation)
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::setup_request("GET", "/api/setup/status", None).await;
        let _ = state.try_update(|s| finish_check(s, generation, result));
    });
}

fn finish_check(s: &mut Model, generation: u64, result: Result<Value, api::Error>) {
    if s.generation != generation {
        return;
    }
    s.pending = false;
    match result {
        Ok(value) => match json_codec::from_value::<SetupStatus>(value) {
            Ok(status) => {
                s.status = status.state;
                if status.state == Status::ManagementReady {
                    s.complete = true;
                    s.unknown = false;
                    s.failure = None;
                    clear_secrets(s);
                }
            }
            Err(error) => s.failure = Some(Failure::Status(std::sync::Arc::new(error))),
        },
        Err(error) => s.failure = Some(Failure::Request(error)),
    }
}

fn submission(s: &Model, password: String) -> Value {
    json!({"username": s.username, "password": password})
}

fn initialize(state: RwSignal<Model>) {
    let Some((body, generation)) = state
        .try_update(|s| {
            if !can_initialize(s) {
                return None;
            }
            s.pending = true;
            s.generation += 1;
            s.failure = None;
            let password = std::mem::take(&mut s.password);
            Some((submission(s, password), s.generation))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::setup_request("POST", "/api/setup/initialize", Some(body)).await;
        let _ = state.try_update(|s| {
            if s.generation == generation {
                finish_initialization(s, result);
            }
        });
    });
}

fn finish_initialization(s: &mut Model, result: Result<Value, api::Error>) {
    s.pending = false;
    s.confirmed = false;
    s.status = Status::Unknown;
    match result {
        Ok(value) => match response::validate(value) {
            Ok(()) => {
                s.complete = true;
                s.unknown = false;
                s.status = Status::ManagementReady;
                s.failure = None;
            }
            Err(error) => {
                s.unknown = true;
                s.failure = Some(Failure::Response(std::sync::Arc::new(error)));
            }
        },
        Err(error) => {
            s.unknown |= error.mutation_outcome_unknown();
            s.failure = Some(Failure::Request(error));
        }
    }
}

impl Model {
    pub(crate) fn reload_blocked(&self) -> bool {
        self.pending || self.unknown
    }
}

fn clear_secrets(s: &mut Model) {
    s.password.clear();
    s.confirmed = false;
}

pub(crate) fn view(state: RwSignal<Model>) -> impl IntoView {
    on_cleanup(move || state.update(clear_secrets));
    check(state);
    view! {
        <section class="zxs-content-panel zxs-stack"><h2>{t("首次设置", "First-time setup")}</h2>
        <p class="zxs-row">{t("未设置账户时，任何可访问此 Web 的人都可以创建唯一管理员账户。请先创建管理员账户。登录后在硬盘管理中添加硬盘，并设置挂载目录和实际存储目录；此步骤不会访问或初始化数据盘。", "Before an account is set, anyone who can access this Web page can create the sole administrator. Create the administrator account first. After signing in, add disks and set their mount and storage directories in disk management. This step does not access or initialize data disks.")}</p>
        <form class="zxs-stack" data-first-account-form="" on:submit=move |event|{event.prevent_default();initialize(state);}>
            <button type="button" class="zxs-button" data-setup-check="" disabled=move ||state.with(|s|s.pending||s.complete) on:click=move |_|check(state)>{t("检查设置状态", "Check setup status")}</button>
            <p class="zxs-row" role="status">{move ||state.with(|s|match s.status {
                Status::Empty => t("可以创建管理员账户，无需添加硬盘。", "Ready to create the administrator account. No disks are required."),
                Status::ManagementReady => t("管理员账户已建立，请登录后添加硬盘。", "The administrator account is ready. Sign in to add disks."),
                Status::Existing => t("已存在安装，请登录。", "An installation already exists. Sign in."),
                Status::RecoveryRequired => t("需要恢复现有安装，禁止重新初始化。", "The existing installation needs recovery. Reinitialization is blocked."),
                Status::Unavailable => t("无法确认系统状态，禁止初始化。", "The system state could not be verified. Initialization is blocked."),
                Status::Unknown => String::new(),
            })}</p>
            <Show when=move ||state.with(|s|s.status==Status::Empty&&!s.unknown&&!s.complete)>
                <div class="zxs-form-grid"><label class="zxs-form-field"><span>{t("管理员用户名", "Administrator username")}</span><input class="zxs-input" data-setup-user="" autocomplete="username" prop:value=move ||state.with(|s|s.username.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.username=event_target_value(&event);s.confirmed=false;})/></label>
                <label class="zxs-form-field"><span>{t("管理员密码", "Administrator password")}</span><input class="zxs-input" data-setup-password="" type="password" autocomplete="new-password" prop:value=move ||state.with(|s|s.password.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.password=event_target_value(&event);s.confirmed=false;})/></label></div>
                <label class="zxs-row"><input data-setup-confirm="" type="checkbox" prop:checked=move ||state.with(|s|s.confirmed) disabled=move ||state.with(|s|s.pending) on:change=move |event|state.update(|s|s.confirmed=event_target_checked(&event))/>{t("确认创建此管理员账户。", "Create this administrator account.")}</label>
                <button type="submit" class="zxs-button" disabled=move ||state.with(|s|!can_initialize(s))>{t("完成首次设置", "Complete first-time setup")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.complete)><p class="zxs-row" role="status">{t("首次设置完成。请使用新管理员登录，在硬盘管理中添加存储。", "First-time setup is complete. Sign in with the new administrator and add storage in disk management.")}</p></Show>
            <Show when=move ||state.with(|s|s.unknown)><p class="zxs-row" role="alert">{t("结果未知，已禁止重复初始化。请检查状态，或尝试使用刚设置的账户登录。", "Outcome unknown. Repeated initialization is blocked. Check status, or try signing in with the account you just configured.")}</p></Show>
            {move ||state.with(|s|s.failure.as_ref().map(ToString::to_string)).map(|message|view!{<ErrorState message=message/>})}
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(|s|{let generation=s.generation+1;let unknown=s.unknown;let complete=s.complete;*s=Model{generation,unknown,complete,..Default::default()};})>{t("清除设置输入", "Clear setup inputs")}</button>
        </form></section>
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/setup/tests.rs"]
mod tests;
