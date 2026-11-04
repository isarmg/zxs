//! First-time management setup never selects or initializes data storage.
//! The one-time account endpoint is open only while no account or prior authority exists.
use crate::api;
use data_serialization::Deserialize;
use json_codec::{Value, json};
use web_interface::{
    components::{ErrorState, Feedback},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Screen {
    #[default]
    SignIn,
    CreateAccount,
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    username: String,
    password: String,
    pending: bool,
    unknown: bool,
    complete: bool,
    generation: u64,
    failure: Option<Failure>,
    status: Status,
    screen: Screen,
}

fn can_initialize(s: &Model) -> bool {
    !s.pending
        && !s.complete
        && !s.unknown
        && s.status == Status::Empty
        && !s.username.trim().is_empty()
        && !s.password.is_empty()
}

pub(crate) fn check(state: RwSignal<Model>) {
    let Some(generation) = state
        .try_update(|s| {
            if s.pending || s.complete {
                return None;
            }
            s.pending = true;
            s.status = Status::Unknown;
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
                if status.state == Status::Empty {
                    s.screen = Screen::CreateAccount;
                } else if status.state == Status::Existing && !s.unknown {
                    s.screen = Screen::SignIn;
                    clear_secrets(s);
                }
                if status.state == Status::ManagementReady {
                    s.screen = Screen::SignIn;
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
    s.status = Status::Unknown;
    match result {
        Ok(value) => match response::validate(value) {
            Ok(()) => {
                s.screen = Screen::SignIn;
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
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn retry_available(&self) -> bool {
        self.unknown
            || self.failure.is_some()
            || (!self.pending
                && matches!(
                    self.status,
                    Status::Unknown | Status::Unavailable | Status::RecoveryRequired
                ))
    }

    pub(crate) fn feedback(&self) -> Option<Feedback> {
        let failure = self.failure.as_ref().map(ToString::to_string);
        let message = if self.unknown {
            let warning = t(
                "结果未确认，请检查设置状态。",
                "Outcome unconfirmed. Check setup status.",
            );
            Some(match failure {
                Some(failure) => format!("{warning} {failure}"),
                None => warning,
            })
        } else {
            failure.or_else(|| match self.status {
                Status::RecoveryRequired => Some(t(
                    "现有账户需要恢复，不能重新设置。",
                    "The existing account needs recovery and cannot be set up again.",
                )),
                Status::Unavailable => Some(t(
                    "无法确认账户状态，请重试。",
                    "The account state could not be verified. Try again.",
                )),
                _ => None,
            })
        };
        message.map(|message| Feedback {
            message,
            request_id: match &self.failure {
                Some(Failure::Request(error)) => error.request_id.clone(),
                _ => None,
            },
        })
    }

    pub(crate) fn authenticated(&mut self) {
        self.generation += 1;
        self.pending = false;
        self.unknown = false;
        self.complete = true;
        self.status = Status::ManagementReady;
        self.screen = Screen::SignIn;
        self.failure = None;
        clear_secrets(self);
    }

    pub(crate) fn reload_blocked(&self) -> bool {
        self.pending || self.unknown
    }
}

fn clear_secrets(s: &mut Model) {
    s.password.clear();
}

pub(crate) fn view(state: RwSignal<Model>, login: ChildrenFn) -> impl IntoView {
    on_cleanup(move || state.update(clear_secrets));
    check(state);
    let create = Memo::new(move |_| state.with(|s| s.screen == Screen::CreateAccount));
    view! {
        <Show when=move ||create.get() fallback=move ||login()>
            {account_form(state)}
        </Show>
    }
}

fn clear_inputs(s: &mut Model) {
    if s.pending {
        return;
    }
    s.generation += 1;
    s.username.clear();
    clear_secrets(s);
    s.failure = None;
}

fn account_form(state: RwSignal<Model>) -> impl IntoView {
    let feedback = Memo::new(move |_| state.with(Model::feedback));
    view! {
        <form data-first-account-form="" aria-label=t("设置账户", "Set up account") aria-busy=move ||state.with(|s|s.pending).to_string() on:submit=move |event|{event.prevent_default();initialize(state);}>
            <label class="zxs-auth-label" for="zxs-setup-username">{t("账户名", "Account name")}</label>
            <input id="zxs-setup-username" class="zxs-input" name="username" data-setup-user="" autocomplete="username" maxlength="64" prop:value=move ||state.with(|s|s.username.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.username=event_target_value(&event);s.failure=None;})/>
            <label class="zxs-auth-label" for="zxs-setup-password">{t("密码", "Password")}</label>
            <input id="zxs-setup-password" class="zxs-input" name="password" data-setup-password="" type="password" autocomplete="new-password" maxlength="1024" prop:value=move ||state.with(|s|s.password.clone()) disabled=move ||state.with(|s|s.pending) on:input=move |event|state.update(|s|{s.password=event_target_value(&event);s.failure=None;})/>
            <div class="zxs-auth-information" data-setup-unknown=move ||state.with(|s|s.unknown.then_some(""))>
                {move ||feedback.get().map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
                <Show when=move ||feedback.get().is_none()><span role="status">{t("请设置账户名和密码", "Please set an account name and password.")}</span></Show>
            </div>
            <div class="zxs-auth-actions">
                <button type="submit" class="zxs-button" disabled=move ||state.with(|s|!can_initialize(s))>{t("设置账户", "Set up account")}</button>
                <button type="button" class="zxs-button" data-setup-check="" aria-label=t("检查设置状态", "Check setup status") title=t("检查设置状态", "Check setup status") disabled=move ||state.with(|s|s.pending||s.complete) on:click=move |_|check(state)><svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 11-2l3 3M4 16l3 3a7 7 0 0 0 11-2"/></svg></button>
                <Show when=move ||state.with(|s|s.unknown)><button type="button" class="zxs-button" data-setup-sign-in="" on:click=move |_|state.update(|s|{s.screen=Screen::SignIn;clear_secrets(s);})>{t("尝试登录", "Try signing in")}</button></Show>
                <button type="button" class="zxs-button" data-setup-clear="" aria-label=t("清除设置输入", "Clear setup inputs") title=t("清除设置输入", "Clear setup inputs") disabled=move ||state.with(|s|s.pending) on:click=move |_|state.update(clear_inputs)><svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="m6 6 12 12M18 6 6 18"/></svg></button>
            </div>
        </form>
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/setup/tests.rs"]
mod tests;
