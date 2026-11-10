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
        request::Intent::DeleteBatch(request) => format!(
            "{} · {} · {}",
            t("整批软删除文件", "Atomic file soft deletion"),
            request.label(),
            record.operation,
        ),
        request::Intent::DeleteDirectory(request) => format!(
            "{} · {} · {}",
            t("软删除整个普通目录", "Soft-delete whole ordinary folder"),
            request.label(),
            record.operation,
        ),
        request::Intent::Undo(node) => format!(
            "{} · {} · {} · {} · {}",
            t("撤销准确删除节点", "Undo exact deletion node"),
            node.path.as_str(),
            node.file_id,
            node.node_id,
            record.operation
        ),
        request::Intent::UndoDirectory(node) => format!(
            "{} · {} · {} · {} · {}",
            t("恢复准确目录删除节点", "Restore exact folder deletion"),
            node.path.as_str(),
            node.directory_id,
            node.node_id,
            record.operation,
        ),
    }
}
fn batch_details(record: &Record) -> impl IntoView + use<> {
    match &record.intent {
        request::Intent::DeleteBatch(request) => Some(batch::retained_view(request)),
        request::Intent::Delete(_)
        | request::Intent::DeleteDirectory(_)
        | request::Intent::Undo(_)
        | request::Intent::UndoDirectory(_) => None,
    }
}

fn directory_details(record: &Record) -> impl IntoView + use<> {
    let details = match &record.intent {
        request::Intent::DeleteDirectory(request) => Some((
            format!(
                "{} · {} · {} · {} · {}",
                request.slot(), request.repository(), request.owner(), request.branch(), request.tree(),
            ),
            vec![format!("{} · {}", request.path().as_str(), request.directory())],
        )),
        request::Intent::UndoDirectory(node) => Some((
            format!(
                "{} · {} · {} · {} · {} · {}",
                node.repository_id, node.owner_directory_id, node.directory_branch_id,
                node.source_tree_id, node.namespace_head, node.operation_id,
            ),
            node.files.iter().map(|(file, member)| format!(
                "{} · {} · {}", file, member.deletion_node_id,
                if member.created_by_directory {
                    t("随目录恢复", "Restored with the folder")
                } else {
                    t("此前已删除，继续保留删除状态", "Previously deleted, remains deleted")
                },
            )).collect(),
        )),
        request::Intent::Delete(_) | request::Intent::DeleteBatch(_) | request::Intent::Undo(_) => None,
    };
    details.map(|(scope, entries)| view! {
        <details class="zxs-stack">
            <summary>{t("核对原目录身份、所有者与恢复范围", "Inspect original folder identity, owner and restoration scope")}</summary>
            <p><code>{scope}</code></p>
            <ul>{entries.into_iter().map(|label| view! {<li><code>{label}</code></li>}).collect_view()}</ul>
        </details>
    })
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
                            {batch_details(&record)}
                            {directory_details(&record)}
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
                            {batch_details(&review.record)}
                            {directory_details(&review.record)}
                            <p>{t("仅使用原账号、文件或目录身份、路径、准确删除节点以及原 operation ID；批次与目录保留原所有者、分支和依据树，不重读当前选择替换任何项。恢复目录只移除该次真正创建的文件 D，此前已删除的文件保持删除。若原请求尚未发布，这会实际执行原删除或撤销；若已发布，只取回原收据。当前权限与状态仍由服务端决定，本机材料不是授权。", "Only the original account, file or folder identity, path, exact deletion node and operation ID are used. Batches and folders retain their original owner, branch and baseline; today's selection never replaces any item. Folder restoration removes only the file D nodes created by that directory operation; previously deleted files remain deleted. If it never published, this can perform the original deletion or restoration; otherwise it retrieves the original receipt. The server checks current permission and state; local material grants no authority.")}</p>
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

