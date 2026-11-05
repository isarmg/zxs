use super::ErrorState;
use crate::{i18n::t, prelude::*};

#[component]
pub fn LoginPage(
    pending: Signal<bool>,
    remote_failure: Signal<Option<super::Feedback>>,
    notice: Signal<String>,
    retry_available: Signal<bool>,
    on_retry: Callback<()>,
    on_submit: Callback<super::LoginCredentials>,
) -> impl IntoView {
    let username = NodeRef::<web_interface::html::Input>::new();
    let password = NodeRef::<web_interface::html::Input>::new();
    let failure = RwSignal::new(None::<(String, Option<String>)>);
    Effect::new(move |_| {
        if let Some(error) = remote_failure.get() {
            failure.set(Some((error.message, error.request_id)));
            if let Some(secret) = password.get_untracked() {
                secret.set_value("");
                if let Err(error) = secret.focus() {
                    web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
                }
            }
        }
    });
    on_cleanup(move || {
        if let Some(secret) = password.get_untracked() {
            secret.set_value("");
        }
    });
    view! {
        <form data-auth-form="login" aria-label=t("登录", "Sign in") novalidate
            aria-busy=move ||pending.get().to_string()
            on:input=move |_|failure.set(None)
            on:submit=move |event| {
                event.prevent_default();
                if pending.get_untracked() { return; }
                let name=username.get_untracked().expect("invariant: mounted login username");
                let secret=password.get_untracked().expect("invariant: mounted login password");
                if name.value().is_empty() {
                    failure.set(Some((t("请输入账户名。", "Enter your account name."),None)));
                    if let Err(error)=name.focus() {web_sys::console::warn_2(&"Could not move keyboard focus".into(),&error);}
                    return;
                }
                if secret.value().is_empty() {
                    failure.set(Some((t("请输入密码。", "Enter your password."),None)));
                    if let Err(error)=secret.focus() {web_sys::console::warn_2(&"Could not move keyboard focus".into(),&error);}
                    return;
                }
                failure.set(None);
                let credentials=super::LoginCredentials{username:name.value(),password:secret.value()};
                secret.set_value("");
                on_submit.run(credentials);
            }>
            <label class="zxs-auth-label" for="zxs-login-username">{t("账户名", "Account name")}</label>
            <div class="zxs-auth-input-row zxs-hover-row">
                <input id="zxs-login-username" class="zxs-input" node_ref=username name="username" autocomplete="username" required maxlength="64" readonly=move ||pending.get()/>
            </div>
            <label class="zxs-auth-label" for="zxs-login-password">{t("密码", "Password")}</label>
            <div class="zxs-auth-input-row zxs-hover-row">
                <input id="zxs-login-password" class="zxs-input" node_ref=password name="password" type="password" autocomplete="current-password" required maxlength="1024" readonly=move ||pending.get()/>
            </div>
            <div class="zxs-auth-information">
                {move ||failure.get().map(|(message,id)|view!{<ErrorState message=message request_id=id/>})}
                <Show when=move ||failure.get().is_none()><span role="status">{move ||notice.get()}</span></Show>
            </div>
            <div class="zxs-auth-actions">
                <button type="submit" class="zxs-button" disabled=move ||pending.get()>{move ||if pending.get(){t("正在登录…", "Signing in…")}else{t("登录", "Sign in")}}</button>
                <Show when=move ||retry_available.get()><button type="button" class="zxs-button" data-setup-check="" aria-label=t("检查设置状态", "Check setup status") title=t("检查设置状态", "Check setup status") disabled=move ||pending.get() on:click=move |_|on_retry.run(())><svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 11-2l3 3M4 16l3 3a7 7 0 0 0 11-2"/></svg></button></Show>
            </div>
        </form>
    }
}
