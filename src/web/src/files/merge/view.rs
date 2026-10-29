use super::*;
use crate::files::history::state::AuxPanel;
use web_interface::components::{Select, SelectOption};

fn cancel(state: &mut State) {
    if !actions::pending(state) {
        state.files.merge = None;
        state.files.publications.remove(&review::Kind::Merge);
    }
}

/// Result candidates are the nodes of the resident page. The current choice
/// stays listed (from its summary) even when its page was replaced.
fn result_options(model: &Model) -> Vec<SelectOption> {
    let Some(draft) = model.merge.as_ref() else {
        return Vec::new();
    };
    let mut options = model
        .history
        .window()
        .map(|window| {
            window
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    SelectOption::new(node.node_id.clone(), node_label(node, index))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(summary) = draft.chosen_summary()
        && !options.iter().any(|option| option.value == summary.node_id)
    {
        options.insert(
            0,
            SelectOption::new(
                summary.node_id.clone(),
                format!("{} · {}", t("已选", "Chosen"), summary_label(summary)),
            ),
        );
    }
    options
}

/// Current candidates plus one local page of explicit selections. Candidate
/// paging retains neither unselected records nor an unbounded checkbox list.
fn branch_choices(model: &Model) -> Vec<InputBranch> {
    let Some(draft) = model.merge.as_ref() else {
        return Vec::new();
    };
    let mut choices = model
        .history
        .branch_page()
        .map(|page| {
            page.writable()
                .map(InputBranch::capture)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for branch in draft.branches().skip(draft.offset).take(SELECTED_PAGE) {
        if !choices.iter().any(|candidate| candidate.id == branch.id) {
            choices.push(branch.clone());
        }
    }
    choices
}

fn survivor_options(model: &Model) -> Vec<SelectOption> {
    let Some(draft) = model.merge.as_ref() else {
        return Vec::new();
    };
    let mut options = draft
        .branches()
        .skip(draft.offset)
        .take(SELECTED_PAGE)
        .map(|branch| SelectOption::new(branch.id.clone(), branch.name.clone()))
        .collect::<Vec<_>>();
    if let Some(branch) = draft.branch_heads.get(&draft.survivor)
        && !options.iter().any(|option| option.value == branch.id)
    {
        options.insert(
            0,
            SelectOption::new(
                branch.id.clone(),
                format!("{} · {}", t("已选", "Selected"), branch.name),
            ),
        );
    }
    options
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.merge.is_some()));
    let branches = Memo::new(move |_| state.with(|s| branch_choices(&s.files)));
    // Submitting needs a current root; cancelling never does.
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .merge
                    .as_ref()
                    .is_none_or(|draft| s.files.history.root() != Some(&draft.root))
        })
    });
    let result_options = Signal::derive(move || state.with(|s| result_options(&s.files)));
    let survivor_options = Signal::derive(move || state.with(|s| survivor_options(&s.files)));
    let selected_page = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| (draft.offset, draft.branch_heads.len()))
                .unwrap_or_default()
        })
    });
    let result = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| draft.chosen.clone())
                .unwrap_or_default()
        })
    });
    let chosen_summary = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .and_then(|draft| draft.chosen_summary().map(summary_label))
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
    let paging = Memo::new(move |_| {
        state.with(|s| {
            let window = s.files.history.window();
            (
                window.and_then(|window| window.previous_cursor.clone()),
                window.and_then(|window| window.next_cursor.clone()),
                s.files.history.loading(),
            )
        })
    });
    view! {<Show when=move ||open.get()><section class="zxs-frame zxs-stack zxs-merge-review" aria-label=t("合并文件分支","Merge file branches")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("合并文件分支","Merge file branches")}</h3><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(cancel)>{t("取消","Cancel")}</button></div>
        <p class="zxs-row">{t("保留分支采用结果节点，其余参与分支保留原节点并变为只读别名；未选择的可写文件分支保持不变。所有当前命名目录分支中引用参与分支的对应条目（含当前未显示的目录分支）将更新到结果节点，其他文件引用不变。","The surviving branch adopts the result node. Other selected branches keep their original nodes as read-only aliases; unselected writable file branches stay unchanged. Matching references in all current named folder branches, including branches not currently displayed, advance to the result node; other file references stay unchanged.")}</p>
        <Show when=move ||state.with(|s|s.files.merge.as_ref().is_some_and(|draft|s.files.history.root()!=Some(&draft.root)))><p class="zxs-row zxs-inline-error" role="status">{t("历史已变化：此草稿保留为待重审输入，请取消并重新选择参与分支与结果节点。","The history changed: this draft is kept as input awaiting review. Cancel and select the input branches and result node again.")}</p></Show>
        <fieldset class="zxs-review-choices"><legend class="zxs-row">{t("参与分支","Input branches")}</legend>{move ||branches.get().into_iter().map(move |branch|{let id=branch.id.clone();let checked_id=id.clone();view!{<label class="zxs-batch-check zxs-row"><input type="checkbox" disabled=move ||disabled.get() prop:checked=move ||state.with(|s|s.files.merge.as_ref().is_some_and(|draft|draft.branch_heads.contains_key(&checked_id))) on:change=move |event|toggle_input(state,&id,event_target_checked(&event))/><span>{branch.name.clone()}</span></label>}}).collect_view()}</fieldset>
        {super::super::branch::paging(state)}
        <div class="zxs-file-toolbar zxs-flow">
            <span class="zxs-row">{move ||selected_page.with(|(_,count)|format!("{count} {}",t("个参与分支已选择", "input branches selected")))}</span>
            <Show when=move ||selected_page.with(|(_,count)|*count>SELECTED_PAGE)>
                <button class="zxs-button" disabled=move ||selected_page.with(|(offset,_)|*offset==0) on:click=move |_|update(state,|draft|draft.offset=draft.offset.saturating_sub(SELECTED_PAGE))>{t("上一页参与分支", "Previous selected inputs")}</button>
                <button class="zxs-button" disabled=move ||selected_page.with(|(offset,count)|offset+SELECTED_PAGE>=*count) on:click=move |_|update(state,|draft|{if draft.offset+SELECTED_PAGE<draft.branch_heads.len(){draft.offset+=SELECTED_PAGE;}})>{t("下一页参与分支", "Next selected inputs")}</button>
            </Show>
        </div>
        <div class="zxs-file-toolbar zxs-flow"><Select label=t("结果节点（可选此文件任一节点，可翻页查找）","Result node (any node of this file; page to find it)") options=result_options value=result disabled=disabled on_change=Callback::new(move |value:String|{state.update(|s|{choose(&mut s.files,&value);});})/><Select label=t("合并后保留的分支","Surviving branch") options=survivor_options value=survivor disabled=disabled on_change=Callback::new(move |value:String|update(state,|draft|draft.survivor=value))/></div>
        <div class="zxs-file-toolbar zxs-flow">
            <span class="zxs-row">{move ||chosen_summary.get().map(|label|format!("{}: {label}",t("当前所选结果","Current result"))).unwrap_or_else(||t("尚未选择结果节点。","No result node chosen yet."))}</span>
            <button class="zxs-button" disabled=move ||paging.with(|(previous,_,loading)|previous.is_none()||*loading) on:pointerdown=move |event|actions::history::preserve_editor_focus(state,&event) on:click=move |_|{if let Some(cursor)=paging.with_untracked(|(previous,_,_)|previous.clone()){actions::history_page(state,cursor);}}>{t("较新节点","Newer nodes")}</button>
            <button class="zxs-button" disabled=move ||paging.with(|(_,next,loading)|next.is_none()||*loading) on:pointerdown=move |event|actions::history::preserve_editor_focus(state,&event) on:click=move |_|{if let Some(cursor)=paging.with_untracked(|(_,next,_)|next.clone()){actions::history_page(state,cursor);}}>{t("较早节点","Older nodes")}</button>
        </div>
        <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Merge))}</p>
        <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|submit(state)>{t("确认合并所选分支","Confirm merge of selected branches")}</button>
    </section></Show>}
}

