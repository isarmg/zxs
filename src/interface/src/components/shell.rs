use super::{
    header::HeaderTargets,
    notification::{Toasts, push_toast},
    *,
};
use crate::{i18n::t, prelude::*};
use gloo_events::EventListener;
use wasm_bindgen::JsCast;

#[component]
pub fn AppShell(
    product: &'static str,
    session: Signal<SessionView>,
    pending_logout: Signal<bool>,
    on_logout: Callback<()>,
    on_account: Callback<()>,
    on_restore: Callback<()>,
    login: ChildrenFn,
    children: ChildrenFn,
) -> impl IntoView {
    let toasts = Toasts::new();
    provide_context(toasts);
    let targets = HeaderTargets {
        actions: NodeRef::new(),
        navigation: NodeRef::new(),
    };
    provide_context(targets);
    if let Some(window) = web_sys::window() {
        let listener = send_wrapper::SendWrapper::new(EventListener::new(
            &window,
            "zxs-notify",
            move |event| {
                if let Some(message) = event
                    .dyn_ref::<web_sys::CustomEvent>()
                    .and_then(|event| event.detail().as_string())
                {
                    push_toast(toasts, message);
                }
            },
        ));
        on_cleanup(move || drop(listener));
    }
    Effect::new(move |_| {
        if session.get().phase != SessionPhase::Authenticated {
            toasts.clear();
        }
    });
    IntoRender::into_render(move || {
        let current = session.get();
        if current.phase == SessionPhase::Loading {
            return view!{<div class="zxs-application-boot" role="status" aria-busy="true" aria-label=t("正在准备应用…","Preparing application…")></div>}.into_any();
        }
        if current.phase != SessionPhase::Authenticated {
            let login = login.clone();
            return view! {
                <div class="authentication-shell" data-zxs-scope="">
                    <div class="authentication-language">
                        <div class="zxs-login-controls" role="group" aria-label=t("显示设置","Display settings")><LanguageToggle/><ThemeToggle/></div>
                    </div>
                    <div class="authentication-content">
                        {(current.phase==SessionPhase::LogoutUnconfirmed).then(||view!{<div><ErrorState message=t("退出请求尚未确认，当前界面已锁定。请保留此页面并重试注销服务器会话；重新加载不代表已退出。","Sign out is unconfirmed and this view is locked. Keep this page open and retry server session revocation; reloading does not confirm sign out.")/><button class="zxs-button" disabled=move ||pending_logout.get() on:click=move |_|if !pending_logout.get_untracked(){on_logout.run(());}>{t("重试退出","Retry sign out")}</button></div>})}
                        <div class="authentication-card">
                            {if current.phase==SessionPhase::Error {
                                view!{<div><ErrorState message=current.message request_id=current.request_id/><button class="zxs-button" on:click=move |_|on_restore.run(())>{t("重试","Retry")}</button></div>}.into_any()
                            } else if current.phase==SessionPhase::LogoutUnconfirmed {
                                ().into_any()
                            } else {login().into_any()}}
                        </div>
                    </div>
                </div>
            }.into_any();
        }
        let children = children.clone();
        view! {
            <div class="zxs-app-shell" data-zxs-scope="" style=("--zxs-header-icon-size", "1em")>
                <a class="zxs-skip-link" href="#zxs-main-content" on:click=move |event| {
                    event.prevent_default();
                    if let Some(main) = web_sys::window()
                        .unwrap()
                        .document()
                        .unwrap()
                        .get_element_by_id("zxs-main-content")
                        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                        && let Err(error) = main.focus()
                    {
                        web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
                    }
                }>
                    {t("跳至正文", "Skip to content")}
                </a>
                <header class="zxs-page-header zxs-flow">
                    <div class="zxs-header-navigation-slot">
                        <div class="zxs-header-brand-navigation zxs-flow">
                            <div class="zxs-product-identity zxs-row"><strong>{product}</strong></div>
                            <div node_ref=targets.navigation style=("display", "contents")></div>
                        </div>
                    </div>
                    <div class="zxs-header-actions zxs-flow zxs-control-group--comfortable"
                        role="group" aria-label=t("全局操作", "Global actions")>
                        <div node_ref=targets.actions style=("display", "contents")></div>
                        <LanguageToggle/>
                        <ThemeToggle/>
                        <button class="zxs-button"
                            disabled=move || pending_logout.get()
                            aria-label=move || {
                                if pending_logout.get() {
                                    t("正在退出…", "Signing out…")
                                } else {
                                    t("退出", "Sign out")
                                }
                            }
                            title=t("退出", "Sign out")
                            on:click=move |_| {
                                if !pending_logout.get_untracked() { on_logout.run(()); }
                            }>
                            <svg aria-hidden="true" width="1em" height="1em" viewBox="0 0 24 24"
                                fill="none" stroke="currentColor" stroke-width="1.7">
                                <path d="M9 4H4v16h5m5-12 4 4-4 4M8 12h12"/>
                            </svg>
                        </button>
                        <button class="zxs-button zxs-account-entry" data-zxs-contour="none"
                            {..view! { <{..} style=("border-radius", "50%") style=("aspect-ratio", "1") /> }}
                            aria-label=t("账号设置", "Account settings")
                            title=t("账号设置", "Account settings")
                            on:click=move |_| {
                                on_account.run(());
                            }>
                            <svg viewBox="0 0 24 24" width="1em" height="1em" fill="none"
                                stroke="currentColor" stroke-width="1.7" aria-hidden="true">
                                <circle cx="12" cy="8" r="3.5"/>
                                <path d="M4.5 21v-2a7.5 7.5 0 0 1 15 0v2"/>
                            </svg>
                        </button>
                    </div>
                </header>
                <Show
                    when=move || !toasts.records.get().is_empty()
                    children=ToChildren::to_children(move || view! {
                        <div class="zxs-toast-stack" role="region" aria-label=t("通知", "Notifications")>
                            <For
                                each=move || toasts.records.get()
                                key=|r| r.id
                                children=move |r| {
                                    let id = r.id;
                                    view! {
                                        <div class="zxs-toast zxs-row zxs-flow" role="status"
                                            aria-live="polite" aria-atomic="true">
                                            <span>{r.message}</span>
                                            <button class="zxs-button"
                                                aria-label=t("关闭通知", "Dismiss notification")
                                                on:click=move |_| toasts.dismiss(id)>"×"</button>
                                        </div>
                                    }
                                }
                            />
                        </div>
                    }.into_inner())
                />
                <div class="zxs-shell-layout zxs-shell-layout--full">
                    <main class="zxs-shell-main zxs-stack" id="zxs-main-content" tabindex="-1">
                        {children()}
                    </main>
                </div>
            </div>
        }.into_any()
    })
}
