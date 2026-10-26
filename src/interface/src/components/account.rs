use super::ErrorState;
use crate::component::{component_props_builder, component_view};
use crate::{i18n::t, prelude::*};
#[component]
pub fn AccountPage(
    username_value: Signal<String>,
    pending: Signal<bool>,
    remote_failure: Signal<Option<super::Feedback>>,
    on_submit: Callback<super::PasswordChange>,
) -> impl IntoView {
    let username = NodeRef::<web_interface::html::Input>::new();
    let current = NodeRef::<web_interface::html::Input>::new();
    let new_password = NodeRef::<web_interface::html::Input>::new();
    let confirmation = NodeRef::<web_interface::html::Input>::new();
    let failure = RwSignal::new(None::<String>);
    let initial = username_value.get_untracked();
    Effect::new(move |_| {
        if let Some(error) = remote_failure.get() {
            failure.set(Some(error.display()));
        }
    });
    on_cleanup(move || {
        for input in [current, new_password, confirmation] {
            if let Some(input) = input.get_untracked() {
                input.set_value("");
            }
        }
    });
    View::new(
    web_interface::html::section()
        .child(
            #[allow(unused_braces)]
            {
                web_interface::html::div()
                    .child(
                        #[allow(unused_braces)]
                        {
                            web_interface::html::form()
                                .child((
                                    web_interface::html::div()
                                        .child((
                                            web_interface::html::label()
                                                .child((
                                                    web_interface::html::span().class("zxs-row zxs-row--center")
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("用户名", "Username")
                                                                })
                                                            },
                                                        ),
                                                    web_interface::html::input()
                                                        .class("zxs-input zxs-input--compact")
                                                        .node_ref(username)
                                                        .name("username")
                                                        .value(
                                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                                                                initial,
                                                            ),
                                                        )
                                                        .autocomplete("username")
                                                        .required(true)
                                                        .minlength("3")
                                                        .maxlength("64")
                                                        .readonly(
                                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(true),
                                                        ),
                                                ))
                                                .class("zxs-form-field"),
                                            web_interface::html::label()
                                                .child((
                                                    web_interface::html::span().class("zxs-row zxs-row--center")
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("当前密码", "Current password")
                                                                })
                                                            },
                                                        ),
                                                    web_interface::html::input()
                                                        .class("zxs-input zxs-input--compact")
                                                        .node_ref(current)
                                                        .name("current_password")
                                                        .r#type("password")
                                                        .autocomplete("current-password")
                                                        .required(true)
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("zxs-form-field"),
                                            web_interface::html::label()
                                                .child((
                                                    web_interface::html::span().class("zxs-row zxs-row--center")
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("新密码", "New password")
                                                                })
                                                            },
                                                        ),
                                                    web_interface::html::input()
                                                        .class("zxs-input zxs-input--compact")
                                                        .node_ref(new_password)
                                                        .name("new_password")
                                                        .r#type("password")
                                                        .autocomplete("new-password")
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("zxs-form-field"),
                                            web_interface::html::label()
                                                .child((
                                                    web_interface::html::span().class("zxs-row zxs-row--center")
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("确认新密码", "Confirm new password")
                                                                })
                                                            },
                                                        ),
                                                    web_interface::html::input()
                                                        .class("zxs-input zxs-input--compact")
                                                        .node_ref(confirmation)
                                                        .name("confirm_password")
                                                        .r#type("password")
                                                        .autocomplete("new-password")
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("zxs-form-field"),
                                        ))
                                        .class("zxs-account-fields"),
                                    IntoRender::into_render({
                                        move || {
                                            failure
                                                .get()
                                                .map(|message| {
                                                    View::new({
    #[allow(unreachable_code)] #[allow(unused_mut)] #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
    component_view(
        #[allow(clippy::needless_borrows_for_generic_args)]
        &ErrorState,
        {
            let mut props = component_props_builder(&ErrorState)
                .message(#[allow(unused_braces)] { message })
                .build();
            props
        },
    )
})
                                                })
                                        }
                                    }),
                                    web_interface::html::div()
                                        .child(
                                            #[allow(unused_braces)]
                                            {
                                                web_interface::html::button()
                                                    .child(
                                                        #[allow(unused_braces)]
                                                        {
                                                            IntoRender::into_render({
                                                                move || {
                                                                    if pending.get() {
                                                                        t("保存中…", "Saving…")
                                                                    } else {
                                                                        t("保存", "Save")
                                                                    }
                                                                }
                                                            })
                                                        },
                                                    )
                                                    .class("zxs-button")
                                                    .r#type("submit")
                                                    .disabled(
                                                        ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                        pending.get()),
                                                    )
                                            },
                                        )
                                        .class("zxs-actions zxs-flow"),
                                ))
                                .class("zxs-account-form zxs-stack")
                                .novalidate(true)
                                .aria_busy(
                                    ::web_interface::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    pending.get().to_string()),
                                )
                                .on(
                                    web_interface::ev::input,
                                    move |_| failure.set(None),
                                )
                                .on(
                                    web_interface::ev::submit,
                                    move |event| {
                                        event.prevent_default();
                                        if pending.get_untracked() {
                                            return;
                                        }
                                        let secret = current.get_untracked().unwrap();
                                        let password = new_password.get_untracked().unwrap();
                                        let confirm = confirmation.get_untracked().unwrap();
                                        if secret.value().is_empty() || password.value().is_empty() {
                                            failure.set(Some(t("请输入当前密码和新密码。", "Enter your current and new password.")));
                                            if let Err(error) = secret.focus() { web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error); } return;
                                        }
                                        if password.value() != confirm.value() {
                                            failure.set(Some(t("两次输入的新密码不一致。", "The new passwords do not match.")));
                                            if let Err(error) = confirm.focus() { web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error); } return;
                                        }
                                        let change = super::PasswordChange { current_password: secret.value(), new_password: password.value() };
                                        failure.set(None);
                                        for input in [&secret, &password, &confirm] { input.set_value(""); }
                                        on_submit.run(change);
                                    },
                                )
                        },
                    )
                    .class("zxs-content-panel zxs-frame zxs-stack")
            },
        )
        .class("zxs-content-stack zxs-account-page zxs-stack")
        .aria_label(
            ::web_interface::prelude::IntoAttributeValue::into_attribute_value(
                t("账号设置", "Account settings"),
            ),
        ),
)
}
