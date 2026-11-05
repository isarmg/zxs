use super::*;

fn mappings(review: ForeignPlanPreview) -> impl IntoView {
    let source: BTreeMap<_, _> = review
        .source
        .histories
        .iter()
        .map(|history| (history.group_id.clone(), history.clone()))
        .collect();
    view! {<section class="zxs-stack" data-foreign-plan-review="">
        <h3 class="zxs-row">{t("4. 核对真实服务器审核", "4. Check the actual server review")}</h3>
        <p class="zxs-row">{format!("{}: {}",t("审核操作编号", "Review operation ID"),review.request.operation_id)}</p>
        <p class="zxs-row" data-foreign-plan-source="">{format!("{}: {}",t("已审核物理来源", "Reviewed physical source"),review.source.source.path)}</p>
        <p class="zxs-row">{format!("{}: {}",t("来源已发布检查点", "Source publication checkpoint"),review.source.checkpoint.publication_hash)}</p>
        <p class="zxs-row">{format!("{}: {}",t("目标一级身份", "Target primary identity"),super::super::origin_label(&review.target.identity))}</p>
        <p class="zxs-row">{format!("{}: {} · {}: {}",t("目标检查点", "Target checkpoint"),review.target.sequence,t("审核摘要", "Review digest"),review.review_digest)}</p>
        {review.target.histories.into_iter().map(|mapping|{
            let history=source.get(&mapping.group_id).expect("invariant: response validation covered every source history");
            view!{<div class="zxs-stack" data-foreign-plan-mapped=mapping.group_id>
                <p class="zxs-row">{format!("{} · {} → {}",super::super::origin_label(&history.origin),mapping.history_tree_id,mapping.destination)}</p>
                <p class="zxs-row">{format!("{}: {}",t("正文暂不可用，历史仍保留", "Contents temporarily unavailable; history retained"),history.temporarily_unavailable_versions.len())}</p>
            </div>}
        }).collect_view()}
        <p class="zxs-row" role="status">{t("服务器在本次观察中确认了空目标与互不碰撞的路径；这不预留目标。之后的变更必须重新审核，执行前仍须重验且不可覆盖。", "The server observed an empty target and non-colliding paths during this review. No target is reserved. Later changes require a new review, and any future execution must revalidate without overwriting.")}</p>
    </section>}
}
fn save_controls(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.review.is_some())>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.save==SaveState::Unsubmitted)>
            <label class="zxs-row"><input type="checkbox" data-foreign-plan-confirm=""
                prop:checked=move ||state.with(|s|s.admin_console.storage.foreign.plan.acknowledged)
                disabled=move ||state.with(|s|!editable(s)) on:change=move |event|state.update(|s|{
                    if editable(s)&&s.admin_console.storage.foreign.plan.save==SaveState::Unsubmitted{
                        s.admin_console.storage.foreign.plan.acknowledged=event_target_checked(&event);
                    }
                })/>{t("已核对所有来源、缺失内容和不可覆盖映射，仅同意保存系统盘审核记录；不授权迁入、降级或改写任何数据盘。", "I reviewed every origin, missing body and no-overwrite mapping. Save only the system-disk review record; this does not authorize import, tier conversion or any data-disk write.")}</label>
            <button type="button" class="zxs-button" data-foreign-plan-save="" disabled=move ||state.with(|s|!editable(s)||!s.admin_console.storage.foreign.plan.can_save())
                on:click=move |_|actions::save(state)>{t("仅保存审核计划", "Save review plan only")}</button>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.save==SaveState::Sending)>
            <p class="zxs-row" role="status">{t("正在保存原审核记录；关闭页面不代表撤销。没有迁入操作。", "Saving the original review record; closing the page does not revoke it. No import operation is running.")}</p>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.save==SaveState::Unknown)>
            <p class="zxs-row" role="alert">{t("审核记录保存结果未知。保留原编号和完整内容，只查询或重试同一保存请求，不新建替代编号。", "The review-save outcome is unknown. Keep the original ID and complete content; query or retry the same save, without creating a replacement ID.")}</p>
            <button type="button" class="zxs-button" data-foreign-plan-retry="" disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.storage.foreign.plan.can_save()) on:click=move |_|actions::save(state)>{t("重试原审核保存", "Retry original review save")}</button>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.save==SaveState::Rejected)>
            <p class="zxs-row" role="alert">{t("服务器拒绝了本次审核保存。请重新核对输入并生成新审核；没有迁入成功。", "The server rejected this review save. Check the inputs and obtain a new review; no import succeeded.")}</p>
        </Show>
        <button type="button" class="zxs-button" data-foreign-plan-execution-unavailable="" disabled=true>{t("迁入执行尚不可用", "Import execution is unavailable")}</button>
    </Show>}
}
fn stored(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s|s.admin_console.storage.foreign.plan.stored.clone()).map(|status|view!{
            <section class="zxs-stack" data-foreign-plan-saved="">
                <p class="zxs-row" role="status">{t("审核计划已保存到系统盘。数据盘未迁入、未降级、未重建；这不是迁入完成回执。", "The review plan is saved on the system disk. No data disk was imported, demoted or reconstructed; this is not an import completion receipt.")}</p>
                <p class="zxs-row">{format!("{}: {}",t("原审核编号", "Original review ID"),status.preview.request.operation_id)}</p>
                <p class="zxs-row">{match status.validation{
                    ForeignPlanValidation::Required=>t("保存的审核仍需重新核对，不保留路径或空间。", "The saved review requires revalidation and reserves neither paths nor space."),
                    ForeignPlanValidation::MatchedAtObservation=>t("来源与目标仅在刚才的观察中匹配；这不保留目标，也不解除执行阻断。", "Source and target matched only during the recent observation. This reserves no target and does not enable execution."),
                }}</p>
                <p class="zxs-row">{t("修改输入将建立新的审核，不会覆盖已保存的不可变记录。", "Editing inputs starts a new review and never overwrites the saved immutable record.")}</p>
            </section>
        })}
        <Show when=move ||state.with(|s|matches!(s.admin_console.storage.foreign.plan.save,SaveState::Unknown|SaveState::Stored))>
            <button type="button" class="zxs-button" data-foreign-plan-status="" disabled=move ||state.with(|s|!s.session_active()||s.admin_console.storage.foreign.plan.reading) on:click=move |_|actions::load(state,actions::Read::Status)>{t("查询原审核记录", "Query original review record")}</button>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.save==SaveState::Stored)>
            <button type="button" class="zxs-button" data-foreign-plan-revalidate="" disabled=move ||state.with(|s|!s.session_active()||s.admin_console.storage.foreign.plan.reading) on:click=move |_|actions::load(state,actions::Read::Revalidate)>{t("只读重验原来源与目标", "Revalidate the original source and target read-only")}</button>
        </Show>
    }
}
pub(in super::super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-stack" aria-label=t("外来历史重建审核向导", "Foreign history reconstruction review wizard")>
        {fields::form(state)}
        {move ||state.with(|s|s.admin_console.storage.foreign.plan.review.clone()).map(mappings)}
        {save_controls(state)}{stored(state)}
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.previewing||s.admin_console.storage.foreign.plan.reading)><p class="zxs-row" role="status">{t("正在重新核对完整来源与目标，尚未执行迁入…", "Rechecking the complete source and target; no import is being executed…")}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.foreign.plan.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    </section>}
}