/// Display rows of the resident provenance page.
#[derive(Clone, PartialEq, Eq)]
struct MergeRow {
    merge_id: String,
    survivor: String,
    chosen: String,
    result: String,
    reused: bool,
    input_count: usize,
}
fn rows(state: &State) -> Vec<MergeRow> {
    let Some(page) = state.files.history.merge_page() else {
        return Vec::new();
    };
    let window = state.files.history.window();
    let branch_name = |id: &str| {
        super::super::history_index::branch(state, id)
            .map(|branch| branch.name().to_owned())
            .unwrap_or_else(|| t("历史分支", "Historical branch"))
    };
    let node_name = |id: &str| {
        window
            .and_then(|window| {
                window
                    .nodes
                    .iter()
                    .position(|node| node.node_id == id)
                    .map(|index| node_label(&window.nodes[index], index))
            })
            .unwrap_or_else(|| t("历史节点", "Historical node"))
    };
    page.merges
        .iter()
        .map(|merge| MergeRow {
            merge_id: merge.merge_id.clone(),
            survivor: branch_name(&merge.surviving_branch_id),
            chosen: node_name(&merge.chosen_node_id),
            result: node_name(&merge.result_node_id),
            reused: merge.reused_node,
            input_count: merge.input_count,
        })
        .collect()
}

