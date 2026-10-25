use crate::{admin, api, files, history, icons::FileIcon, management, monitor};
use json_codec::{Value, json};
use web_interface::components::{
    AccountPage, AppShell, ErrorState, Feedback, HeaderActions, HeaderNavigation, LoginCredentials,
    LoginPage, PasswordChange, SessionPhase, SessionView,
};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Files,
    Details,
    Health,
    Admin,
    Status,
    History,
    Account,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SessionTransition {
    #[default]
    Idle,
    Restoring,
    LoggingOut,
    LogoutUnconfirmed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PasswordChangeState {
    #[default]
    Idle,
    Pending,
    Unknown,
}

#[derive(Clone, Default)]
pub(crate) struct State {
    pub authenticated: bool,
    pub session_epoch: u64,
    pub principal: String,
    pub session_transition: SessionTransition,
    pub session_failure: Option<Feedback>,
    pub login_failure: Option<Feedback>,
    // A lost response may follow a committed change and revoked sessions.
    // Only a settled sign-out/new session clears this non-idempotent fence.
    pub password_change: PasswordChangeState,
    pub password_failure: Option<Feedback>,
    pub page: Page,
    pub login_in_flight: bool,
    pub errors: Vec<Feedback>,
    pub notice: String,
    pub files: files::Model,
    pub monitor: monitor::Model,
    pub management: management::Model,
    pub admin_console: admin::Model,
    pub activity: history::Model,
}

impl State {
    pub(crate) fn restoring(&self) -> bool {
        self.session_transition == SessionTransition::Restoring
    }
    pub(crate) fn logout_pending(&self) -> bool {
        self.session_transition == SessionTransition::LoggingOut
    }
    pub(crate) fn logout_unconfirmed(&self) -> bool {
        self.session_transition == SessionTransition::LogoutUnconfirmed
    }
    fn password_pending(&self) -> bool {
        self.password_change == PasswordChangeState::Pending
    }
    fn password_unknown(&self) -> bool {
        self.password_change == PasswordChangeState::Unknown
    }

    /// App-owned reloads must not destroy in-memory operation recovery fences.
    fn reload_blocked(&self) -> bool {
        self.login_in_flight
            || self.session_transition != SessionTransition::Idle
            || self.password_change != PasswordChangeState::Idle
            || self.management.retrying
            || self.management.retry_unknown
            || self.admin_console.reload_blocked()
            || files::pending(self)
    }

    fn password_change_blocked(&self) -> bool {
        !self.session_active() || self.password_pending() || self.password_unknown()
    }

    /// Business work belongs only to a settled, authenticated session. Logout
    /// retains credentials for revocation retries, not for admitting new work.
    pub(crate) fn session_active(&self) -> bool {
        self.authenticated && self.session_transition == SessionTransition::Idle
    }
}

pub(crate) fn current_session(state: RwSignal<State>, epoch: u64) -> bool {
    state
        .try_with_untracked(|s| s.session_active() && s.session_epoch == epoch)
        .unwrap_or(false)
}

/// Check session ownership before a read's resource/draft ownership. Closing a
/// panel or superseding a read cannot hide expiry of that same session. An old
/// session's response, however, must never revoke a replacement login.
pub(crate) fn accept_read_response(
    state: RwSignal<State>,
    epoch: u64,
    result: &Result<Value, api::Error>,
) -> bool {
    if !current_session(state, epoch) {
        return false;
    }
    if let Err(error) = result
        && error.status == 401
    {
        fail(state, error.clone());
        return false;
    }
    true
}

pub(crate) fn list(value: &Value, key: &str) -> Vec<Value> {
    value
        .as_array()
        .or_else(|| value.get(key).and_then(Value::as_array))
        .cloned()
        .unwrap_or_default()
}
pub(crate) fn text(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}
pub(crate) fn feedback(error: &api::Error) -> Feedback {
    Feedback {
        message: format!("HTTP {} · {}", error.status, error),
        request_id: error.request_id.clone(),
    }
}
pub(crate) fn fail(state: RwSignal<State>, error: api::Error) {
    state.update(|s| {
        if error.status == 401 {
            s.authenticated = false;
        }
        s.errors.insert(0, feedback(&error));
        s.errors.truncate(5);
    });
}
pub(crate) fn set_notice(state: RwSignal<State>, notice: String) {
    state.update(|s| s.notice = notice);
}

pub fn app() -> impl IntoView {
    let state = RwSignal::new(State {
        page: Page::Files,
        ..Default::default()
    });
    let setup_state = RwSignal::new(crate::setup::Model::default());
    provide_context(web_interface::components::LanguageSwitchGuard(
        Signal::derive(move || {
            state.with(State::reload_blocked)
                || setup_state.with(crate::setup::Model::reload_blocked)
        }),
    ));
    restore_session(state);
    crate::polling::install(state);
    let session = Memo::new(move |_| state.with(session_view));
    let logout_pending = Memo::new(move |_| state.with(|s| s.logout_pending()));
    let login_pending = Memo::new(move |_| {
        state.with(|s| {
            s.login_in_flight || s.restoring() || s.logout_pending() || s.logout_unconfirmed()
        })
    });
    let login_failure = Memo::new(move |_| state.with(|s| s.login_failure.clone()));
    let on_login = Callback::new(move |credentials: LoginCredentials| {
        let Some(epoch) = state.try_update(begin_login).flatten() else {
            return;
        };
        spawn_local(async move {
            let result = api::login(&credentials.username, &credentials.password).await;
            if !state.with_untracked(|s| {
                s.session_epoch == epoch && !s.logout_pending() && !s.logout_unconfirmed()
            }) {
                return;
            }
            match result {
                Ok(value) => {
                    state.update(|s| {
                        let login_in_flight = s.login_in_flight;
                        let session_epoch = s.session_epoch;
                        *s = State {
                            session_epoch,
                            authenticated: true,
                            principal: value.principal,
                            page: Page::Files,
                            login_in_flight,
                            ..Default::default()
                        };
                    });
                    management::load(state);
                }
                Err(error) => state.update(|s| s.login_failure = Some(feedback(&error))),
            }
            state.update(|s| s.login_in_flight = false);
        });
    });
    let login: ChildrenFn = ToChildren::to_children(
        move || view! { {management::summary(state)} {move ||state.with(|s|s.session_failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})} <LoginPage pending=login_pending.into() remote_failure=login_failure.into() on_submit=on_login/> {crate::setup::view(setup_state)} },
    );
    view! {
        <AppShell product="zxs" session=session.into() pending_logout=logout_pending.into() on_logout=Callback::new(move |_|logout(state)) on_account=Callback::new(move |_|navigate(state,Page::Account)) on_restore=Callback::new(move |_|restore_session(state)) login=login>
            {workspace(state)}
            <Show when=move ||state.with(|s|!s.notice.is_empty())><p class="zxs-status zxs-row" role="status" aria-live="polite">{move ||state.with(|s|s.notice.clone())}</p></Show>
            <Show when=move ||state.with(|s|!s.errors.is_empty())><aside class="zxs-error-center zxs-stack" aria-label=t("请求错误","Request errors")>{move ||state.with(|s|s.errors.clone()).into_iter().enumerate().map(move |(index,error)|view! {
                <div class="zxs-inline-feedback zxs-flow"><ErrorState message=error.message request_id=error.request_id/><button class="zxs-button" aria-label=t("关闭错误提示","Dismiss error") on:click=move |_|state.update(|s|{if index<s.errors.len(){s.errors.remove(index);}})>"×"</button></div>
            }).collect_view()}</aside></Show>
        </AppShell>
    }
}
pub(crate) fn navigate(state: RwSignal<State>, page: Page) {
    state.update(|s| {
        s.page = page;
    });
    match page {
        Page::Health => management::load(state),
        Page::Status => monitor::load(state),
        Page::History => history::load(state),
        _ => {}
    }
}
fn workspace(state: RwSignal<State>) -> impl IntoView {
    let page = Memo::new(move |_| state.with(|s| s.page));
    view! {
        <HeaderNavigation label=t("主导航","Main navigation")>
            {[ (Page::Health,t("管理健康","Management health")), (Page::Files,t("文件","Files")), (Page::Details,t("详情","Details")), (Page::Status,t("状态","Status")), (Page::History,t("历史记录","History")) ].into_iter().map(move |(page,title)|view! {
                <button class="zxs-button" aria-current=move ||(state.with(|s|s.page==page)).then_some("page") on:click=move |_|navigate(state,page)>{title}</button>
            }).collect_view()}
            <Show when=move ||state.with(|s|s.session_active())><button class="zxs-button" aria-current=move ||state.with(|s|(s.page==Page::Admin).then_some("page")) on:click=move |_|navigate(state,Page::Admin)>{t("系统管理","Administration")}</button></Show>
        </HeaderNavigation>
        <HeaderActions><div class="zxs-header-tools zxs-flow">
            <Show when=move ||state.with(|s|matches!(s.page,Page::Files|Page::Details|Page::Status|Page::History))>
                <button class="zxs-button zxs-header-refresh" type="button" aria-label=t("刷新","Refresh") title=t("刷新","Refresh") disabled=move ||state.with(|s|match s.page{Page::Files=>s.files.loading||s.files.loading_disks,Page::Details=>s.files.loading||s.files.history_loading||s.files.selected.is_none(),Page::Status=>s.monitor.loading||!s.session_active(),Page::History=>s.activity.loading,_=>true}) on:click=move |_|match state.with_untracked(|s|s.page){Page::Files|Page::Details=>if state.with_untracked(|s|s.files.slot.is_empty()){files::load_disks(state)}else{files::load_entries(state)},Page::Status=>monitor::load(state),Page::History=>history::load(state),_=>{}}><FileIcon name="refresh"/></button>
            </Show>
            <span class="zxs-session-identity">{move ||state.with(|s|s.principal.clone())}</span>
        </div></HeaderActions>
        <Show when=move ||state.with(|s|s.page!=Page::Health&&!s.management.storage.writable())>{management::summary(state)}</Show>
        {move ||match page.get() { Page::Health=>management::view(state).into_any(),Page::Admin=>admin::view(state).into_any(),Page::Details=>files::details(state).into_any(),Page::Status=>monitor::view(state).into_any(),Page::History=>history::view(state).into_any(),Page::Account=>account(state).into_any(),Page::Files=>files::view(state).into_any() }}
    }
}
fn session_view(state: &State) -> SessionView {
    SessionView {
        phase: if state.restoring() {
            SessionPhase::Loading
        } else if state.logout_unconfirmed() || (state.logout_pending() && !state.authenticated) {
            // Retrying revocation is still unconfirmed. Keep the recovery shell
            // until the response settles, rather than exposing login or setup.
            SessionPhase::LogoutUnconfirmed
        } else if state.authenticated {
            SessionPhase::Authenticated
        } else {
            SessionPhase::Anonymous
        },
        username: state.principal.clone(),
        message: state
            .session_failure
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_else(|| {
                if state.authenticated {
                    String::new()
                } else {
                    state.notice.clone()
                }
            }),
        request_id: state
            .session_failure
            .as_ref()
            .and_then(|e| e.request_id.clone()),
    }
}
// Session transitions own an epoch before their first asynchronous step.
// Disable duplicate submissions at the action boundary as well as in the view.
fn begin_login(state: &mut State) -> Option<u64> {
    if state.login_in_flight
        || state.restoring()
        || state.logout_pending()
        || state.logout_unconfirmed()
    {
        return None;
    }
    state.login_in_flight = true;
    state.session_epoch += 1;
    state.login_failure = None;
    state.management.loading = false;
    Some(state.session_epoch)
}
fn begin_restore(state: &mut State) -> Option<u64> {
    if state.login_in_flight
        || state.restoring()
        || state.logout_pending()
        || state.logout_unconfirmed()
    {
        return None;
    }
    state.session_transition = SessionTransition::Restoring;
    state.session_epoch += 1;
    state.session_failure = None;
    Some(state.session_epoch)
}
fn restore_session(state: RwSignal<State>) {
    let Some(epoch) = state.try_update(begin_restore).flatten() else {
        return;
    };
    state.update(|s| s.management.loading = false);
    management::load(state);
    spawn_local(async move {
        let result = api::restore().await;
        if !state.with_untracked(|s| {
            s.session_epoch == epoch && !s.logout_pending() && !s.logout_unconfirmed()
        }) {
            return;
        }
        match result {
            Ok(value) => {
                state.update(|s| {
                    s.authenticated = true;
                    s.principal = value.principal;
                });
            }
            Err(error) => state.update(|s| {
                s.authenticated = false;
                if error.status != 401 {
                    s.session_failure = Some(feedback(&error));
                }
            }),
        }
        state.update(|s| s.session_transition = SessionTransition::Idle);
        management::load(state);
    });
}
pub(crate) fn begin_logout(state: &mut State) -> Option<u64> {
    if state.logout_pending() {
        return None;
    }
    state.session_transition = SessionTransition::LoggingOut;
    state.session_epoch += 1;
    api::invalidate_session_work();
    Some(state.session_epoch)
}
fn logout(state: RwSignal<State>) {
    let Some(epoch) = state.try_update(begin_logout).flatten() else {
        return;
    };
    // Finish the requested logout even if the UI is disposed while it is pending.
    js_sys::futures::spawn_local(async move {
        let result = api::request("POST", "/api/auth/logout", Some(json!({}))).await;
        if !state
            .try_with_untracked(|s| s.session_epoch == epoch && s.logout_pending())
            .unwrap_or(false)
        {
            return;
        }
        complete_logout(state, result);
    });
}
pub(crate) fn complete_logout(state: RwSignal<State>, result: Result<Value, api::Error>) {
    let failure = match result {
        Ok(value) if storage_contract::client_protocol::logout_confirmed(&value) => {
            end_session(state, String::new());
            return;
        }
        Err(error) if error.status == 401 => {
            end_session(state, String::new());
            return;
        }
        Err(error) => feedback(&error),
        Ok(_) => t(
            "未收到有效的退出确认，请重试退出；不会恢复会话或重复登录。",
            "Sign-out was not confirmed. Retry signing out; session restoration and sign-in remain blocked.",
        ).as_str().into(),
    };
    state.update(|s| {
        s.authenticated = false;
        s.session_transition = SessionTransition::LogoutUnconfirmed;
        s.session_failure = Some(failure);
    });
}
fn end_session(state: RwSignal<State>, notice: String) {
    api::clear_session();
    let session_epoch = state.with_untracked(|s| s.session_epoch + 1);
    state.set(State {
        session_epoch,
        page: Page::Files,
        notice,
        ..Default::default()
    });
}
fn account(state: RwSignal<State>) -> impl IntoView {
    let username = Memo::new(move |_| state.with(|s| s.principal.clone()));
    let pending = Memo::new(move |_| state.with(State::password_change_blocked));
    let failure = Memo::new(move |_| state.with(|s| s.password_failure.clone()));
    let change = Callback::new(move |credentials: PasswordChange| {
        if state.with_untracked(State::password_change_blocked) {
            return;
        }
        state.update(|s| {
            s.password_change = PasswordChangeState::Pending;
            s.password_failure = None;
        });
        let epoch = state.with_untracked(|s| s.session_epoch);
        spawn_local(async move {
            let result = api::request(
                "POST",
                "/api/auth/password",
                Some(json!({
                    "current_password": credentials.current_password,
                    "new_password": credentials.new_password,
                })),
            )
            .await;
            if !current_session(state, epoch) {
                return;
            }
            complete_password_change(state, result);
        });
    });
    view! { <section class="zxs-content-stack zxs-stack"><p class="zxs-row">{t("修改密码后，该账号的全部会话将失效。","Changing the password signs this account out on all devices.")}</p><Show when=move ||state.with(|s|s.password_unknown())><p class="zxs-row" role="alert">{t("改密结果未知，已禁用重复提交。请先退出登录，再核实可用的密码；不会自动重试改密。","The password change outcome is unknown; repeat submission is disabled. Sign out, then verify which password works. The change will not be retried automatically.")}</p></Show><AccountPage username_value=username.into() pending=pending.into() remote_failure=failure.into() on_submit=change/></section> }
}
fn complete_password_change(state: RwSignal<State>, result: Result<Value, api::Error>) {
    match result {
        Ok(value) if storage_contract::client_protocol::password_change_confirmed(&value) => {
            end_session(
                state,
                t(
                    "密码已更新，请使用新密码登录。",
                    "Your password was changed. Sign in with the new password.",
                ),
            );
        }
        Ok(_) => state.update(reject_unconfirmed_password_change),
        Err(error) => state.update(|s| reject_password_change(s, &error)),
    }
}
fn reject_unconfirmed_password_change(state: &mut State) {
    state.password_change = PasswordChangeState::Unknown;
    state.password_failure = Some(
        t(
            "未收到有效的改密确认，请核对会话状态。",
            "The password change was not confirmed. Check your session.",
        )
        .as_str()
        .into(),
    );
}
fn reject_password_change(state: &mut State, error: &api::Error) {
    if error.status == 401 {
        state.authenticated = false;
    }
    if state.password_unknown() || error.mutation_outcome_unknown() {
        state.password_change = PasswordChangeState::Unknown;
    } else {
        state.password_change = PasswordChangeState::Idle;
    }
    state.password_failure = Some(feedback(error));
}

#[cfg(test)]
#[path = "../../../test/web/unit/ui/tests.rs"]
mod tests;
