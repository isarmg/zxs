use super::{
    header::HeaderTargets,
    notification::{Toasts, push_toast},
    *,
};
use crate::{
    component::{component_props_builder, component_view},
    i18n::t,
    prelude::*,
};
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
                        {(!current.message.is_empty() && current.phase==SessionPhase::Anonymous).then(||view!{<p role="status">{current.message.clone()}</p>})}
                        {(current.phase==SessionPhase::LogoutUnconfirmed).then(||view!{<div><ErrorState message=t("退出请求尚未确认，当前界面已锁定。请保留此页面并重试注销服务器会话；重新加载不代表已退出。","Sign out is unconfirmed and this view is locked. Keep this page open and retry server session revocation; reloading does not confirm sign out.")/><button class="zxs-button" disabled=move ||pending_logout.get() on:click=move |_|if !pending_logout.get_untracked(){on_logout.run(());}>{t("重试退出","Retry sign out")}</button></div>})}
                        <div class="authentication-card">
                            <div class="zxs-product-identity"><strong>{product}</strong></div>
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
        View::new(
    web_interface::html::div()
        .child((
            web_interface::html::a()
                .child(
                    #[allow(unused_braces)]
                    {
                        IntoRender::into_render({
                            t("跳至正文", "Skip to content")
                        })
                    },
                )
                .class("zxs-skip-link")
                .href("#zxs-main-content")
                .on(
                    web_interface::ev::click,
                    move |event| {
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
                    },
                ),
            web_interface::html::header()
                .child((
                    web_interface::html::div()
                        .child(
                            #[allow(unused_braces)]
                            {
                                web_interface::html::div()
                                    .child((
                                        web_interface::html::div()
                                            .child(
                                                #[allow(unused_braces)]
                                                {
                                                    web_interface::html::strong()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            { IntoRender::into_render({ product }) },
                                                        )
                                                },
                                            )
                                            .class("zxs-product-identity zxs-row"),
                                        web_interface::html::div()
                                            .node_ref(targets.navigation)
                                            .style(("display", "contents")),
                                    ))
                                    .class("zxs-header-brand-navigation zxs-flow")
                            },
                        )
                        .class("zxs-header-navigation-slot"),
                    web_interface::html::div()
                        .child((
                            web_interface::html::div()
                                .node_ref(targets.actions)
                                .style(("display", "contents")),
                            {
                                #[allow(unreachable_code)] #[allow(unused_mut)]
                                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                component_view(
                                    #[allow(clippy::needless_borrows_for_generic_args)]
                                    &LanguageToggle,
                                    {
                                        let mut props = component_props_builder(
                                                &LanguageToggle,
                                            )
                                            .build();
                                        props
                                    },
                                )
                            },
                            {
                                #[allow(unreachable_code)] #[allow(unused_mut)]
                                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                component_view(
                                    #[allow(clippy::needless_borrows_for_generic_args)]
                                    &ThemeToggle,
                                    {
                                        let mut props = component_props_builder(
                                                &ThemeToggle,
                                            )
                                            .build();
                                        props
                                    },
                                )
                            },
                            web_interface::html::button()
                                .child(
                                    #[allow(unused_braces)]
                                    {
                                        web_interface::svg::svg()
                                            .child(
                                                #[allow(unused_braces)]
                                                {
                                                    web_interface::svg::path()
                                                        .attr("d", "M9 4H4v16h5m5-12 4 4-4 4M8 12h12")
                                                },
                                            )
                                            .attr("aria-hidden", "true")
                                            .attr("width", "1em")
                                            .attr("height", "1em")
                                            .attr("viewBox", "0 0 24 24")
                                            .attr("fill", "none")
                                            .attr("stroke", "currentColor")
                                            .attr("stroke-width", "1.7")
                                    },
                                )
                                .class("zxs-button")
                                .disabled(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    pending_logout.get()),
                                )
                                .aria_label(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    {
                                        if pending_logout.get() {
                                            t("正在退出…", "Signing out…")
                                        } else {
                                            t("退出", "Sign out")
                                        }
                                    }),
                                )
                                .title(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                        t("退出", "Sign out"),
                                    ),
                                )
                                .on(
                                    web_interface::ev::click,
                                    move |_| {
                                        if !pending_logout.get_untracked() { on_logout.run(()); }
                                    },
                                ),
                            web_interface::html::button()
                                .child(
                                    #[allow(unused_braces)]
                                    {
                                        web_interface::svg::svg()
                                            .child((
                                                web_interface::svg::circle()
                                                    .attr("cx", "12")
                                                    .attr("cy", "8")
                                                    .attr("r", "3.5"),
                                                web_interface::svg::path()
                                                    .attr("d", "M4.5 21v-2a7.5 7.5 0 0 1 15 0v2"),
                                            ))
                                            .attr("viewBox", "0 0 24 24")
                                            .attr("width", "1em")
                                            .attr("height", "1em")
                                            .attr("fill", "none")
                                            .attr("stroke", "currentColor")
                                            .attr("stroke-width", "1.7")
                                            .attr("aria-hidden", "true")
                                    },
                                )
                                .class("zxs-button zxs-account-entry")
                                .attr("data-zxs-contour", "none")
                                .style(("border-radius", "50%"))
                                .style(("aspect-ratio", "1"))
                                .aria_label(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                        t("账号设置", "Account settings"),
                                    ),
                                )
                                .title(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                        t("账号设置", "Account settings"),
                                    ),
                                )
                                .on(
                                    web_interface::ev::click,
                                    move |_| {
                                        on_account.run(());
                                    },
                                ),
                        ))
                        .class("zxs-header-actions zxs-flow zxs-control-group--comfortable")
                        .role("group")
                        .aria_label(
                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                t("全局操作", "Global actions"),
                            ),
                        ),
                ))
                .class("zxs-page-header zxs-flow"),
            {
                #[allow(unreachable_code)] #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &Show,
                    {
                        let mut props = component_props_builder(
                                &Show,
                            )
                            .when(
                                #[allow(unused_braces)]
                                { move || !toasts.records.get().is_empty() },
                            )
                            .children({
                                ::web_interface::children::ToChildren::to_children(move || {
                                    web_interface::html::div()
                                        .child(
                                            #[allow(unused_braces)]
                                            {
                                                {
                                                    #[allow(unreachable_code)] #[allow(unused_mut)]
                                                    #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                                    component_view(
                                                        #[allow(clippy::needless_borrows_for_generic_args)]
                                                        &For,
                                                        {
                                                            let mut props = component_props_builder(
                                                                    &For,
                                                                )
                                                                .each(
                                                                    #[allow(unused_braces)]
                                                                    { move || toasts.records.get() },
                                                                )
                                                                .key(#[allow(unused_braces)] { |r| r.id })
                                                                .children(
                                                                    #[allow(unused_braces)]
                                                                    {
                                                                        move |r| {
                                                                            let id = r.id;
                                                                            View::new(
    web_interface::html::div()
        .child((
            web_interface::html::span()
                .child(
                    #[allow(unused_braces)]
                    { IntoRender::into_render({ r.message }) },
                ),
            web_interface::html::button()
                .child(#[allow(unused_braces)] { "×" })
                .class("zxs-button")
                .aria_label(
                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                        t("关闭通知", "Dismiss notification"),
                    ),
                )
                .on(
                    web_interface::ev::click,
                    move |_| toasts.dismiss(id),
                ),
        ))
        .class("zxs-toast zxs-row zxs-flow")
        .role("status")
        .aria_live("polite")
        .aria_atomic("true"),
)
                                                                        }
                                                                    },
                                                                )
                                                                .build();
                                                            props
                                                        },
                                                    )
                                                }
                                            },
                                        )
                                        .class("zxs-toast-stack")
                                        .role("region")
                                        .aria_label(
                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                                t("通知", "Notifications"),
                                            ),
                                        )
                                })
                            })
                            .build();
                        props
                    },
                )
            },
            web_interface::html::div()
                .child(
                    #[allow(unused_braces)]
                    {
                        web_interface::html::main()
                            .child(
                                #[allow(unused_braces)]
                                {
                                    IntoRender::into_render(children())
                                },
                            )
                            .class("zxs-shell-main zxs-stack")
                            .id("zxs-main-content")
                            .tabindex("-1")
                    },
                )
                .class("zxs-shell-layout zxs-shell-layout--full"),
        ))
        .class("zxs-app-shell")
        .attr("data-zxs-scope", "")
        .style(("--zxs-header-icon-size", "1em")),
)
            .into_any()
    })
}
