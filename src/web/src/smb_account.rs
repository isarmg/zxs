//! SMB credentials are separate from the Web administrator's credentials.
use crate::{
    api,
    ui::{State, current_session, feedback},
};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::smb_account::{PasswordChangeAvailability, SmbAccountStatus};
use web_interface::{
    components::{ErrorState, Feedback, Select, SelectOption},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Clone, Default)]
pub(crate) struct Model {
    status: Option<SmbAccountStatus>,
    loading: bool,
    pending: bool,
    unknown: bool,
    failure: Option<Feedback>,
    changed: bool,
    selected: String,
    binding_unknown: bool,
    binding_changed: bool,
}
impl Model {
    pub(crate) fn reload_blocked(&self) -> bool {
        self.pending || self.unknown || self.binding_unknown
    }
    fn can_change(&self) -> bool {
        !self.loading
            && !self.pending
            && !self.unknown
            && !self.binding_unknown
            && matches!(
                &self.status,
                Some(SmbAccountStatus::Configured {
                    username,
                    password_change: PasswordChangeAvailability::Available,
                    ..
                }) if username == &self.selected
            )
    }
    fn generation(&self) -> Option<u64> {
        match &self.status {
            Some(SmbAccountStatus::Configured { generation, .. }) => Some(*generation),
            _ => None,
        }
    }
    fn can_bind(&self) -> bool {
        !self.loading
            && !self.pending
            && !self.unknown
            && !self.binding_unknown
            && matches!(&self.status,
            Some(SmbAccountStatus::Configured { username, candidates, .. }) if self.selected != *username && candidates.contains(&self.selected))
    }
    fn loaded(&mut self, status: SmbAccountStatus) {
        self.selected = match &status {
            SmbAccountStatus::Configured { username, .. } => username.clone(),
            _ => String::new(),
        };
        self.status = Some(status);
        self.binding_unknown = false;
        self.binding_changed = false;
        self.changed = false;
    }
}

pub(crate) fn load(state: RwSignal<State>) {
    if state
        .with_untracked(|s| !s.session_active() || s.smb_account.loading || s.smb_account.pending)
    {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.smb_account.loading = true;
        s.smb_account.failure = None;
    });
    spawn_local(async move {
        let result = api::validated_request("GET", "/api/smb/account", None, |value| {
            SmbAccountStatus::deserialize(value).map_err(|source| {
                let mut error = api::error(
                    storage_contract::ErrorCode::InvalidResponse,
                    t("SMB 账户响应无效。", "The SMB account response is invalid."),
                );
                error.status = 200;
                error.source = Some(std::sync::Arc::new(api::ErrorSource::SmbAccount(source)));
                error
            })
        })
        .await;
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| s.smb_account.loading = false);
        match result {
            Ok(status) => state.update(|s| s.smb_account.loaded(status)),
            Err(error) => {
                state.update(|s| {
                    s.smb_account.status = None;
                    s.smb_account.failure = Some(feedback(&error));
                });
                if error.status == 401 {
                    crate::ui::fail(state, error);
                }
            }
        }
    });
}

fn complete(model: &mut Model, result: &Result<Value, api::Error>) {
    model.pending = false;
    match result {
        Ok(value) if value == &json!({"changed":true}) => {
            model.changed = true;
            model.failure = None;
        }
        Ok(_) => {
            model.unknown = true;
            model.failure = Some(
                t(
                    "SMB 改密未获确认，请先核实可用密码。",
                    "The SMB password change was not confirmed. Verify which password works first.",
                )
                .as_str()
                .into(),
            );
        }
        Err(error) => {
            model.unknown = error.mutation_outcome_unknown();
            model.failure = Some(feedback(error));
        }
    }
}

fn change(state: RwSignal<State>, current_password: String, new_password: String) {
    if state.with_untracked(|s| !s.session_active() || !s.smb_account.can_change()) {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    let generation = state.with_untracked(|s| {
        s.smb_account
            .generation()
            .expect("invariant: configured SMB generation")
    });
    state.update(|s| {
        s.smb_account.pending = true;
        s.smb_account.failure = None;
        s.smb_account.changed = false;
    });
    spawn_local(async move {
        let result = api::request(
            "POST",
            "/api/smb/password",
            Some(json!({"expected_generation":generation.to_string(),"current_password":current_password,"new_password":new_password})),
        )
        .await;
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| complete(&mut s.smb_account, &result));
        if let Err(error) = result
            && error.status == 401
        {
            crate::ui::fail(state, error);
        }
    });
}

