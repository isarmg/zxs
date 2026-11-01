use crate::{i18n::t, prelude::*};
#[component]
pub fn LoadingState() -> impl IntoView {
    view! {
        <div class="zxs-loading zxs-row" role="status" aria-live="polite">
            {t("正在加载…", "Loading…")}
        </div>
    }
}
#[component]
pub fn EmptyState(message: String) -> impl IntoView {
    view! { <div class="zxs-empty zxs-row">{message}</div> }
}
#[component]
pub fn ErrorState(
    message: String,
    #[prop(optional_no_strip)] request_id: Option<String>,
) -> impl IntoView {
    view! {
        <div class="zxs-error zxs-row" role="alert">
            {message}
            {request_id.filter(|value| super::safe_request_id(value)).map(|id| view! {
                <p class="zxs-request-id">
                    {t("请求标识：", "Request ID: ")}<code>{id}</code>
                </p>
            })}
        </div>
    }
}
