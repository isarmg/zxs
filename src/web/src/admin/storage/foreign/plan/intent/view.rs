use super::*;
use web_interface::view_renderer::html::attribute::custom::custom_attribute;

pub(in super::super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.stored.is_some())>
        <section class="zxs-stack" aria-label=t("保存目标盘执行意图", "Save a target-disk execution intent") data-foreign-intent="">
            <h3 class="zxs-row">{t("5. 可选：保存目标盘执行意图", "5. Optional: save a target-disk execution intent")}</h3>
            <p class="zxs-row" id="zxs-foreign-intent-scope">{t("这一步会在已审核的目标数据盘保存不可变意图。不会复制正文、接纳磁盘或发布目录；来源不改写，目标路径和空间也不会预留。正式迁入仍不可用。", "This step saves an immutable intent on the reviewed target data disk. It does not copy contents, admit a disk or publish a directory. The source is unchanged, and no path or space is reserved. Import remains unavailable.")}</p>
            <button type="button" class="zxs-button" data-foreign-intent-preview="" disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.storage.editable()||!s.admin_console.storage.foreign.plan.intent.can_preview()) on:click=move |_|actions::preview(state)>{t("重新核验并预览执行意图", "Revalidate and preview the execution intent")}</button>
            {move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.preview.clone()).map(|preview|view!{
                <section class="zxs-stack" data-foreign-intent-review="" id="zxs-foreign-intent-review">
                    <p class="zxs-row">{format!("{}: {}",t("执行意图编号", "Execution intent ID"),preview.request.operation_id)}</p>
                    <p class="zxs-row">{format!("{}: {}",t("原审核编号", "Original review ID"),preview.request.plan.operation_id)}</p>
                    <p class="zxs-row">{format!("{}: {}",t("目标数据盘", "Target data disk"),super::super::super::origin_label(&preview.plan.target.identity))}</p>
                    <p class="zxs-row">{format!("{}: {} · {}: {}",t("当前主体", "Current principal"),preview.principal,t("执行意图摘要", "Intent digest"),preview.intent_digest)}</p>
                </section>
            })}
            <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.preview.is_some()&&s.admin_console.storage.foreign.plan.intent.save==SaveState::Unsubmitted)>
                <label class="zxs-row"><input type="checkbox" data-foreign-intent-confirm="" {..custom_attribute("aria-describedby","zxs-foreign-intent-review zxs-foreign-intent-scope")}
                    prop:checked=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.acknowledged)
                    disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.storage.editable())
                    on:change=move |event|state.update(|s|{
                        if s.session_active()&&s.admin_console.storage.editable(){
                            let model=&mut s.admin_console.storage.foreign.plan.intent;
                            if model.save==SaveState::Unsubmitted {model.acknowledged=event_target_checked(&event);}
                        }
                    })/>{t("我已核对全部来源及目标，仅确认在上述目标数据盘保存这个执行意图；不授权复制、接纳或发布。", "I reviewed every source and destination. Save only this execution intent on the target data disk above; do not copy, admit or publish.")}</label>
                <button type="button" class="zxs-button" data-foreign-intent-save="" {..custom_attribute("aria-describedby","zxs-foreign-intent-review zxs-foreign-intent-scope")} disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.storage.editable()||!s.admin_console.storage.foreign.plan.intent.can_save()) on:click=move |_|actions::save(state)>{t("仅保存目标盘意图", "Save target-disk intent only")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.save==SaveState::Sending)>
                <p class="zxs-row" role="status">{t("正在保存原执行意图。离开页面不撤销服务器写入，也不表示迁入开始。", "Saving the original execution intent. Leaving does not revoke the server write, and no import has started.")}</p>
            </Show>
            <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.save==SaveState::Unknown)>
                <p class="zxs-row" role="alert">{t("目标盘保存结果未知，原编号、目标身份和摘要已保留。只能查询或重试同一请求；未找到或重试被拒绝都不能证明早前未保存。", "The target-disk save outcome is unknown. The original ID, target identity and digest are retained. Query or retry the same request; not-found or a rejected retry cannot disprove an earlier save.")}</p>
                <button type="button" class="zxs-button" data-foreign-intent-retry="" disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.storage.foreign.plan.intent.can_save()) on:click=move |_|actions::save(state)>{t("重试原执行意图保存", "Retry the original intent save")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.save==SaveState::Rejected)>
                <p class="zxs-row" role="alert">{t("本次意图保存被明确拒绝。核对错误后重新预览和确认；没有接入成功。", "This intent save was explicitly rejected. Check the error, then preview and confirm again. No admission succeeded.")}</p>
            </Show>
            {move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.stored.clone()).map(|status|view!{
                <p class="zxs-row" role="status" data-foreign-intent-saved="">{format!("{} {} · {}: {} · {}: {}",t("目标盘执行意图已保存，未复制或接入。请保留编号：", "Target-disk intent saved; nothing copied or admitted. Keep this ID:"),status.address.operation_id,t("完整历史组", "Complete history groups"),status.histories,t("正文暂不可用", "Temporarily unavailable contents"),status.temporarily_unavailable_versions)}</p>
            })}
            <Show when=move ||state.with(|s|matches!(s.admin_console.storage.foreign.plan.intent.save,SaveState::Unknown|SaveState::Stored))>
                <button type="button" class="zxs-button" data-foreign-intent-status="" disabled=move ||state.with(|s|!s.session_active()||s.admin_console.storage.foreign.plan.intent.reading) on:click=move |_|actions::status(state)>{t("查询原目标盘执行意图", "Query the original target-disk intent")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.previewing||s.admin_console.storage.foreign.plan.intent.reading)><p class="zxs-row" role="status">{t("正在只读核对执行意图…", "Checking the execution intent read-only…")}</p></Show>
            {move ||state.with(|s|s.admin_console.storage.foreign.plan.intent.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
        </section>
    </Show>}
}
