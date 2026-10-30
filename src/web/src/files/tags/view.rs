use super::*;
use web_interface::components::{Select, SelectOption};

const SELECTED_ROWS: usize = super::super::page::PAGE_SIZE;

fn cancel(state: &mut State, instance: u64) {
    if !actions::pending(state)
        && state
            .files
            .tag_edit
            .as_ref()
            .is_some_and(|draft| draft.instance == instance)
    {
        state.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Tags,
            instance,
        });
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
    view! {
        <span class="zxs-node-tags zxs-flow" aria-label=t("节点标签","Node tags")>
            {move ||labels.get().into_iter().map(|name|view!{<span class="zxs-node-tag zxs-input zxs-input--compact">{name}</span>}).collect_view()}
        </span>
    }
}

fn move_page(state: RwSignal<State>, instance: u64, next: bool, delta: bool) {
    state.update(|s| {
        if let Some(draft) = s
            .files
            .tag_edit
            .as_mut()
            .filter(|draft| draft.instance == instance)
        {
            let (page, count) = if delta {
                (&mut draft.delta_page, draft.add.len() + draft.remove.len())
            } else {
                (&mut draft.target_page, draft.targets.len())
            };
            if next && (*page + 1) * SELECTED_ROWS < count {
                *page += 1;
            } else if !next && *page > 0 {
                *page -= 1;
            }
        }
    });
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let instances =
        Memo::new(move |_| state.with(|s| s.files.tag_edit.as_ref().map(|draft| draft.instance)));
    let sources = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| {
                    draft
                        .targets
                        .keys()
                        .skip(draft.target_page * SELECTED_ROWS)
                        .take(SELECTED_ROWS)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let review_needed = Signal::derive(move || {
        state.with(|s| {
            s.files.tag_edit.as_ref().is_some_and(|draft| {
                !s.files.file_history.ready() || s.files.file_history.root() != Some(draft.root())
            })
        })
    });
    let disabled = Signal::derive(move || {
        state.with(|s| !actions::history_write_allowed(s)) || review_needed.get()
    });
    let navigation_disabled =
        Signal::derive(move || state.with(|s| !actions::history_navigation_ready(s)));
    let inherited_options = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| {
                    draft
                        .targets
                        .keys()
                        .skip(draft.target_page * SELECTED_ROWS)
                        .take(SELECTED_ROWS)
                        .filter_map(|id| s.files.checked_nodes.get(id))
                        .flat_map(|node| node.tags.iter().cloned())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default()
        })
    });
    let changes = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .tag_edit
                .as_ref()
                .map(|draft| {
                    draft
                        .add
                        .iter()
                        .map(|name| (true, name))
                        .chain(draft.remove.iter().map(|name| (false, name)))
                        .skip(draft.delta_page * SELECTED_ROWS)
                        .take(SELECTED_ROWS)
                        .map(|(add, name)| (add, name.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    view! {
        <div class="zxs-file-toolbar zxs-flow">
            <button class="zxs-button" disabled=move ||state.with(|s|!actions::write_allowed(s)||s.files.checked_nodes.is_empty()||!s.files.file_history.ready()) on:click=move |_|start(state)>{t("编辑所选节点标签","Edit selected node tags")}</button>
            <span class="zxs-row">{move ||state.with(|s|format!("{} {}",s.files.checked_nodes.len(),t("个节点已选择","nodes selected")))}</span>
        </div>
        <For each=move ||instances.get().into_iter().collect::<Vec<_>>() key=|instance|*instance children=move |instance|view!{
            <section class="zxs-frame zxs-stack zxs-tag-review" aria-label=t("节点标签编辑","Node tag editor")>
                <div class="zxs-file-toolbar zxs-flow">
                    <h3 class="zxs-row">{t("节点标签","Node tags")}</h3>
                    <button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|state.update(|s|cancel(s,instance))>{t("取消","Cancel")}</button>
                </div>
                <p class="zxs-row">{t("标签编辑新增继承原标签和正文的元数据子节点。换页保留全部所选节点；历史节点不会使较新内容回退。","Tag edits add metadata child nodes inheriting the existing labels and content. Page changes preserve every selected node; historical nodes do not roll back newer content.")}</p>
                <Show when=move ||review_needed.get()>
                    <p class="zxs-row" role="status">{review_required()}</p>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get() on:click=move |_|actions::review_history_tags(state,instance)>{t("读取全部节点及分支以重新审阅","Load every selected node and branch for review")}</button>
                </Show>
                <Show when=move ||state.with(|s|s.files.tag_edit.as_ref().is_some_and(|draft|draft.resolved.is_some()))>
                    <div class="zxs-frame zxs-stack">
                        <p class="zxs-row">{t("全部身份已重新读取。请检查旧标签、新标签及分支依据，再明确接受。","Every selected identity has been read again. Check previous and current labels and branch bases, then explicitly accept the review.")}</p>
                        {move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|{
                            draft.targets.iter().skip(draft.target_page*SELECTED_ROWS).take(SELECTED_ROWS).filter_map(|(id,branch)|{
                                let review=draft.resolved.as_ref()?;
                                let old=s.files.checked_nodes.get(id)?;
                                let new=review.nodes.iter().find(|node|node.id==*id)?;
                                let old_head=branch.as_ref().and_then(|id|draft.branches.get(id)).map(|branch|branch.expected.as_str()).unwrap_or("");
                                let new_head=branch.as_ref().and_then(|id|review.branches.get(id)).map(|branch|branch.expected.as_str()).unwrap_or("");
                                Some(format!("{}: {} → {} · {} → {}",id,old.tags.iter().cloned().collect::<Vec<_>>().join(" / "),new.tags.iter().cloned().collect::<Vec<_>>().join(" / "),old_head,new_head))
                            }).collect::<Vec<_>>()
                        }).unwrap_or_default()).into_iter().map(|line|view!{<p class="zxs-row">{line}</p>}).collect_view()}
                        <button class="zxs-button" disabled=move ||navigation_disabled.get() on:click=move |_|accept_review(state,instance)>{t("接受全部重新审阅的依据","Accept every reviewed base")}</button>
                    </div>
                </Show>
                <div class="zxs-stack">
                    {move ||sources.get().into_iter().map(move |id|{
                        let source=StoredValue::new(id);
                        let selected=Signal::derive(move ||state.with(|s|s.files.tag_edit.as_ref()
                            .and_then(|draft|draft.targets.get(&source.get_value())).cloned().flatten().unwrap_or_default()));
                        let options=Signal::derive(move ||state.with(|s|{
                            let mut options=s.files.file_history.branches().iter().filter(|branch|branch.writable())
                                .map(|branch|SelectOption::new(branch.id(),branch.name())).collect::<Vec<_>>();
                            if let Some(branch)=s.files.tag_edit.as_ref().and_then(|draft|draft.targets.get(&source.get_value())
                                .and_then(|id|id.as_ref()).and_then(|id|draft.branches.get(id))) {
                                if !s.files.file_history.branches().iter().any(|candidate|candidate.id()==branch.id) {
                                    options.push(SelectOption::new(&branch.id,&branch.name));
                                }
                            }
                            options
                        }));
                        view!{<div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{source.get_value()}</span><Select label=t("关联分支","Associated branch") options=options value=selected disabled=disabled on_change=Callback::new(move |value:String|assign_branch(state,instance,source.get_value(),value))/></div>}
                    }).collect_view()}
                </div>
                <div class="zxs-file-toolbar zxs-flow">
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.tag_edit.as_ref().is_none_or(|draft|draft.target_page==0)) on:click=move |_|move_page(state,instance,false,false)>{t("上一页所选节点","Previous selected nodes")}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.tag_edit.as_ref().is_none_or(|draft|(draft.target_page+1)*SELECTED_ROWS>=draft.targets.len())) on:click=move |_|move_page(state,instance,true,false)>{t("下一页所选节点","Next selected nodes")}</button>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.previous_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,false)>{t("上一页分支","Previous branches")}</button>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()||state.with(|s|s.files.file_history.next_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,true)>{t("下一页分支","Next branches")}</button>
                </div>
                <div class="zxs-file-toolbar zxs-flow">
                    <label class="zxs-flow"><span class="zxs-row">{t("新增标签","Add tags")}</span>
                        <input class="zxs-input zxs-input--compact zxs-input--underline" aria-label=t("新增标签","Add tag") maxlength=128 data-max-bytes=storage_contract::MAX_TAG_BYTES.to_string() disabled=disabled prop:value=move ||state.with(|s|s.files.tag_edit.as_ref().map(|draft|draft.input.clone()).unwrap_or_default()) on:input=move |event|edit_input(state,instance,event_target_value(&event)) on:keydown=move |event:web_sys::KeyboardEvent|{if event.key()=="Enter"&&!event.is_composing(){event.prevent_default();add_input(state,instance);}}/>
                    </label>
                    <button class="zxs-button" disabled=disabled on:click=move |_|add_input(state,instance)>{t("添加标签","Add tag")}</button>
                </div>
                <div class="zxs-node-tags zxs-flow">
                    {move ||changes.get().into_iter().map(move |(add,name)|{
                        let value=name.clone();
                        let label=format!("{} {name}",t("撤销标签更改","Undo tag change"));
                        let text=format!("{} {name}",if add{"+"}else{"−"});
                        view!{<button class="zxs-button zxs-tag-choice" aria-label=label disabled=disabled on:click=move |_|update(state,instance,|draft|{if add{draft.add.remove(&value);}else{draft.remove.remove(&value);}})><span class="zxs-node-tag zxs-input zxs-input--compact">{text}</span></button>}
                    }).collect_view()}
                </div>
                <div class="zxs-file-toolbar zxs-flow">
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.tag_edit.as_ref().is_none_or(|draft|draft.delta_page==0)) on:click=move |_|move_page(state,instance,false,true)>{t("上一页标签更改","Previous tag changes")}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|s.files.tag_edit.as_ref().is_none_or(|draft|(draft.delta_page+1)*SELECTED_ROWS>=draft.add.len()+draft.remove.len())) on:click=move |_|move_page(state,instance,true,true)>{t("下一页标签更改","Next tag changes")}</button>
                </div>
                <fieldset class="zxs-review-choices">
                    <legend class="zxs-row">{t("从所选节点移除本页标签","Remove this page's tags from selected nodes")}</legend>
                    {move ||inherited_options.get().into_iter().map(move |name|{
                        let pressed=name.clone();let value=name.clone();
                        let label=format!("{} {name}",t("移除标签","Remove tag"));
                        view!{<button class="zxs-button zxs-tag-choice" aria-label=label aria-pressed=move ||state.with(|s|s.files.tag_edit.as_ref().is_some_and(|draft|draft.remove.contains(&pressed))).to_string() disabled=disabled on:click=move |_|update(state,instance,|draft|{if !draft.remove.remove(&value){draft.add.remove(&value);draft.remove.insert(value.clone());}})><span class="zxs-node-tag zxs-input zxs-input--compact">{name}</span></button>}
                    }).collect_view()}
                </fieldset>
                <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Tags))}</p>
                <button class="zxs-button" disabled=disabled on:click=move |_|submit(state,instance)>{t("保存标签子节点","Save tag child nodes")}</button>
            </section>
        }/>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/view_tests.rs"]
mod tests;
