use super::*;
use web_interface::components::{Select, SelectOption};

fn cancel(state: &mut State) {
    if !actions::pending(state) {
        state.files.tag_edit = None;
        state.files.publications.remove(&review::Kind::Tags);
    }
}

pub(crate) fn chips(id: String) -> impl IntoView {
    let id = StoredValue::new(id);
    let history = super::super::history_index::shared();
    let labels = Memo::new(move |_| {
        history.with(|history| {
            id.with_value(|id| {
                history
                    .node(id)
                    .map(|(_, node)| tags(node))
                    .unwrap_or_default()
            })
        })
    });
    view! {<span class="zxs-node-tags zxs-flow" aria-label=t("节点标签","Node tags")>{move ||labels.get().into_iter().map(|name|view!{<span class="zxs-node-tag zxs-input zxs-input--compact">{name}</span>}).collect_view()}</span>}
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.tag_edit.is_some()));
    let source_nodes = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| draft.nodes.clone())
                .unwrap_or_default()
        })
    });
    let disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let branch_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| {
                    draft
                        .branches
                        .iter()
                        .map(|branch| SelectOption::new(branch.id(), branch.name()))
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let inherited_options = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| draft.nodes.iter().flat_map(tags).collect::<BTreeSet<_>>())
                .unwrap_or_default()
        })
    });
    view! {<div class="zxs-file-toolbar zxs-flow"><button class="zxs-button" disabled=move ||disabled.get()||state.with(|s|s.files.checked_nodes.is_empty()||s.files.history_loading) on:click=move |_|start(state)>{t("编辑所选节点标签","Edit selected node tags")}</button><span class="zxs-row">{move ||state.with(|s|format!("{} {}",s.files.checked_nodes.len(),t("个节点已选择","nodes selected")))}</span></div>
    <Show when=move ||open.get()><section class="zxs-frame zxs-stack zxs-tag-review" aria-label=t("节点标签编辑","Node tag editor")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("节点标签","Node tags")}</h3><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(cancel)>{t("取消","Cancel")}</button></div>
        <p class="zxs-row">{t("标签编辑在所选节点后新增元数据子节点，继承原标签和正文。只有来源仍是当前节点的引用才推进，历史节点不会使较新内容回退。","Tag edits add metadata child nodes that inherit existing labels and content. Only references still at the source advance; editing history never rolls back newer content.")}</p>
        <div class="zxs-stack">{move ||source_nodes.get().into_iter().enumerate().map(move |(index,node)|{
            let source=StoredValue::new(history_node_id(&node));let selected=Signal::derive(move ||state.with(|s|s.files.tag_edit.as_ref().and_then(|draft|draft.targets.iter().find(|target|target.source==source.get_value())).map(|target|target.branch.clone()).unwrap_or_default()));
            view!{<div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{merge::node_label(&node,index)}</span><Select label=t("关联分支","Associated branch") options=branch_options value=selected disabled=disabled on_change=Callback::new(move |value:String|update(state,|draft|{let head=draft.branches.iter().find(|branch|branch.id()==value).and_then(FileBranch::head).unwrap_or_default().to_owned();if let Some(target)=draft.targets.iter_mut().find(|target|target.source==source.get_value()){target.branch=value;target.expected=head;}}))/></div>}
        }).collect_view()}</div>
        <div class="zxs-file-toolbar zxs-flow"><label class="zxs-flow"><span class="zxs-row">{t("新增标签","Add tags")}</span><input class="zxs-input zxs-input--compact zxs-input--underline" aria-label=t("新增标签","Add tag") data-max-bytes=storage_contract::MAX_TAG_BYTES.to_string() disabled=move ||disabled.get() prop:value=move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|draft.input.clone()).unwrap_or_default()) on:input=move |event|update(state,|draft|draft.input=event_target_value(&event)) on:keydown=move |event:web_sys::KeyboardEvent|{if event.key()=="Enter"&&!event.is_composing(){event.prevent_default();add_input(state);}}/></label><button class="zxs-button" type="button" disabled=move ||disabled.get() on:click=move |_|add_input(state)>{t("添加标签","Add tag")}</button></div>
        <div class="zxs-node-tags zxs-flow">{move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|draft.add.clone()).unwrap_or_default()).into_iter().map(move |name|{let value=name.clone();view!{<button class="zxs-button zxs-tag-choice" aria-label=format!("{} {name}",t("移除新增标签","Remove added tag")) disabled=move ||disabled.get() on:click=move |_|update(state,|draft|{draft.add.remove(&value);})><span class="zxs-node-tag zxs-input zxs-input--compact">{name.clone()}</span></button>}}).collect_view()}</div>
        <fieldset class="zxs-review-choices"><legend class="zxs-row">{t("从所选节点移除标签","Remove tags from selected nodes")}</legend>{move ||inherited_options.get().into_iter().map(move |name|{let pressed=name.clone();let value=name.clone();view!{<button class="zxs-button zxs-tag-choice" type="button" aria-label=format!("{} {name}",t("移除标签","Remove tag")) aria-pressed=move ||state.with(|s|s.files.tag_edit.as_ref().is_some_and(|draft|draft.remove.contains(&pressed))).to_string() disabled=move ||disabled.get() on:click=move |_|update(state,|draft|{if !draft.remove.remove(&value){draft.add.remove(&value);draft.remove.insert(value.clone());}})><span class="zxs-node-tag zxs-input zxs-input--compact">{name.clone()}</span></button>}}).collect_view()}</fieldset>
        <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Tags))}</p><button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|submit(state)>{t("保存标签子节点","Save tag child nodes")}</button>
    </section></Show>}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/view_tests.rs"]
mod tests;
