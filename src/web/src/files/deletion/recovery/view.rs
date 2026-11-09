use super::*;
use web_interface::i18n::t;

fn description(record: &Record) -> String {
    match &record.intent {
        request::Intent::Delete(target) => format!(
            "{} · {} · {} · {}",
            t("软删除整个文件", "Soft-delete whole file"),
            target.path.as_str(),
            target.file,
            record.operation
        ),
        request::Intent::Undo(node) => format!(
            "{} · {} · {} · {} · {}",
            t("撤销准确删除节点", "Undo exact deletion node"),
            node.path.as_str(),
            node.file_id,
            node.node_id,
            record.operation
        ),
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    // Login/session admission hydrates before admitting more browser events.
    // Rendering, navigation and opening this panel never perform a mutation.
    view! {
        <Show when=move ||state.with(blocks_writes)>
            <aside class="zxs-surface zxs-stack" data-zxs-file-deletion-recovery="">
                <h2>{t("软删除与撤销：原操作恢复", "Soft deletion and restore: original-operation recovery")}</h2>
                <p>{t("本浏览器按站点和账号保存原请求与准确删除节点，不保存密码、登录令牌或完成结论。刷新或重新登录只查询原收据，不自动重新删除或恢复；关闭文件面板不丢弃记录。", "This browser retains original requests and exact deletion nodes per site and account, never passwords, login tokens or completion claims. Refreshing or signing in only queries original receipts; it never automatically deletes or restores. Closing file panels keeps these records.")}</p>
                <div class="zxs-row">
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending||!s.files.deletion.recovery.owns(s)||s.files.deletion.recovery.records.iter().all(|r|r.settled.is_some())) on:click=move |_|{
                        if state.with_untracked(|s|s.files.deletion.recovery.owns(s)&&!s.files.mutation_pending&&s.files.deletion.recovery.records.iter().any(|r|r.settled.is_none())) {
                            actions::poll(state);
                        }
                    }>{t("只查询原操作结果", "Check original outcomes only")}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending) on:click=move |_|restore(state)>{t("重新读取本机原请求", "Reload local original requests")}</button>
                </div>
                <p>{t("查询不会再次发送删除或撤销。查询失败、暂未找到或删除列表变化都不能解除原操作围栏；只有核对原账号、摘要和完整结果的回执才能确认。", "Checking never resends deletion or restoration. A failed or missing lookup, or a changed deletion list, cannot settle the original operation. Confirmation requires the receipt matching its account, digest and complete result.")}</p>
                <For each=move ||state.with(|s|s.files.deletion.recovery.records.iter().map(|r|(s.session_epoch,r.record.clone())).collect::<Vec<_>>()) key=|row|(row.0,row.1.operation) children=move |(epoch,record)| {
                    let operation = record.operation;
                    let id = operation.to_string();
                    let retry_id = id.clone();
                    let cleanup_id = id.clone();
                    view! {
                        <section class="zxs-stack">
                            <p><code>{description(&record)}</code></p>
                            <Show when=move ||state.with(|s|s.files.deletion.recovery.records.iter().any(|r|r.record.operation==operation&&r.settled.is_none()))>
                                <p>{t("结果尚未确认。查询返回找不到或重试遭到拒绝，都不证明原操作未发布。", "The outcome is unconfirmed. A missing lookup or rejected retry does not prove the original operation never published.")}</p>
                                {
                                    let retry_id = retry_id.clone();
                                    view! {<button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending||s.files.polling_operations.contains(&retry_id)) on:click=move |_|begin_retry(state,epoch,operation)>{t("核对并续发原请求…", "Review original-request retry…")}</button>}
                                }
                            </Show>
                            <Show when=move ||state.with(|s|s.files.deletion.recovery.records.iter().any(|r|r.record.operation==operation&&r.settled.is_some()))>
                                <p>{t("原操作已获明确结果；只剩本机恢复记录清理，清理不会再次提交。", "The original operation has a confirmed outcome. Only local recovery cleanup remains; cleanup never resubmits.")}</p>
                                {
                                    let cleanup_id = cleanup_id.clone();
                                    view! {<button class="zxs-button" on:click=move |_| { if current_session(state,epoch) { finish(state,&cleanup_id); } }>{t("重试本机记录清理", "Retry local record cleanup")}</button>}
                                }
                            </Show>
                        </section>
                    }
                }/>
                {move ||state.with(|s|s.files.deletion.recovery.review.clone()).map(|review| {
                    let epoch = review.epoch;
                    let generation = review.generation;
                    view! {
                        <section class="zxs-stack" data-zxs-file-deletion-retry-review="">
                            <h3>{t("确认续发同一原请求", "Confirm the same original request")}</h3>
                            <p><code>{description(&review.record)}</code></p>
                            <p>{t("仅使用原账号、文件、路径或准确删除节点以及原 operation ID。若原请求尚未发布，这会实际执行原删除或撤销；若已发布，只取回原收据。当前权限与状态仍由服务端决定。本机材料不是授权。", "Only the original account, file, path or exact deletion node and original operation ID are used. If it never published, this can perform the original deletion or restoration; otherwise it retrieves the original receipt. The server checks current permission and state. Local material grants no authority.")}</p>
                            <button class="zxs-button" data-zxs-confirm-deletion-retry="" disabled=move ||state.with(|s|reviewed(s,epoch,generation).is_none()) on:click=move |_|retry(state,epoch,generation)>{t("确认续发原操作，不生成新 ID", "Retry original operation, without a new ID")}</button>
                            <button class="zxs-button" on:click=move |_|state.update(|s| {
                                if s.session_epoch==epoch && s.files.deletion.recovery.review.as_ref().is_some_and(|r|r.generation==generation) {
                                    s.files.deletion.recovery.cancel_review();
                                }
                            })>{t("关闭，不续发", "Close without retrying")}</button>
                        </section>
                    }
                })}
                {move ||state.with(|s|s.files.deletion.recovery.error.clone()).map(|error|view!{<p role="alert">{error.to_string()}</p>})}
            </aside>
        </Show>
    }
}
