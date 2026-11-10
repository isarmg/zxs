use super::{can_retry_tags, has_tag_replay, retry_tags};
use crate::ui::State;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(has_tag_replay)>
            <section class="zxs-frame zxs-stack" data-tag-request-recovery="" aria-label=t("标签请求恢复","Tag request recovery")>
                <p class="zxs-row" role="status">{t(
                    "标签保存结果尚未确认，系统会继续查询原请求。重试会保留原来的文件、节点和标签。",
                    "The tag save has not been confirmed. The original request is still being checked. Retrying keeps the same file, nodes and tags.",
                )}</p>
                <div class="zxs-row">
                    <button class="zxs-button" data-tag-request-retry=""
                        disabled=move ||!state.with(can_retry_tags)
                        on:click=move |_|retry_tags(state)>{t("重试原标签请求","Retry original tag request")}</button>
                </div>
            </section>
        </Show>
    }
}
