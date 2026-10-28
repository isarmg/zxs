use super::*;
use web_interface::components::{Select, SelectOption};

fn cancel(state: &mut State) {
    if !actions::pending(state) {
        state.files.merge = None;
        state.files.publications.remove(&review::Kind::Merge);
    }
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.merge.is_some()));
    let branches = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| draft.branches.clone())
                .unwrap_or_default()
        })
    });
    let disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let result_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| {
                    draft
                        .nodes
                        .iter()
                        .enumerate()
                        .map(|(index, node)| {
                            SelectOption::new(history_node_id(node), node_label(node, index))
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let survivor_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .map(|draft| {
                    draft
                        .branches
                        .iter()
                        .filter(|branch| draft.selected.contains(branch.id()))
                        .map(|branch| SelectOption::new(branch.id(), branch.name()))
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
                .map(|draft| draft.chosen.clone())
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
    view! {<Show when=move ||open.get()><section class="zxs-frame zxs-stack zxs-merge-review" aria-label=t("合并文件分支","Merge file branches")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("合并文件分支","Merge file branches")}</h3><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(cancel)>{t("取消","Cancel")}</button></div>
        <p class="zxs-row">{t("保留分支采用结果节点，其余参与分支保留原节点并变为只读别名；未选择的可写文件分支保持不变。所有当前命名目录分支中引用参与分支的对应条目（含当前未显示的目录分支）将更新到结果节点，其他文件引用不变。","The surviving branch adopts the result node. Other selected branches keep their original nodes as read-only aliases; unselected writable file branches stay unchanged. Matching references in all current named folder branches, including branches not currently displayed, advance to the result node; other file references stay unchanged.")}</p>
        <fieldset class="zxs-review-choices"><legend class="zxs-row">{t("参与分支","Input branches")}</legend>{move ||branches.get().into_iter().map(move |branch|{let id=branch.id().to_owned();let checked_id=id.clone();view!{<label class="zxs-batch-check zxs-row"><input type="checkbox" disabled=move ||disabled.get() prop:checked=move ||state.with(|s|s.files.merge.as_ref().is_some_and(|draft|draft.selected.contains(&checked_id))) on:change=move |event|{let checked=event_target_checked(&event);update(state,|draft|{if checked{draft.selected.insert(id.clone());}else{draft.selected.remove(&id);if draft.survivor==id{draft.survivor.clear();}}});}/><span>{branch.name().to_owned()}</span></label>}}).collect_view()}</fieldset>
        <div class="zxs-file-toolbar zxs-flow"><Select label=t("结果节点（可选此文件任一节点）","Result node (any node of this file)") options=result_options value=result disabled=disabled on_change=Callback::new(move |value:String|update(state,|draft|draft.chosen=value))/><Select label=t("合并后保留的分支","Surviving branch") options=survivor_options value=survivor disabled=disabled on_change=Callback::new(move |value:String|update(state,|draft|draft.survivor=value))/></div>
        <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Merge))}</p>
        <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|submit(state)>{t("确认合并所选分支","Confirm merge of selected branches")}</button>
    </section></Show>}
}

/// The acyclic tree may deliberately omit an edge when reusing an ancestor.
/// Preserve all inputs in a separate provenance list instead of inventing
/// another content node or hiding the source branch history.
pub(crate) fn history() -> impl IntoView {
    let history = super::super::history_index::shared();
    let records = Memo::new(move |_| {
        history.with(|history| {
            history.snapshot["merges"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|record| {
                    let branch_name = |id: &str| {
                        history
                            .branch(id)
                            .map(|branch| branch.name().to_owned())
                            .unwrap_or_else(|| t("历史分支", "Historical branch"))
                    };
                    let node_name = |id: &str| {
                        history
                            .node(id)
                            .map(|(index, node)| node_label(node, index))
                            .unwrap_or_else(|| t("历史节点", "Historical node"))
                    };
                    let inputs = record["input_branch_ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(index, id)| {
                            let node = record["input_node_ids"][index].as_str().unwrap_or_default();
                            format!(
                                "{} ({})",
                                branch_name(id.as_str().unwrap_or_default()),
                                node_name(node)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" / ");
                    let survivor =
                        branch_name(record["surviving_branch_id"].as_str().unwrap_or_default());
                    let chosen = node_name(record["chosen_node_id"].as_str().unwrap_or_default());
                    let result = node_name(record["result_node_id"].as_str().unwrap_or_default());
                    (
                        inputs,
                        survivor,
                        chosen,
                        result,
                        record["reused_node"] == true,
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    view! {<Show when=move ||!records.get().is_empty()><section class="zxs-merge-history zxs-stack" aria-label=t("合并来源记录","Merge provenance")><h3 class="zxs-row">{t("合并来源记录","Merge provenance")}</h3>{move ||records.get().into_iter().map(|(inputs,survivor,chosen,result,reused)|view!{<div class="zxs-frame zxs-stack"><p class="zxs-row">{format!("{}: {inputs}",t("参与分支及依据节点","Input branches and base nodes"))}</p><p class="zxs-row">{format!("{}: {chosen} → {result}",t("选定内容及结果","Chosen content and result"))}</p><p class="zxs-row">{format!("{}: {survivor} · {}",t("保留分支","Surviving branch"),if reused{t("复用已有节点；所有来源在此保留，树只显示无环连线。","Reused an existing node; all sources are recorded here and the tree shows only acyclic edges.")}else{t("新建合并节点，正文与标签采用所选内容。","Added a merge node using the chosen content and tags.")})}</p></div>}).collect_view()}</section></Show>}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/view_tests.rs"]
mod tests;