fn confirmed_binding(
    value: Value,
    requested: &str,
    generation: u64,
) -> Result<SmbAccountStatus, api::Error> {
    match SmbAccountStatus::deserialize(value) {
        Ok(status @ SmbAccountStatus::Configured { .. })
            if matches!(&status, SmbAccountStatus::Configured { username, generation: saved, .. }
                if username == requested && Some(*saved) == generation.checked_add(1)) =>
        {
            Ok(status)
        }
        unconfirmed => {
            let mut error = api::error(
                storage_contract::ErrorCode::InvalidResponse,
                t(
                    "SMB 账号保存未获确认，请刷新读取实际账号。",
                    "SMB account save was not confirmed. Refresh to read the effective account.",
                ),
            );
            error.status = 200;
            if let Err(source) = unconfirmed {
                error.source = Some(std::sync::Arc::new(api::ErrorSource::SmbAccount(source)));
            }
            Err(error)
        }
    }
}

fn complete_binding(model: &mut Model, result: &Result<SmbAccountStatus, api::Error>) {
    model.pending = false;
    match result {
        Ok(status) => {
            model.loaded(status.clone());
            model.binding_changed = true;
        }
        Err(error) => {
            model.binding_unknown = error.mutation_outcome_unknown();
            model.failure = Some(feedback(error));
        }
    }
}

fn bind(state: RwSignal<State>, administrator_password: String) {
    if state.with_untracked(|s| !s.session_active() || !s.smb_account.can_bind()) {
        return;
    }
    let (epoch, username, generation) = state.with_untracked(|s| {
        (
            s.session_epoch,
            s.smb_account.selected.clone(),
            s.smb_account
                .generation()
                .expect("invariant: configured SMB generation"),
        )
    });
    state.update(|s| {
        s.smb_account.pending = true;
        s.smb_account.failure = None;
        s.smb_account.binding_changed = false;
        s.smb_account.changed = false;
    });
    spawn_local(async move {
        let requested = username.clone();
        let result = api::validated_request("POST", "/api/smb/account",
            Some(json!({"username":username,"expected_generation":generation.to_string(),"administrator_password":administrator_password})),
            move |value| confirmed_binding(value, &requested, generation)).await;
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| complete_binding(&mut s.smb_account, &result));
        if let Err(error) = result
            && error.status == 401
        {
            crate::ui::fail(state, error);
        }
    });
}

