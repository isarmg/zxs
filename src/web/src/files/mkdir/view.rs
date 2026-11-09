use super::*;
use web_interface::i18n::t;

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(blocks_writes)>
            <aside class="zxs-surface zxs-stack" data-zxs-folder-create-recovery="">
                <h2>{t("新建文件夹：原操作恢复", "New folder: original-operation recovery")}</h2>
                <p>{t(
                    "原账号、磁盘、完整路径和操作编号已按站点保留。离开目录或刷新不会重新创建文件夹；只有核对原回执才能解除未确认状态。",
                    "The original account, disk, full path and operation ID are retained for this site. Navigation or refresh never creates the folder again. Only a matching original receipt confirms the outcome.",
                )}</p>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending)
                    on:click=move |_|restore(state)>{t("重新读取本机原请求", "Reload local original requests")}</button>
                <For each=move ||state.with(|s|s.files.mkdir.records.iter().map(|retained|(s.session_epoch,Arc::clone(&retained.record))).collect::<Vec<_>>())
                    key=|row|(row.0,row.1.operation()) children=move |(epoch,record)| {
                        let operation = record.operation();
                        let label = record.label();
                        let requires_post = !record.can_lookup();
                        view! {
                            <section class="zxs-stack">
                                <p>{label}</p>
                                <Show when=move ||state.with(|s|s.files.mkdir.records.iter().any(|retained|retained.record.operation()==operation&&retained.phase!=Phase::Settled))>
                                    <p>{t(
                                        "重试保留同一编号和原路径。若尚未创建，这会实际创建原文件夹；若已经发布，服务端只返回原收据。找不到操作或重试被拒绝都不能证明原请求未发布。",
                                        "Retry keeps the same ID and original path. It can create the original folder if still uncommitted; an already published request returns its original receipt. A missing lookup or rejected retry does not prove that the first request never published.",
                                    )}</p>
                                    {requires_post.then(||view! {
                                        <p>{t(
                                            "这是空盘首次创建，原请求尚无仓库身份；请显式重试原请求完成核对，不用当前列表猜测创建结果。",
                                            "This was the first creation on an empty disk, before a repository identity existed. Explicitly retry the original request to verify it; the current listing is not proof of its outcome.",
                                        )}</p>
                                    })}
                                    <button class="zxs-button" data-zxs-folder-create-retry=""
                                        disabled=move ||state.with(|s|retry_record(s,epoch,operation).is_none())
                                        on:click=move |_|retry(state,epoch,operation)>{t("重试原文件夹创建，不生成新编号", "Retry original folder creation, without a new ID")}</button>
                                </Show>
                                <Show when=move ||state.with(|s|s.files.mkdir.records.iter().any(|retained|retained.record.operation()==operation&&retained.phase==Phase::Settled))>
                                    <p>{t("原操作已有明确结果；仅本机记录清理未完成，不会再次提交。", "The original outcome is confirmed. Only local record cleanup remains; it never resubmits.")}</p>
                                    <button class="zxs-button" on:click=move |_| {
                                        if current_session(state,epoch) {
                                            finish(state,&operation.to_string());
                                        }
                                    }>{t("重试本机记录清理", "Retry local record cleanup")}</button>
                                </Show>
                            </section>
                        }
                    }/>
                {move ||state.with(|s|s.files.mkdir.error.clone()).map(|error|view! {
                    <p role="alert">{error.to_string()}</p>
                })}
            </aside>
        </Show>
    }
}
