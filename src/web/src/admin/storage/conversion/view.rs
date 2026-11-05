use super::*;

pub(in super::super) fn form(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|source(&s.admin_console.storage).is_some())>
            <p class="zxs-row">{t("普通目录只能转换为新的第 1 层主盘。为保护原内容，需要另一个已核验的空目标目录；源目录不删除、不改写。损坏或外来 ZXS 格式会被检查拒绝，不会自动转换。", "An ordinary directory can become only a new tier-1 primary. A separate verified empty target is required to preserve the source. Source files are never removed or rewritten. Inspection rejects damaged or foreign ZXS formats instead of converting them automatically.")}</p>
            <Show when=move ||state.with(|s|!active(&s.admin_console.storage))>
                <p class="zxs-row" role="status">{t("当前转换需要已有活动存储池。请先通过空目录向导建立或恢复存储；本步骤不能直接创建第一个存储池。", "Conversion currently requires an active pool. First establish or restore storage through the empty-directory wizard. This step cannot create the first pool directly.")}</p>
            </Show>
            <Show when=move ||state.with(|s|active(&s.admin_console.storage))>
                <h3 class="zxs-row">{t("选择新主盘的空目标", "Choose an empty target for the new primary")}</h3>
                <Show when=move ||state.with(|s|targets(&s.admin_console.storage).is_empty())><p class="zxs-row" role="status">{t("没有已核验的空目标。请在发现父目录中准备独立空目录后刷新。", "No verified empty target is available. Prepare a separate empty directory under the discovery parent and refresh.")}</p></Show>
                <For each=move ||state.with(|s|targets(&s.admin_console.storage)) key=|selection|selection.selection_id.clone() children=move |selection|{
                    let path=StoredValue::new(selection.root_path);
                    view!{<button type="button" class="zxs-button" data-conversion-target=path.get_value()
                        aria-pressed=move ||state.with(|s|(s.admin_console.storage.conversion.target.as_ref()==Some(&path.get_value())).to_string())
                        disabled=move ||state.with(|s|!s.admin_console.storage.editable())
                        on:click=move |_|edit(state,|model|model.target=Some(path.get_value()))>{path.get_value()}</button>}
                }/>
                <label class="zxs-form-field"><span>{t("新主盘槽位（唯一）", "New primary slot (unique)")}</span><input class="zxs-input" type="text" autocomplete="off" data-conversion-slot=""
                    prop:value=move ||state.with(|s|s.admin_console.storage.conversion.slot.clone()) disabled=move ||state.with(|s|!s.admin_console.storage.editable())
                    on:input=move |event|edit(state,|model|model.slot=event_target_value(&event))/></label>
                <label class="zxs-row"><input type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.conversion.metadata)
                    disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:change=move |event|edit(state,|model|model.metadata=event_target_checked(&event))/>{t("同时保存元数据（否则仅存数据）", "Also store metadata (otherwise data only)")}</label>
                <button type="button" class="zxs-button" data-conversion-preview="" disabled=move ||state.with(|s|!s.admin_console.storage.editable()||s.admin_console.storage.conversion.target.is_none())
                    on:click=move |_|preview(state)>{t("检查普通目录转换", "Inspect ordinary-directory conversion")}</button>
            </Show>
        </Show>
    }
}
fn review(preview: &OrdinaryDiskConversionPreview) -> String {
    let target = &preview.target.request.additions[0];
    format!(
        "{}: {} → {} · {}: {} · {}: 1 · {}: {} · {}: {} · {}: {} B · {}: {} B",
        t("转换", "Conversion"),
        preview.source.path,
        target.selection.root_path,
        t("新主盘", "New primary"),
        target.slot_id,
        t("层级", "Tier"),
        t("文件", "Files"),
        preview.source_preview.files,
        t("目录", "Directories"),
        preview.source_preview.directories,
        t("内容", "Contents"),
        preview.source_preview.content_bytes,
        t("临时空间预算", "Temporary-space budget"),
        preview.source_preview.temporary_bytes
    )
}
fn publication_state(state: CommitState) -> String {
    match state {
        CommitState::Published => t("已发布", "Published"),
        CommitState::Acked => t("已确认发布", "Acknowledged publication"),
        CommitState::PublishedDegraded => t("已发布（降级）", "Published with degraded protection"),
        _ => unreachable!("invariant: conversion terminal evidence admits only published receipts"),
    }
}
fn status_text(status: &OrdinaryDiskConversionStatus) -> String {
    let phase = match status.phase {
        OrdinaryDiskConversionPhase::Registering => t(
            "正在接纳新一级盘，文件尚未导入。",
            "Registering the new primary; files have not been imported.",
        ),
        OrdinaryDiskConversionPhase::Importing => t(
            "新一级盘已接纳，正在导入；转换尚未完成。",
            "The new primary is registered; import is in progress. Conversion is not complete.",
        ),
        OrdinaryDiskConversionPhase::Completed => t(
            "转换完成：新一级盘已接纳，原文件名与内容已发布到新盘根目录。源目录保持不变。",
            "Conversion completed: the new primary is registered and original file names and contents are published at its root. The source directory is unchanged.",
        ),
        OrdinaryDiskConversionPhase::Stopping => t(
            "正在停止原转换并保留数据。服务器仍在核对永久结果；不能继续导入或新建替代操作。",
            "Stopping the original conversion and retaining data. The server is verifying its permanent outcome; do not resume import or start a replacement.",
        ),
        OrdinaryDiskConversionPhase::Stopped => t(
            "转换已停止，未完成内容导入。来源、已加入的新盘及暂存文件保留，未自动注销或清理；可以另行预览新任务。",
            "Conversion stopped; content import did not complete. The source, admitted new disk and staged files are retained, without automatic deregistration or cleanup. A new task may now be reviewed.",
        ),
        OrdinaryDiskConversionPhase::RecoveryRequired => t(
            "转换需要恢复。已接纳的盘不会自动撤销；请核对原操作和工作区。",
            "Conversion needs recovery. A registered disk is not automatically rolled back. Inspect the original operation and workspace.",
        ),
    };
    format!("{phase} {}", status.preview.operation_id)
}
pub(in super::super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s|s.admin_console.storage.conversion.preview.clone().filter(|_|!s.admin_console.storage.conversion.submitted&&s.admin_console.storage.conversion.status.is_none())).map(|preview|view!{
            <section class="zxs-frame zxs-stack" data-conversion-review="">
                <h3 class="zxs-row">{t("3. 核对新一级盘转换", "3. Review conversion into a new primary")}</h3>
                <p class="zxs-row">{review(&preview)}</p>
                <p class="zxs-row">{if preview.target.request.additions[0].metadata_member {t("新盘同时为元数据成员。", "The new disk also becomes a metadata member.")}else{t("新盘仅存数据，不加入元数据组。", "The new disk stores data only and does not join the metadata group.")}}</p>
                <p class="zxs-row">{t("先登记新主盘，再导入文件内容，两个阶段不原子。此转换不保留原权限、所有权或历史；临时字节预算不是可用空间保证。中断时保留原操作和工作区，不能把登记成功当作转换完成。", "Registration and content import are two non-atomic stages. This conversion does not retain original permissions, ownership or history. The temporary byte budget is not a free-space guarantee. After interruption, retain the original operation and workspace; successful registration alone is not completed conversion.")}</p>
                <label class="zxs-row"><input type="checkbox" data-conversion-confirm="" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:checked=move ||state.with(|s|s.admin_console.storage.conversion.acknowledged)
                    on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.conversion.acknowledged=event_target_checked(&event);}})/>{t("已核对源、空目标和一级角色，同意仅转换文件名与内容。", "I reviewed the source, empty target and primary role, and approve converting names and contents only.")}</label>
                <button type="button" class="zxs-button" data-conversion-commit="" disabled=move ||state.with(|s|!s.admin_console.storage.editable()||!s.admin_console.storage.conversion.acknowledged) on:click=move |_|submit(state,Submit::Commit)>{t("创建新一级盘并转换内容", "Create the new primary and convert contents")}</button>
            </section>
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.conversion.submitted)>
            <p class="zxs-row" role="status">{t("转换已提交。结果未确认时只核对原编号，不新建替代操作；关闭页面不会取消服务器处理。", "Conversion was submitted. While its outcome is unconfirmed, inspect the original ID rather than creating a replacement. Closing the page does not cancel server processing.")}</p>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.conversion.status.clone()).map(|status|view!{
            <section class="zxs-frame zxs-stack" data-conversion-status=""><p class="zxs-row" role="status">{status_text(&status)}</p><p class="zxs-row">{review(&status.preview)}</p>
                {status.result.map(|result|view!{<p class="zxs-row">{format!("{}: {} · {}: {}",t("发布状态", "Publication state"),publication_state(result.commit_state),t("当前内容健康", "Current content health"),result.current_health.as_str())}</p>})}
                {status.stop_result.map(|result|view!{<p class="zxs-row">{format!("{}: {} · {}",t("永久停止回执", "Permanent stop receipt"),result.operation_id,publication_state(result.commit_state))}</p>})}
                {status.workspace_id.map(|workspace|view!{<p class="zxs-row">{format!("{}: {workspace}",t("原工作区（保留的导入数据）", "Original workspace (retained import data)"))}</p>})}
                {status.root_operation_id.map(|id|view!{<p class="zxs-row">{format!("{}: {id}",t("根目录原操作", "Original root-directory operation"))}</p>})}
                {status.failure.map(|failure|view!{<p class="zxs-row" role="alert">{format!("{}: {}",failure.code,failure.message)}</p>})}
            </section>
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.conversion.submitted||s.admin_console.storage.conversion.status.is_some())>
            <button type="button" class="zxs-button" data-conversion-status-refresh="" disabled=move ||state.with(|s|s.admin_console.storage.conversion.reading) on:click=move |_|load(state)>{t("核对原转换状态", "Check original conversion status")}</button>
            <button type="button" class="zxs-button" data-conversion-resume="" disabled=move ||state.with(|s|!resumable(&s.admin_console.storage.conversion)) on:click=move |_|submit(state,Submit::Resume)>{t("继续同一转换操作", "Continue the same conversion operation")}</button>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.conversion.submitted)>
            <Show when=move ||state.with(|s|s.admin_console.storage.conversion.stop_requested)>
                <p class="zxs-row" role="status">{t("停止结果尚未确认，只查询或重试原停止请求，不会转而重新导入。", "The stop outcome is not yet confirmed. Only query or retry the original stop request; import will not restart.")}</p>
            </Show>
            <p class="zxs-row">{t("停止仅在原新盘已登记完成、服务器证明没有未决发布时成立。来源、新盘及暂存内容全部保留，不自动回滚、注销或删除；若原导入已经永久发布，仍按转换完成处理。", "Stopping requires completed registration and server proof that no publication is unresolved. The source, new disk and staged contents are retained, without automatic rollback, deregistration or deletion. If the original import is already permanently published, it remains completed.")}</p>
            <label class="zxs-row"><input type="checkbox" data-conversion-stop-confirm="" prop:checked=move ||state.with(|s|s.admin_console.storage.conversion.stop_acknowledged) disabled=move ||state.with(|s|!stoppable(&s.admin_console.storage.conversion))
                on:change=move |event|state.update(|s|{if s.session_active()&&stoppable(&s.admin_console.storage.conversion){s.admin_console.storage.conversion.stop_acknowledged=event_target_checked(&event);}})/>{t("确认停止原转换，保留来源、已加入新盘及暂存文件。", "Stop the original conversion and retain the source, admitted new disk and staged files.")}</label>
            <button type="button" class="zxs-button" data-conversion-stop="" disabled=move ||state.with(|s|!stoppable(&s.admin_console.storage.conversion)||!s.admin_console.storage.conversion.stop_acknowledged) on:click=move |_|submit(state,Submit::Stop)>{t("停止转换并保留数据", "Stop conversion and retain data")}</button>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.conversion.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    }
}
