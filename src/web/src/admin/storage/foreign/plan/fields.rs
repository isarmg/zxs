use super::*;
use actions::edit;

fn mode_choices(state: RwSignal<State>) -> impl IntoView {
    view! {<div class="zxs-stack">
        <h3 class="zxs-row">{t("2. 选择重建意图", "2. Choose reconstruction intent")}</h3>
        {[Mode::ReconstructExistingPrimary, Mode::AdmitNewPrimary, Mode::AdmitSecondary].into_iter().map(|mode|view!{
            <button type="button" class="zxs-button" data-foreign-plan-mode=format!("{mode:?}")
                aria-pressed=move ||state.with(|s|(s.admin_console.storage.foreign.plan.draft.mode==mode).to_string())
                disabled=move ||state.with(|s|!editable(s))
                on:click=move |_|edit(state,|draft|draft.mode=mode)>{mode.label()}</button>
        }).collect_view()}
        <p class="zxs-row">{t("新一级与降级方式仅记录在当前页面草稿中，当前不会发送审核或执行请求。", "New-primary and secondary modes remain local page drafts; no review or execution request is sent for them.")}</p>
    </div>}
}
fn destination(state: RwSignal<State>) -> impl IntoView {
    view! {<div class="zxs-stack">
        <label class="zxs-form-field"><span>{t("目标槽位（明确指定）", "Target slot (explicit)")}</span><input class="zxs-input" type="text" autocomplete="off" data-foreign-plan-slot=""
            prop:value=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.slot.clone()) disabled=move ||state.with(|s|!editable(s))
            on:input=move |event|edit(state,|draft|draft.slot=event_target_value(&event))/></label>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.mode==Mode::ReconstructExistingPrimary)>
            <label class="zxs-form-field"><span>{t("已存在的空父目录（留空表示盘根）", "Existing empty parent directory (empty means disk root)")}</span><input class="zxs-input" type="text" autocomplete="off" data-foreign-plan-parent=""
                prop:value=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.parent.clone()) disabled=move ||state.with(|s|!editable(s))
                on:input=move |event|edit(state,|draft|draft.parent=event_target_value(&event))/></label>
            <p class="zxs-row">{t("服务器将核对现有一级、目标身份和空目录。预览不创建目录、不覆盖内容，也不预留目标。", "The server checks the existing primary, exact target identity and empty directory. Preview creates no directory, overwrites nothing and reserves no target.")}</p>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.mode!=Mode::ReconstructExistingPrimary)>
            <h3 class="zxs-row">{t("可选独立复制目标（仅草稿）", "Optional separate copy target (draft only)")}</h3>
            <p class="zxs-row">{t("新角色的原盘接纳或复制方案尚未确定。此选择仅记录设计意图，不是已审核转换能力，也不要求先准备空盘。", "In-place admission versus copying for a new role is not yet defined. This choice only records design intent; it is not a reviewed conversion capability and does not require preparing an empty disk.")}</p>
            <button type="button" class="zxs-button" data-foreign-draft-target-none=""
                aria-pressed=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.empty_target.is_none().to_string())
                disabled=move ||state.with(|s|!editable(s)) on:click=move |_|edit(state,|draft|draft.empty_target=None)>{t("不指定复制目标", "Leave copy target unspecified")}</button>
            <For each=move ||state.with(|s|s.admin_console.storage.wizard.candidates().into_iter().filter_map(|candidate|match candidate.state{DiscoveredDiskState::Empty{selection}=>Some(selection),_=>None}).collect::<Vec<_>>())
                key=|selection|selection.selection_id.clone() children=move |selection|{
                    let selection=StoredValue::new(selection);
                    view!{<button type="button" class="zxs-button" data-foreign-draft-target=selection.get_value().root_path
                        aria-pressed=move ||state.with(|s|(s.admin_console.storage.foreign.plan.draft.empty_target.as_ref()==Some(&selection.get_value())).to_string())
                        disabled=move ||state.with(|s|!editable(s)) on:click=move |_|edit(state,|draft|draft.empty_target=Some(selection.get_value()))>{selection.get_value().root_path}</button>}
                }/>
        </Show>
    </div>}
}
fn primary(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.mode==Mode::AdmitSecondary)>
        <h3 class="zxs-row">{t("必须明确选择一级身份", "Explicit primary identity required")}</h3>
        <p class="zxs-row">{t("只有三级来源可降为二级，一级身份与绑定代数必须同时绑定，不能先成功再补上游。以下只列本页真实服务器审核曾返回的一级身份，未来执行仍须重验。", "Only a tier-3 source may become tier 2, with its primary identity and binding generation bound together. There is no intermediate success before selecting an upstream. Choices below are primary identities returned by actual server reviews on this page; future execution must revalidate them.")}</p>
        <Show when=move ||state.with(|s|s.admin_console.storage.foreign.plan.known_primary.is_empty())>
            <p class="zxs-row" role="status">{t("尚无完整的已审核一级身份。当前API没有一级身份选择清单；可先审核一个已有一级空目标。这里不会按槽位猜测身份或默认选择。", "No complete reviewed primary identity is available. The current API has no primary-identity selection inventory; first review an existing primary's empty target. A slot name is never used to guess an identity or select a default.")}</p>
        </Show>
        <For each=move ||state.with(|s|s.admin_console.storage.foreign.plan.known_primary.clone()) key=|primary|(primary.identity.pool_id.clone(),primary.identity.disk_id.clone(),primary.identity.slot_id.clone(),primary.identity.binding_generation,primary.sequence)
            children=move |primary|{let primary=StoredValue::new(primary);view!{
                <button type="button" class="zxs-button" data-foreign-draft-primary=primary.get_value().identity.disk_id
                    aria-pressed=move ||state.with(|s|(s.admin_console.storage.foreign.plan.draft.primary.as_ref()==Some(&primary.get_value())).to_string())
                    disabled=move ||state.with(|s|!editable(s)) on:click=move |_|edit(state,|draft|draft.primary=Some(primary.get_value()))>{format!("{} · {} {}",super::super::origin_label(&primary.get_value().identity),t("检查点", "checkpoint"),primary.get_value().sequence)}</button>
            }}/>
    </Show>}
}
// A retiring keyed row may observe the new draft before its owner is removed.
// An absent display value is empty; Draft::intent still rejects incomplete mappings.
fn history_names(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-stack" aria-label=t("逐历史来源映射", "Per-history origin mapping")>
        <h3 class="zxs-row">{t("3. 分别命名各份来源历史", "3. Name each source history separately")}</h3>
        <p class="zxs-row">{t("同名仓库或相同槽位不合并。映射按历史组身份保留原来源；暂不可用正文继续保留历史，不视为无效内容。", "Equal repository names or slots do not merge sources. Each mapping retains its history-group identity and original origin. Temporarily unavailable bodies retain their history and are not treated as invalid contents.")}</p>
        <For each=move ||state.with(|s|s.admin_console.storage.foreign.preview.as_ref().map(|p|p.histories.clone()).unwrap_or_default()) key=|history|history.group_id.clone()
            children=move |history|{let group=StoredValue::new(history.group_id.clone());view!{
                <section class="zxs-stack" data-foreign-plan-group=history.group_id>
                    <h3 class="zxs-row">{history.repository_name}</h3>
                    <p class="zxs-row">{super::super::origin_label(&history.origin)}</p><p class="zxs-row">{history.history_tree_id}</p>
                    <p class="zxs-row" data-foreign-retained-missing=history.temporarily_unavailable_versions.len().to_string()>{format!("{}: {} · {}: {}",t("已核验本地正文", "Verified local contents"),history.verified_local_versions,t("正文暂不可用，保留历史", "Contents temporarily unavailable; history retained"),history.temporarily_unavailable_versions.len())}</p>
                    <label class="zxs-form-field"><span>{t("独立目标子目录名", "Distinct destination child name")}</span><input class="zxs-input" type="text" autocomplete="off" data-foreign-plan-name=group.get_value()
                        prop:value=move ||state.with(|s|s.admin_console.storage.foreign.plan.draft.names.get(&group.get_value()).cloned().unwrap_or_default())
                        disabled=move ||state.with(|s|!editable(s)) on:input=move |event|edit(state,|draft|{draft.names.insert(group.get_value(),event_target_value(&event));})/></label>
                </section>
            }}/>
    </section>}
}
pub(super) fn form(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.storage.foreign.preview.is_some())>
        <section class="zxs-stack" data-foreign-plan-form="">
            {mode_choices(state)}{destination(state)}{primary(state)}{history_names(state)}
            <p class="zxs-row" role="status">{move ||state.with(|s|{
                let foreign=&s.admin_console.storage.foreign;
                let Some(source)=&foreign.preview else{return String::new()};
                if foreign.plan.review.is_some(){return t("当前映射的服务器审核见下方；审核不执行迁入，也不预留目标。", "The server review for this mapping is shown below. A review neither executes import nor reserves the target.")};
                match foreign.plan.draft.intent(source,&foreign.plan.known_primary){
                    Ok(Intent::ReconstructExistingPrimary(_))=>t("映射已填写，尚未经服务器核验。", "Mapping is entered but has not been verified by the server."),
                    Ok(_)=>DraftError::PreviewUnavailable.message(),
                    Err(error)=>error.message(),
                }
            })}</p>
            <button type="button" class="zxs-button" data-foreign-plan-preview="" disabled=move ||state.with(|s|{
                let foreign=&s.admin_console.storage.foreign;
                !editable(s)||foreign.preview.as_ref().is_none_or(|source|!matches!(foreign.plan.draft.intent(source,&foreign.plan.known_primary),Ok(Intent::ReconstructExistingPrimary(_))))
            }) on:click=move |_|actions::preview(state)>{t("只读核验不可覆盖映射", "Review no-overwrite mapping read-only")}</button>
        </section>
    </Show>}
}
