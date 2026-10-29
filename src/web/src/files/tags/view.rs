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
                    .map(|(_, node)| node.tags.clone())
                    .unwrap_or_default()
            })
        })
    });
    view! {<span class="zxs-node-tags zxs-flow" aria-label=t("节点标签","Node tags")>{move ||labels.get().into_iter().map(|name|view!{<span class="zxs-node-tag zxs-input zxs-input--compact">{name}</span>}).collect_view()}</span>}
}

/// Targets of the current local page of the selected list.
fn visible_targets(draft: &Draft) -> Vec<Target> {
    draft
        .targets
        .iter()
        .skip(draft.offset)
        .take(SELECTED_PAGE)
        .cloned()
        .collect()
}

fn removal_options(draft: &Draft) -> BTreeSet<String> {
    draft
        .targets
        .iter()
        .skip(draft.offset)
        .take(SELECTED_PAGE)
        .flat_map(|target| target.tags.iter().cloned())
        .chain(draft.remove.iter().cloned())
        .collect()
}

fn branch_options(model: &Model, source: &str) -> Vec<SelectOption> {
    let mut options = model
        .history
        .branch_page()
        .map(|page| {
            page.writable()
                .filter(|branch| branch.head().is_some())
                .map(|branch| SelectOption::new(branch.id(), branch.name()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(draft) = model.tag_edit.as_ref()
        && let Some(target) = draft.targets.iter().find(|target| target.source == source)
        && let Some(branch) = draft
            .branches
            .iter()
            .find(|branch| branch.id == target.branch)
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
    let open = Memo::new(move |_| state.with(|s| s.files.tag_edit.is_some()));
    let page = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| (draft.offset, draft.targets.len(), visible_targets(draft)))
                .unwrap_or_default()
        })
    });
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .tag_edit
                    .as_ref()
                    .is_some_and(|draft| s.files.history.root() != Some(&draft.root))
        })
    });
    let inherited_options = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(removal_options)
                .unwrap_or_default()
        })
    });
    let selected_count = Memo::new(move |_| state.with(|s| s.files.checked_nodes.len()));
    let start_disabled = Signal::derive(move || {
        disabled.get()
            || state.with(|s| {
                s.files.checked_nodes.is_empty()
                    || s.files.history.loading()
                    || s.files.history.root().is_none()
            })
    });
    view! {<div class="zxs-file-toolbar zxs-flow"><button class="zxs-button" disabled=move ||start_disabled.get() on:click=move |_|start(state)>{t("编辑所选节点标签","Edit selected node tags")}</button><span class="zxs-row">{move ||format!("{} {}",selected_count.get(),t("个节点已选择","nodes selected"))}</span><button class="zxs-button" disabled=move ||state.with(|s|s.files.tag_edit.is_some()||actions::pending(s)||s.files.checked_nodes.is_empty()) on:click=move |_|state.update(|s|s.files.clear_checked_nodes())>{t("清除节点选择", "Clear selected nodes")}</button></div>
    <Show when=move ||open.get()><section class="zxs-frame zxs-stack zxs-tag-review" aria-label=t("节点标签编辑","Node tag editor")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("节点标签","Node tags")}</h3><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(cancel)>{t("取消","Cancel")}</button></div>
        <p class="zxs-row">{t("标签编辑在所选节点后新增元数据子节点，继承原标签和正文。只有来源仍是当前节点的引用才推进，历史节点不会使较新内容回退。","Tag edits add metadata child nodes that inherit existing labels and content. Only references still at the source advance; editing history never rolls back newer content.")}</p>
        <Show when=move ||state.with(|s|s.files.tag_edit.as_ref().is_some_and(|draft|s.files.history.root()!=Some(&draft.root)))><p class="zxs-row zxs-inline-error" role="status">{t("历史已变化：此草稿保留为待重审输入，请取消、清除旧选择并重新选择节点。","The history changed: this draft is kept as input awaiting review. Cancel, clear the old selection, and select the nodes again.")}</p></Show>
        <div class="zxs-stack">{move ||{let (offset,_,targets)=page.get();targets.into_iter().enumerate().map(move |(index,target)|{
            let source=StoredValue::new(target.source.clone());let selected=Signal::derive(move ||state.with(|s|s.files.tag_edit.as_ref().and_then(|draft|draft.targets.iter().find(|target|target.source==source.get_value())).map(|target|target.branch.clone()).unwrap_or_default()));
            let options=Signal::derive(move ||state.with(|s|branch_options(&s.files,&source.get_value())));
            view!{<div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{format!("{} · {}",offset+index+1,target.label)}</span><Select label=t("关联分支","Associated branch") options=options value=selected disabled=disabled on_change=Callback::new(move |value:String|select_branch(state,&source.get_value(),&value))/></div>}
        }).collect_view()}}</div>
        {super::super::branch::paging(state)}
        <Show when=move ||page.with(|(_,total,_)|*total>SELECTED_PAGE)><div class="zxs-file-toolbar zxs-flow">
            <button class="zxs-button" disabled=move ||page.with(|(offset,_,_)|*offset==0) on:click=move |_|update(state,|draft|draft.offset=draft.offset.saturating_sub(SELECTED_PAGE))>{t("上一页所选","Previous selected")}</button>
            <span class="zxs-row">{move ||{let (offset,total,targets)=page.get();format!("{}–{} / {total}",offset+1,offset+targets.len())}}</span>
            <button class="zxs-button" disabled=move ||page.with(|(offset,total,_)|offset+SELECTED_PAGE>=*total) on:click=move |_|update(state,|draft|{if draft.offset+SELECTED_PAGE<draft.targets.len(){draft.offset+=SELECTED_PAGE;}})>{t("下一页所选","Next selected")}</button>
        </div></Show>
        <div class="zxs-file-toolbar zxs-flow"><label class="zxs-flow"><span class="zxs-row">{t("新增标签","Add tags")}</span><input class="zxs-input zxs-input--compact zxs-input--underline" aria-label=t("新增标签","Add tag") data-max-bytes=storage_contract::MAX_TAG_BYTES.to_string() disabled=move ||disabled.get() prop:value=move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|draft.input.clone()).unwrap_or_default()) on:input=move |event|update(state,|draft|draft.input=event_target_value(&event)) on:keydown=move |event:web_sys::KeyboardEvent|{if event.key()=="Enter"&&!event.is_composing(){event.prevent_default();add_input(state);}}/></label><button class="zxs-button" type="button" disabled=move ||disabled.get() on:click=move |_|add_input(state)>{t("添加标签","Add tag")}</button></div>
        <div class="zxs-node-tags zxs-flow">{move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|draft.add.clone()).unwrap_or_default()).into_iter().map(move |name|{let value=name.clone();view!{<button class="zxs-button zxs-tag-choice" aria-label=format!("{} {name}",t("移除新增标签","Remove added tag")) disabled=move ||disabled.get() on:click=move |_|update(state,|draft|{draft.add.remove(&value);})><span class="zxs-node-tag zxs-input zxs-input--compact">{name.clone()}</span></button>}}).collect_view()}</div>
        <fieldset class="zxs-review-choices"><legend class="zxs-row">{t("从所选节点移除标签（当前所选页及已勾选标签）","Remove tags (current selected page and checked tags)")}</legend>{move ||inherited_options.get().into_iter().map(move |name|{let pressed=name.clone();let value=name.clone();view!{<button class="zxs-button zxs-tag-choice" type="button" aria-label=format!("{} {name}",t("移除标签","Remove tag")) aria-pressed=move ||state.with(|s|s.files.tag_edit.as_ref().is_some_and(|draft|draft.remove.contains(&pressed))).to_string() disabled=move ||disabled.get() on:click=move |_|remove_tag(state,&value)><span class="zxs-node-tag zxs-input zxs-input--compact">{name.clone()}</span></button>}}).collect_view()}</fieldset>
        <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Tags))}</p><button class="zxs-button" disabled=move ||disabled.get()||state.with(|s|s.files.history.root().is_none()) on:click=move |_|submit(state)>{t("保存标签子节点","Save tag child nodes")}</button>
    </section></Show>}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/view_tests.rs"]
mod tests;