/// The acyclic tree may deliberately omit an edge when reusing an ancestor.
/// Preserve all inputs in a separate provenance list instead of inventing
/// another content node or hiding the source branch history.
pub(crate) fn history(state: RwSignal<State>) -> impl IntoView {
    let records = Memo::new(move |_| state.with(rows));
    let paging = Memo::new(move |_| {
        state.with(|s| {
            let page = s.files.history.merge_page();
            (
                page.is_some(),
                page.and_then(|page| page.previous_cursor.clone()),
                page.and_then(|page| page.next_cursor.clone()),
                s.files.history.root().is_some(),
            )
        })
    });
    let sources = Memo::new(move |_| {
        state.with(|s| match s.files.history.panel.as_ref() {
            Some(AuxPanel::MergeSources { merge_id, page }) => {
                Some((merge_id.clone(), page.clone()))
            }
            _ => None,
        })
    });
    view! {<section class="zxs-merge-history zxs-stack" aria-label=t("合并来源记录","Merge provenance")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("合并来源记录","Merge provenance")}</h3>
            <Show when=move ||paging.with(|(loaded,_,_,current)|!*loaded&&*current)><button class="zxs-button" on:click=move |_|actions::load_history_merges(state,None)>{t("加载合并记录","Load merge records")}</button></Show>
            <Show when=move ||paging.with(|(loaded,_,_,_)|*loaded)>
                <button class="zxs-button" disabled=move ||paging.with(|(_,previous,_,_)|previous.is_none()) on:click=move |_|{if let Some(cursor)=paging.with_untracked(|(_,previous,_,_)|previous.clone()){actions::load_history_merges(state,Some(cursor));}}>{t("较新记录","Newer records")}</button>
                <button class="zxs-button" disabled=move ||paging.with(|(_,_,next,_)|next.is_none()) on:click=move |_|{if let Some(cursor)=paging.with_untracked(|(_,_,next,_)|next.clone()){actions::load_history_merges(state,Some(cursor));}}>{t("较早记录","Older records")}</button>
            </Show>
        </div>
        <Show when=move ||paging.with(|(loaded,_,_,_)|*loaded)&&records.with(Vec::is_empty)><p class="zxs-row">{t("本页没有合并记录。","No merge records on this page.")}</p></Show>
        {move ||records.get().into_iter().map(|row|{let merge_id=row.merge_id.clone();view!{<div class="zxs-frame zxs-stack"><p class="zxs-row">{format!("{}: {chosen} → {result}",t("选定内容及结果","Chosen content and result"),chosen=row.chosen,result=row.result)}</p><p class="zxs-row">{format!("{}: {} · {}",t("保留分支","Surviving branch"),row.survivor,if row.reused{t("复用已有节点；所有来源在此保留，树只显示无环连线。","Reused an existing node; all sources are recorded here and the tree shows only acyclic edges.")}else{t("新建合并节点，正文与标签采用所选内容。","Added a merge node using the chosen content and tags.")})}</p>
            <div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{format!("{} {}",row.input_count,t("个参与输入","inputs"))}</span><button class="zxs-button" on:click=move |_|actions::open_merge_sources_panel(state,merge_id.clone(),0)>{t("查看参与分支及依据节点","Show input branches and base nodes")}</button></div>
        </div>}}).collect_view()}
        {move ||sources.get().map(|(merge_id,page)|{
            let next=page.next_offset;let previous=page.previous_offset();let next_id=merge_id.clone();let previous_id=merge_id.clone();
            view!{<div class="zxs-frame zxs-stack" aria-label=t("合并输入","Merge inputs")>
                <div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{format!("{} {}–{} / {}",t("参与输入","Inputs"),page.offset+1,page.offset+page.inputs.len(),page.total)}</span>
                    <button class="zxs-button" disabled=previous.is_none() on:click=move |_|{if let Some(offset)=previous{actions::open_merge_sources_panel(state,previous_id.clone(),offset);}}>{t("上一页","Previous")}</button>
                    <button class="zxs-button" disabled=next.is_none() on:click=move |_|{if let Some(offset)=next{actions::open_merge_sources_panel(state,next_id.clone(),offset);}}>{t("下一页","Next")}</button>
                    <button class="zxs-button" on:click=move |_|actions::close_panel(state)>{t("关闭","Close")}</button></div>
                {page.inputs.iter().map(|input|{
                    let branch=state.with_untracked(|s|super::super::history_index::branch(s,&input.branch_id).map(|branch|branch.name().to_owned())).unwrap_or_else(||t("历史分支","Historical branch"));
                    let node=input.node_id.clone();
                    view!{<p class="zxs-row">{format!("{} · {branch} ({node})",input.index+1)}</p>}
                }).collect_view()}
            </div>}
        })}
    </section>}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/view_tests.rs"]
mod tests;
