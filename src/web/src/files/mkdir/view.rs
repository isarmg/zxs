use super::*;
use web_interface::i18n::t;

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(blocks_writes)>
            <aside class="zxs-surface zxs-stack" data-zxs-folder-create-recovery="">
                <h2>{t("文件路径与目录分支：原请求恢复", "File paths and folder branches: original-request recovery")}</h2>
                <p>{t(
                    "原账号、请求方法、目标、路径和操作编号已按站点保留；目录分支创建保留原来源，默认切换保留原分支和目录依据。导航或刷新只查询原结果，不自动重发。",
                    "The original account, request method, target, paths and operation ID are retained. Folder branch creation retains its reviewed source; default switching retains the original branch and namespace basis. Navigation and refresh only query; they never resend.",
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
                                        "重试保留同一编号、请求方法和完整原输入。尚未发布时会实际执行原建目录、文件移动、分支创建或默认切换；已发布时只返回原收据。查询缺失或重试被拒绝不证明原请求未发布。",
                                        "Retry retains the same ID, method and complete original input. It can perform the original folder creation, file move, branch creation or default switch if uncommitted; a published request returns its original receipt. A missing lookup or rejected retry does not prove that the first request never published.",
                                    )}</p>
                                    {requires_post.then(||view! {
                                        <p>{t(
                                            "这是空盘首次创建，原请求尚无仓库身份；请显式重试原请求完成核对，不用当前列表猜测创建结果。",
                                            "This was the first creation on an empty disk, before a repository identity existed. Explicitly retry the original request to verify it; the current listing is not proof of its outcome.",
                                        )}</p>
                                    })}
                                    <button class="zxs-button" data-zxs-folder-create-retry=""
                                        disabled=move ||state.with(|s|retry_record(s,epoch,operation).is_none())
                                        on:click=move |_|retry(state,epoch,operation)>{t("确认重试原请求，不生成新编号", "Confirm original-request retry, without a new ID")}</button>
                                </Show>
                                <Show when=move ||state.with(|s|s.files.mkdir.records.iter().any(|retained|retained.record.operation()==operation&&retained.phase==Phase::Settled))>
                                    <p>{t("原操作已有明确结果；仅本机记录清理未完成，不会再次提交。", "The original outcome is confirmed. Only local record cleanup remains; it never resubmits.")}</p>
                                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending) on:click=move |_| {
                                        if current_session(state,epoch) && !state.with_untracked(|s|s.files.mutation_pending) {
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