fn explanation(status: Option<&SmbAccountStatus>) -> String {
    use PasswordChangeAvailability::*;
    match status {
        None => t("正在读取 SMB 配置。", "Reading SMB configuration."),
        Some(SmbAccountStatus::Disabled {}) => t(
            "尚未配置 SMB。请由服务器管理员准备 Samba 账户并配置 --smb-user、共享 valid users 与本机桥接。",
            "SMB is not configured. A server administrator must provision a Samba account, --smb-user, share valid users and the local bridge.",
        ),
        Some(SmbAccountStatus::Configured {
            password_change, ..
        }) => match password_change {
            Available => t(
                "从服务器已预置的 Samba 账号中选择并保存当前绑定；保存会断开旧的 ZXS SMB 连接。下方修改所选生效账号的独立 SMB 密码，不修改 Web 登录密码。",
                "Select and save the active binding from Samba accounts provisioned on the server. Saving disconnects old ZXS SMB connections. The password form changes the effective account’s separate SMB password, not your Web password.",
            ),
            ClientUnavailable => t(
                "服务器未提供可安全执行的 /usr/bin/smbpasswd（不可带 setuid/setgid）。请由管理员检查 Samba 客户端后刷新。",
                "The server needs executable /usr/bin/smbpasswd without setuid/setgid permissions. Ask the administrator to check the Samba client, then refresh.",
            ),
            UnsupportedAccountName => t(
                "此名称不能由本地 SMB 改密客户端安全处理；请在服务器设置密码。",
                "This account name cannot be handled safely by the local SMB password client. Set its password on the server.",
            ),
            RequiresUnprivilegedService => t(
                "Web 改密要求 ZXS 以非 root 服务账户运行，不会提升权限或直接修改 Samba 凭据库。",
                "Web password changes require ZXS to run as a non-root service account. No privilege escalation or direct Samba credential database writes are used.",
            ),
        },
    }
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let current = NodeRef::<web_interface::html::Input>::new();
    let new_password = NodeRef::<web_interface::html::Input>::new();
    let confirmation = NodeRef::<web_interface::html::Input>::new();
    let input_failure = RwSignal::new(None::<String>);
    let administrator_password = NodeRef::<web_interface::html::Input>::new();
    let options = Signal::derive(move || {
        state.with(|s| match &s.smb_account.status {
            Some(SmbAccountStatus::Configured { candidates, .. }) => candidates
                .iter()
                .map(|name| SelectOption::new(name.clone(), name.clone()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
    });
    let selected = Signal::derive(move || state.with(|s| s.smb_account.selected.clone()));
    view! {
        <section class="zxs-content-stack zxs-account-page zxs-stack" aria-label=t("SMB 账号与密码", "SMB account and password")>
            <h2 class="zxs-row">{t("SMB 账号与密码", "SMB account and password")}</h2>
            <p class="zxs-row">{move ||state.with(|s| explanation(s.smb_account.status.as_ref()))}</p>
            <Show when=move ||state.with(|s| matches!(s.smb_account.status,Some(SmbAccountStatus::Configured { .. })))>
                <p class="zxs-row" data-smb-account="">{move ||state.with(|s| match &s.smb_account.status { Some(SmbAccountStatus::Configured { username, .. }) => format!("{}：{}",t("SMB 账号","SMB account"),username), _ => String::new() })}</p>
            </Show>
            <Show when=move ||state.with(|s| matches!(s.smb_account.status, Some(SmbAccountStatus::Configured { .. })))>
                <p class="zxs-row">{t("可选项必须已由服务器管理员配置 Unix/Samba 账户、密码及所有 ZXS 共享的 valid users，并使用 --smb-allowed-user 加入名单。这里真实切换 ZXS 的访问绑定，不创建或重命名系统账户。", "Candidates must already have Unix/Samba accounts, passwords and valid users access to every ZXS share, and be listed with --smb-allowed-user. This form changes ZXS access binding; it does not create or rename system accounts.")}</p>
                <form class="zxs-account-form zxs-stack" on:submit=move |event| {
                    event.prevent_default();
                    if state.with_untracked(|s|!s.session_active()||!s.smb_account.can_bind()) { return; }
                    let input = administrator_password.get_untracked().expect("invariant: mounted Web password confirmation");
                    let password = input.value(); input.set_value("");
                    for field in [current,new_password,confirmation] { if let Some(input) = field.get_untracked() { input.set_value(""); } }
                    bind(state,password);
                }>
                    <div class="zxs-form-grid">
                        <div class="zxs-form-field"><span class="zxs-row">{t("绑定 SMB 账号","Bind SMB account")}</span>
                            <Select show_label=false label=t("绑定 SMB 账号","Bind SMB account") options=options value=selected disabled=Signal::derive(move ||state.with(|s|!s.session_active()||s.smb_account.loading||s.smb_account.reload_blocked())) on_change=Callback::new(move |username:String|state.update(|s|{if !s.smb_account.loading && !s.smb_account.reload_blocked() { s.smb_account.selected=username; s.smb_account.binding_changed=false; s.smb_account.changed=false; }}))/></div>
                        <label class="zxs-form-field"><span class="zxs-row">{t("当前 Web 管理密码（确认账号切换）","Current Web administrator password (confirm account switch)")}</span><input class="zxs-input zxs-input--compact" type="password" name="smb_binding_administrator_password" autocomplete="current-password" maxlength="1024" required=true node_ref=administrator_password disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_bind())/></label>
                    </div>
                    <button class="zxs-button" type="submit" disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_bind())>{t("保存 SMB 账号","Save SMB account")}</button>
                </form>
            </Show>
            <button class="zxs-button" type="button" disabled=move ||state.with(|s|!s.session_active()||s.smb_account.loading||s.smb_account.pending) on:click=move |_| { for field in [current,new_password,confirmation,administrator_password] { if let Some(input) = field.get_untracked() { input.set_value(""); } } load(state); }>{t("刷新 SMB 配置", "Refresh SMB configuration")}</button>
            <Show when=move ||state.with(|s|matches!(s.smb_account.status,Some(SmbAccountStatus::Configured { password_change: PasswordChangeAvailability::Available,.. })))>
                <p class="zxs-row">{t("请输入当前 SMB 密码。每项为 1–255 个 UTF-8 字节且不能包含控制字符；新密码还须满足 Samba 策略。连接固定为服务器本机 Samba，账号需已存在。请使用 HTTPS 或受控可信网络。", "Enter your current SMB password. Each password must be 1–255 UTF-8 bytes without control characters, and the new password must meet Samba policy. Changes go only to local Samba for an existing account. Use HTTPS or a controlled trusted network.")}</p>
                <form class="zxs-account-form zxs-stack" aria-busy=move ||state.with(|s|s.smb_account.pending.to_string()) on:submit=move |event| {
                    event.prevent_default();
                    if state.with_untracked(|s|!s.session_active()||!s.smb_account.can_change()) { return; }
                    let current = current.get_untracked().expect("invariant: mounted form current password");
                    let password = new_password.get_untracked().expect("invariant: mounted form new password");
                    let confirmation = confirmation.get_untracked().expect("invariant: mounted form confirmation");
                    if current.value().is_empty()||password.value().is_empty() { input_failure.set(Some(t("请输入当前 SMB 密码和新密码。","Enter your current and new SMB password."))); return; }
                    if password.value()!=confirmation.value() { input_failure.set(Some(t("两次输入的新密码不一致。","The new passwords do not match."))); return; }
                    input_failure.set(None);
                    let old = current.value(); let new = password.value();
                    for input in [&current,&password,&confirmation] { input.set_value(""); }
                    change(state,old,new);
                }>
                    <div class="zxs-form-grid">
                        <label class="zxs-form-field"><span class="zxs-row">{t("当前 SMB 密码","Current SMB password")}</span><input class="zxs-input zxs-input--compact" type="password" name="smb_current_password" autocomplete="current-password" maxlength="255" required=true node_ref=current disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_change())/></label>
                        <label class="zxs-form-field"><span class="zxs-row">{t("新 SMB 密码","New SMB password")}</span><input class="zxs-input zxs-input--compact" type="password" name="smb_new_password" autocomplete="new-password" maxlength="255" required=true node_ref=new_password disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_change())/></label>
                        <label class="zxs-form-field"><span class="zxs-row">{t("确认新 SMB 密码","Confirm new SMB password")}</span><input class="zxs-input zxs-input--compact" type="password" name="smb_confirm_password" autocomplete="new-password" maxlength="255" required=true node_ref=confirmation disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_change())/></label>
                    </div>
                    {move ||input_failure.get().map(|message|view!{<ErrorState message=message/>})}
                    <div class="zxs-actions zxs-flow"><button class="zxs-button" type="submit" disabled=move ||state.with(|s|!s.session_active()||!s.smb_account.can_change())>{move ||state.with(|s|if s.smb_account.pending { t("保存中…","Saving…") }else {t("修改 SMB 密码","Change SMB password")})}</button></div>
                </form>
            </Show>
            {move ||state.with(|s|s.smb_account.failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
            <Show when=move ||state.with(|s|s.smb_account.binding_changed)><p class="zxs-row" role="status">{t("SMB 账号绑定已保存，旧 ZXS SMB 连接已撤销。请使用生效账号及其 Samba 密码重新连接。Web 会话保持有效。", "The SMB account binding was saved and old ZXS SMB connections were revoked. Reconnect with the effective account and its Samba password. Your Web session remains active.")}</p></Show>
            <Show when=move ||state.with(|s|s.smb_account.binding_unknown)><p class="zxs-row" role="alert">{t("账号保存结果未知。请先刷新 SMB 配置读取已持久化的生效账号，禁止盲目重复保存。", "The account save outcome is unknown. Refresh SMB configuration to read the durable effective account before another save.")}</p></Show>
            <Show when=move ||state.with(|s|s.smb_account.changed)><p class="zxs-row" role="status">{t("SMB 密码已修改。请在 SMB 客户端更新保存的凭据；现有 SMB 连接不保证立即失效。Web 登录密码保持不变。", "The SMB password was changed. Update saved credentials in your SMB clients; existing SMB connections may remain active. Your Web password is unchanged.")}</p></Show>
            <Show when=move ||state.with(|s|s.smb_account.unknown)>
                <p class="zxs-row" role="alert">{t("结果未知，已阻止重复提交。请先在 SMB 客户端核实当前可用密码，再解锁表单；刷新配置不会重试改密。", "The outcome is unknown and repeat submission is blocked. Verify which password works in your SMB client before unlocking the form. Refreshing configuration does not retry the change.")}</p>
                <button class="zxs-button" type="button" disabled=move ||state.with(|s|!s.session_active()||s.smb_account.pending) on:click=move |_|state.update(|s| {s.smb_account.unknown=false;s.smb_account.failure=None;})>{t("已核实密码，允许重新填写", "I verified the password; unlock the form")}</button>
            </Show>
        </section>
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/smb_account.rs"]
mod tests;
