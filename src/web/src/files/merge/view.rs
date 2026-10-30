use super::*;
use web_interface::components::{Select, SelectOption};

const SELECTED_ROWS: usize = super::super::page::PAGE_SIZE;

fn cancel(state: &mut State, instance: u64) {
    if !actions::pending(state)
        && state
            .files
            .merge
            .as_ref()
            .is_some_and(|draft| draft.instance == instance)
    {
        state.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Merge,
            instance,
        });
        state.files.merge = None;
        state.files.publications.remove(&review::Kind::Merge);
    }
}

fn move_selected_page(state: RwSignal<State>, instance: u64, next: bool) {
    state.update(|s| {
        if let Some(draft) = s
            .files
            .merge
            .as_mut()
            .filter(|draft| draft.instance == instance)
        {
            if next && (draft.selected_page + 1) * SELECTED_ROWS < draft.selected.len() {
                draft.selected_page += 1;
            } else if !next && draft.selected_page > 0 {
                draft.selected_page -= 1;
            }
        }
    });
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let instances =
        Memo::new(move |_| state.with(|s| s.files.merge.as_ref().map(|draft| draft.instance)));
    let branches = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .file_history
                .branches()
                .iter()
                .filter(|branch| branch.writable())
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let selected = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| {
                    draft
                        .selected
                        .values()
                        .skip(draft.selected_page * SELECTED_ROWS)
                        .take(SELECTED_ROWS)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let disabled = Signal::derive(move || state.with(|s| !actions::history_write_allowed(s)));
    let navigation_disabled =
        Signal::derive(move || state.with(|s| !actions::history_navigation_ready(s)));
    let review_needed = Signal::derive(move || {
        state.with(|s| {
            s.files.merge.as_ref().is_some_and(|draft| {
                !s.files.file_history.ready() || s.files.file_history.root() != Some(draft.root())
            })
        })
    });
    let result_options = Signal::derive(move || {
        state.with(|s| {
            let mut options = s
                .files
                .file_history
                .nodes()
                .iter()
                .filter(|node| node.visible_content())
                .enumerate()
                .map(|(index, node)| SelectOption::new(node.id(), node_label(node, index)))
                .collect::<Vec<_>>();
            if let Some(chosen) = s
                .files
                .merge
                .as_ref()
                .and_then(|draft| draft.chosen.as_ref())
            {
                if !s
                    .files
                    .file_history
                    .nodes()
                    .iter()
                    .any(|node| node.id() == chosen.id)
                {
                    options.push(SelectOption::new(
                        &chosen.id,
                        format!("{} · {}", t("所选节点", "Selected node"), chosen.id),
                    ));
                }
            }
            options
        })
    });
    let survivor_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| {
                    let mut branches = draft
                        .selected
                        .values()
                        .skip(draft.selected_page * SELECTED_ROWS)
                        .take(SELECTED_ROWS)
                        .collect::<Vec<_>>();
                    if let Some(branch) = draft.selected.get(&draft.survivor) {
                        if !branches.iter().any(|candidate| candidate.id == branch.id) {
                            branches.push(branch);
                        }
                    }
                    branches
                        .into_iter()
                        .map(|branch| SelectOption::new(&branch.id, &branch.name))
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let result = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .and_then(|draft| draft.chosen.as_ref())
                .map(|node| node.id.clone())
                .unwrap_or_default()
        })
    });
    let survivor = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| draft.survivor.clone())
                .unwrap_or_default()
        })
    });
    view! {
        <For
            each={move || instances.get().into_iter().collect::<Vec<_>>()}
            key={|instance| *instance}
            children={move |instance| view! {
            <section class="zxs-frame zxs-stack zxs-merge-review" aria-label=t("合并文件分支","Merge file branches")>
                <div class="zxs-file-toolbar zxs-flow">
                    <h3 class="zxs-row">{t("合并文件分支","Merge file branches")}</h3>
                    <button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(|s|cancel(s,instance))>{t("取消","Cancel")}</button>
                </div>
                <p class="zxs-row">{t("保留分支采用结果节点，其余参与分支保留原节点并变为只读别名。可分页浏览本文件任一历史节点；换页不会清除所选依据。","The surviving branch adopts the result node; other selected branches retain their nodes as read-only aliases. Browse any history node of this file across pages; page changes preserve selected bases.")}</p>
                <Show when=move ||review_needed.get()>
                    <p class="zxs-row" role="status">{review_required()}</p>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get() on:click=move |_|actions::review_history_merge(state,instance)>{t("读取全部所选依据以重新审阅","Load all selected bases for review")}</button>
                </Show>
                <Show when=move ||state.with(|s|s.files.merge.as_ref().is_some_and(|draft|draft.resolved.is_some()))>
                    <div class="zxs-frame zxs-stack">
                        <p class="zxs-row">{t("全部所选身份已重新读取。请检查下面的旧依据与新依据，再明确接受。","Every selected identity has been read again. Check the previous and current bases below, then explicitly accept the review.")}</p>
                        {move ||state.with(|s|s.files.merge.as_ref().map(|draft|{
                            draft.selected.values().skip(draft.selected_page*SELECTED_ROWS).take(SELECTED_ROWS).filter_map(|old|{
                                draft.resolved.as_ref()?.selected.get(&old.id).map(|new|format!("{}: {} → {}",old.name,old.expected,new.expected))
                            }).collect::<Vec<_>>()
                        }).unwrap_or_default()).into_iter().map(|line|view!{<p class="zxs-row">{line}</p>}).collect_view()}
                        <button class="zxs-button" disabled=move ||navigation_disabled.get() on:click=move |_|accept_review(state,instance)>{t("接受重新审阅的全部依据","Accept every reviewed base")}</button>
                    </div>
                </Show>
                <fieldset class="zxs-review-choices">
                    <legend class="zxs-row">{t("本页可写分支","Writable branches on this page")}</legend>
                    {move ||branches.get().into_iter().map(move |branch|{
                        let id=branch.id().to_owned();let checked_id=id.clone();
                        view!{<label class="zxs-batch-check zxs-row">
                            <input type="checkbox" disabled=move ||disabled.get()||review_needed.get() prop:checked=move ||state.with(|s|s.files.merge.as_ref().is_some_and(|draft|draft.selected.contains_key(&checked_id))) on:change=move |event|select_branch(state,instance,id.clone(),event_target_checked(&event))/>
                            <span>{branch.name().to_owned()}</span>
                        </label>}
                    }).collect_view()}
                </fieldset>
                <div class="zxs-file-toolbar zxs-flow">
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.previous_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,false)>{t("上一页分支","Previous branches")}</button>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.next_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,true)>{t("下一页分支","Next branches")}</button>
                </div>
                <p class="zxs-row" role="heading" {web_interface::attr::custom::custom_attribute("aria-level", "4")}>{t("跨页所选分支","Selected branches across pages")}</p>
                {move ||selected.get().into_iter().map(move |branch|{
                    let id=branch.id.clone();
                    view!{<div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{format!("{} · {}",branch.name,branch.expected)}</span><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|select_branch(state,instance,id.clone(),false)>{t("移除选择","Remove selection")}</button></div>}
                }).collect_view()}
                <div class="zxs-file-toolbar zxs-flow">
                    <span class="zxs-row">{move ||state.with(|s|s.files.merge.as_ref().map(|draft|format!("{} {}",draft.selected.len(),t("个分支已选择","branches selected"))).unwrap_or_default())}</span>
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.merge.as_ref().is_none_or(|draft|draft.selected_page==0)) on:click=move |_|move_selected_page(state,instance,false)>{t("上一页所选项","Previous selected items")}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.merge.as_ref().is_none_or(|draft|(draft.selected_page+1)*SELECTED_ROWS>=draft.selected.len())) on:click=move |_|move_selected_page(state,instance,true)>{t("下一页所选项","Next selected items")}</button>
                </div>
                <div class="zxs-file-toolbar zxs-flow">
                    <Select label=t("结果节点（可选此文件任一节点）","Result node (any node of this file)") options=result_options value=result disabled=Signal::derive(move ||disabled.get()||review_needed.get()) on_change=Callback::new(move |value:String|choose_node(state,instance,value))/>
                    <Select label=t("合并后保留的分支","Surviving branch") options=survivor_options value=survivor disabled=Signal::derive(move ||disabled.get()||review_needed.get()) on_change=Callback::new(move |value:String|choose_survivor(state,instance,value))/>
                </div>
                <div class="zxs-file-toolbar zxs-flow">
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.previous_cursor().is_none()) on:click=move |_|actions::page_history_nodes(state,false)>{t("上一页节点","Previous nodes")}</button>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.next_cursor().is_none()) on:click=move |_|actions::page_history_nodes(state,true)>{t("下一页节点","Next nodes")}</button>
                    <input class="zxs-input zxs-input--compact" aria-label=t("精确历史节点 ID","Exact history node ID") maxlength=256 prop:value=move ||state.with(|s|s.files.merge.as_ref().map(|draft|draft.anchor.clone()).unwrap_or_default()) on:input=move |event|state.update(|s|{if let Some(draft)=s.files.merge.as_mut().filter(|draft|draft.instance==instance){draft.anchor=event_target_value(&event);}})/>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get() on:click=move |_|{let id=state.with_untracked(|s|s.files.merge.as_ref().map(|draft|draft.anchor.trim().to_owned()).unwrap_or_default());if !id.is_empty(){actions::anchor_history_node(state,id);}}>{t("定位节点","Locate node")}</button>
                </div>
                <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Merge))}</p>
                <button class="zxs-button" disabled=move ||disabled.get()||review_needed.get() on:click=move |_|submit(state,instance)>{t("确认合并所选分支","Confirm merge of selected branches")}</button>
            </section>
        }}/>
    }
}

pub(crate) fn history(state: RwSignal<State>) -> impl IntoView {
    super::super::history::provenance_view(state)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/view_tests.rs"]
mod tests;
