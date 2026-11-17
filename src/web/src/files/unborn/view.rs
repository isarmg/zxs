use super::{
    Phase, Target, can_review, can_submit, load, submit,
    recovery::{cleanup, lookup, restore, retry, retry_record},
};
use crate::ui::State;
use storage_contract::file_draft::CommitMode;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s| Target::capture(s).is_some())>
            <section class="zxs-entry-section zxs-stack" data-zxs-unborn-working="">
                <h3 class="zxs-row">{t("工作内容，尚无历史节点", "Working content, no history yet")}</h3>
                <p class="zxs-row">{t(
                    "普通保存更新工作内容，不生成历史版本。第一次显式提交才建立文件历史。",
                    concat!(
                        "Ordinary saves update working content, not historical versions. ",
                        "The first explicit commit starts file history.",
                    ),
                )}</p>
                <button class="zxs-button" data-zxs-review-first-commit=""
                    disabled=move ||!state.with(can_review)
                    on:click=move |_|load(state)>
                    {t("核对当前工作内容", "Review current working content")}
                </button>
                <Show when=move ||state.with(|s| s.files.unborn.reading.is_some())>
                    <p role="status">{t("正在读取…", "Reading…")}</p>
                </Show>
                <Show when=move ||state.with(|s| {
                    s.files.unborn.review.as_ref().is_some_and(|(target, _)| target.current(s))
                })>
                    <p class="zxs-row" role="status">{move ||state.with(|s| {
                        s.files.unborn.review.as_ref().map(|(_, snapshot)| format!(
                            "{} · {}", match snapshot.mode {
                                CommitMode::Manual => t("手动", "Manual"),
                                CommitMode::Automatic => t("自动", "Automatic"),
                            }, crate::format::bytes(snapshot.logical_size),
                        )).unwrap_or_default()
                    })}</p>
                    <label class="zxs-stack">
                        {t("首个提交备注", "First commit message")}
                        <input type="text" maxlength="128"
                            prop:value=move ||state.with(|s| s.files.unborn.message.clone())
                            disabled=move ||state.with(|s| {
                                s.files.unborn.pending.is_some()
                            })
                            on:input=move |event|state.update(|s| {
                                if s.files.unborn.pending.is_none() {
                                    s.files.unborn.message = event_target_value(&event);
                                }
                            })/>
                    </label>
                    <button class="zxs-button" data-zxs-first-commit=""
                        disabled=move ||!state.with(can_submit)
                        on:click=move |_|submit(state)>
                        {t("显式提交首个历史节点", "Explicitly commit the first history node")}
                    </button>
                </Show>
                {move ||state.with(|s| {
                    if s.files.unborn.pending.is_none() { s.files.unborn.error.clone() }
                    else { None }
                }).map(|error|view! { <p role="alert">{error.to_string()}</p> })}
            </section>
        </Show>
    }
}

pub(crate) fn recovery_view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(super::blocks_writes)>
            <aside class="zxs-surface zxs-stack" data-zxs-first-commit-recovery="">
                <h2>{t("首个提交：原请求恢复", "First commit: original-request recovery")}</h2>
                <p>{t(
                    "保留原账号、文件、路径、工作代数和操作编号。刷新不自动提交；重试只发送原请求，不重新选择最新内容。",
                    concat!(
                        "The original account, file, path, working generation and operation ID ",
                        "are retained. Refresh never submits automatically; retry sends that ",
                        "request without selecting newer content.",
                    ),
                )}</p>
                <p class="zxs-row">{move ||state.with(|s| {
                    s.files.unborn.pending.as_ref().map(|p| format!(
                        "{} · {}",
                        p.original.request.command.target.path.as_str(), p.original.request.command.operation_id,
                    )).unwrap_or_default()
                })}</p>
                <button class="zxs-button" data-zxs-lookup-first-commit=""
                    disabled=move ||!state.with(|s| s.files.unborn.pending.as_ref()
                        .is_some_and(|p| p.phase == Phase::Pending))
                    on:click=move |_|lookup(state)>
                    {t("只查询原提交结果", "Look up the original commit without retrying")}
                </button>
                <Show when=move ||state.with(|s| s.files.unborn.pending.as_ref()
                    .is_some_and(|p| p.phase == Phase::LookingUp))>
                    <p role="status">{t("正在查询原回执；未发送提交。", "Looking up the receipt; no commit was sent.")}</p>
                </Show>
                <button class="zxs-button" data-zxs-retry-first-commit=""
                    disabled=move ||state.with(|s| retry_record(s).is_none())
                    on:click=move |_|retry(state)>
                    {t("确认重试原提交", "Confirm retry of the original commit")}
                </button>
                <Show when=move ||state.with(|s| {
                    s.files.unborn.pending.as_ref().is_some_and(|p| matches!(p.phase, Phase::Settled | Phase::Removing))
                })>
                    <p role="status">{t(
                        "原结果已确认，仅本机记录清理待完成。",
                        "The original result is confirmed; only local record cleanup remains.",
                    )}</p>
                    <button class="zxs-button"
                        disabled=move ||state.with(|s| s.files.unborn.changing_storage())
                        on:click=move |_|cleanup(state)>
                        {t("重试本机记录清理", "Retry local record cleanup")}
                    </button>
                </Show>
                <button class="zxs-button"
                    disabled=move ||state.with(|s| {
                        s.files.mutation_pending || s.files.unborn.pending.as_ref().is_some_and(|p| {
                            !matches!(p.phase, Phase::Pending | Phase::Settled)
                        })
                    })
                    on:click=move |_|restore(state)>
                    {t("重新读取本机原请求", "Reload local original request")}
                </button>
                <Show when=move ||state.with(|s| s.files.unborn.changing_storage())>
                    <p role="status">{t(
                        "正在等待或核对本机原请求，请保留本页。",
                        "Waiting for or verifying the local original request. Keep this page open.",
                    )}</p>
                </Show>
                {move ||state.with(|s| s.files.unborn.error.clone())
                    .map(|error|view! { <p role="alert">{error.to_string()}</p> })}
            </aside>
        </Show>
    }
}
