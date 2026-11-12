//! Inline operation review using shared rows, frames and selects.
use super::actions::{load_sources, load_target, submit, toggle_source, update};
use super::*;
use crate::icons::FileIcon;
use web_interface::components::{Select, SelectOption};
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let active = Memo::new(move |_| state.with(|s| s.files.batch.is_some()));
    let separation = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .is_some_and(|draft| draft.kind == Kind::Separate)
        })
    });
    let blocked = Signal::derive(move || {
        state.with(|s| {
            !file_actions::write_allowed(s)
                || s.files.batch.as_ref().is_some_and(|draft| {
                    draft.loading
                        || draft.submitted_operation.is_some()
                })
        })
    });
    let sources = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .map(|draft| draft.sources.clone())
                .unwrap_or_default()
        })
    });
    let entries = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .map(|draft| draft.entries.clone())
                .unwrap_or_default()
        })
    });
    let mapping_count = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .map(|draft| draft.mappings.len())
                .unwrap_or_default()
        })
    });
    let confirmation = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .and_then(|draft| draft.name_error.clone())
                .filter(|failure| failure.entry.is_some() && failure.error.candidate().is_some())
        })
    });
    view! {<Show when=move ||active.get()>
        <section class="zxs-content-panel zxs-batch-editor zxs-frame zxs-stack" aria-label=t("批量文件操作","Batch file operation")>
            <div class="zxs-file-heading zxs-flow"><h2 class="zxs-row">{move ||state.with(|s|s.files.batch.as_ref().map(|draft|format!("{} · {} {}",draft.kind.label(),draft.entries.len(),t("项","items"))).unwrap_or_default())}</h2><button class="zxs-button" disabled=move ||state.with(file_actions::pending) on:click=move |_|state.update(|s|s.files.batch=None)>{t("取消","Cancel")}</button></div>
            <p class="zxs-row">{move ||if separation.get(){t("所选文件是保留身份和历史的原件。请指定同一文件的另一个已有位置：目标保留自己的已保存内容，获得独立身份，不继承旧历史。两边必须是各自稳定的 main 工作分支，服务端会验证身份和最新工作树。","The selected file retains its identity and history. Choose another existing occurrence of the same file: it keeps its own saved content and gets an independent identity without old history. Both must be their stable main workspaces; the server verifies identities and current trees.")}else{t("仅更新选中的目录分支；未选分支和历史版本保持原样。","Only the selected folder branches change; unselected branches and historical versions stay as they are.")}}</p>
            <fieldset class="zxs-batch-scopes"><legend class="zxs-row">{move ||if separation.get(){t("保留原件的目录分支","Retained original folder branch")}else{t("源目录分支","Source folder branches")}}</legend>
                {move ||sources.get().into_iter().map(move |branch|{
                    let id=branch_id(&branch);let checked_id=id.clone();let toggle_id=id;
                    view!{<label class="zxs-batch-check zxs-row"><input type="checkbox" disabled=move ||blocked.get() prop:checked=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.selected_sources.contains(&checked_id))) on:change=move |event|toggle_source(state,toggle_id.clone(),event_target_checked(&event))/><span>{branch_label(&branch)}</span></label>}
                }).collect_view()}
            </fieldset>
            <Show when=move ||sources.get().is_empty()&&!separation.get()&&!blocked.get()
                &&state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.source_resolved))>
                <p class="zxs-row">
                    {t("源目录没有可用的目录分支，无法执行本次批次。",
                        "The source has no available folder branch for this batch.")}
                </p>
            </Show>
            <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|!draft.source_resolved&&!draft.loading))><button class="zxs-button" disabled=move ||blocked.get() on:click=move |_|load_sources(state)>{t("重试读取源目录分支", "Retry loading source folder branches")}</button></Show>
            <div class="zxs-stack">{move ||entries.get().into_iter().map(move |entry|{
                let id=entry_key(&entry);let value_id=StoredValue::new(id.clone());let input_id=StoredValue::new(id);let name=if separation.get(){format!("{}: {}",t("保留原件","Retained original"),selected_path(&entry))}else{entry.name().to_owned()};let dom_id=StoredValue::new(entry_id(&entry));
                view!{<label class="zxs-batch-entry zxs-form-field"><span class="zxs-row">{name}</span>
                    <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.kind!=Kind::Delete))>
                        <Show when=move ||separation.get()><span class="zxs-row">{t("要分离的已有文件名","Existing file name to separate")}</span></Show><input class="zxs-input zxs-input--compact" type="text" aria-label=move ||if separation.get(){t("要分离的已有文件名","Existing file name to separate")}else{t("目标名称","Destination name")} data-entry-id=dom_id.get_value() autocomplete="off" spellcheck="false" disabled=move ||blocked.get() prop:value=move ||state.with(|s|s.files.batch.as_ref().and_then(|draft|draft.names.get(&value_id.get_value())).cloned().unwrap_or_default()) on:input=move |event|update(state,|draft|{draft.names.insert(input_id.get_value(),event_target_value(&event));draft.error.clear();})/>
                    </Show>
                </label>}
            }).collect_view()}</div>
            <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.kind.has_destination()))>
                <div class="zxs-file-toolbar zxs-flow"><label class="zxs-batch-destination zxs-form-field"><span class="zxs-row">{t("目标目录（目录内相对路径，留空为根目录）","Destination folder (namespace-relative path; empty for root)")}</span><input class="zxs-input zxs-input--compact" type="text" aria-label=t("目标目录","Destination folder") disabled=move ||blocked.get() prop:value=move ||state.with(|s|s.files.batch.as_ref().map(|draft|draft.target_path.clone()).unwrap_or_default()) on:input=move |event|update(state,|draft|{draft.target_path=event_target_value(&event);draft.targets.clear();draft.target_resolved=false;for mapping in &mut draft.mappings{mapping.target.clear();}draft.generation+=1;draft.error.clear();}) on:keydown=move |event:web_sys::KeyboardEvent|if event.key()=="Enter"&&!event.is_composing(){event.prevent_default();load_target(state);}/></label><button class="zxs-button" disabled=move ||blocked.get() on:click=move |_|load_target(state)>{t("读取目标分支","Load destination branches")}</button></div>
                <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft| {
                    draft.kind!=Kind::Separate&&draft.target_resolved&&draft.targets.is_empty()
                }))>
                    <p class="zxs-row">
                        {t("目标没有可用的目录分支，无法执行本次批次。",
                            "The destination has no available folder branch for this batch.")}
                    </p>
                </Show>
                <div class="zxs-stack">{move ||(0..mapping_count.get()).map(move |index|mapping_view(state,index,blocked)).collect_view()}</div>
                <Show when=move ||!separation.get()><button class="zxs-button" disabled=move ||blocked.get()||state.with(|s|s.files.batch.as_ref().is_none_or(|draft|draft.selected_sources.is_empty())) on:click=move |_|update(state,|draft|{if let Some(source)=draft.selected_sources.first(){draft.mappings.push(Mapping{source:source.clone(),target:String::new()});}})>{t("添加分支映射","Add branch mapping")}</button></Show>
            </Show>
            {move ||confirmation.get().map(|expected| view! {
                <button class="zxs-button" type="button" disabled=move ||blocked.get()
                    on:click=move |_|super::actions::confirm_name(state, &expected)>
                    {t("确认使用 NFC 名称", "Confirm NFC name")}
                </button>
            })}
            <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|s.files.batch.as_ref().map(|draft|draft.error.clone()).unwrap_or_default())}</p>
            <div class="zxs-file-toolbar zxs-flow"><button class="zxs-button" disabled=move ||blocked.get()||state.with(|s|s.files.batch.as_ref().is_none_or(|draft|!draft.source_resolved)) on:click=move |_|submit(state)>{t("执行所选操作","Apply selected operation")}</button></div>
        </section>
    </Show>}
}
fn mapping_view(state: RwSignal<State>, index: usize, blocked: Signal<bool>) -> impl IntoView {
    let source_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .map(|draft| {
                    draft
                        .sources
                        .iter()
                        .filter(|branch| draft.selected_sources.contains(&branch_id(branch)))
                        .map(|branch| SelectOption::new(branch_id(branch), branch_label(branch)))
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let target_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .map(|draft| {
                    draft
                        .targets
                        .iter()
                        .map(|branch| SelectOption::new(branch_id(branch), branch_label(branch)))
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let source = Signal::derive(move || {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .and_then(|draft| draft.mappings.get(index))
                .map(|mapping| mapping.source.clone())
                .unwrap_or_default()
        })
    });
    let target = Signal::derive(move || {
        state.with(|s| {
            s.files
                .batch
                .as_ref()
                .and_then(|draft| draft.mappings.get(index))
                .map(|mapping| mapping.target.clone())
                .unwrap_or_default()
        })
    });
    view! {<div class="zxs-file-toolbar zxs-batch-mapping zxs-flow">
        <Select label=t("映射源分支","Mapping source branch") options=source_options value=source disabled=blocked on_change=Callback::new(move |value:String|update(state,|draft|{if let Some(mapping)=draft.mappings.get_mut(index){mapping.source=value;}draft.error.clear();}))/>
        <span class="zxs-row" aria-hidden="true">"→"</span>
        <Select label=t("映射目标分支","Mapping destination branch") options=target_options value=target disabled=blocked on_change=Callback::new(move |value:String|update(state,|draft|{if let Some(mapping)=draft.mappings.get_mut(index){mapping.target=value;}draft.error.clear();}))/>
        <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.kind!=Kind::Separate))><button class="zxs-button" aria-label=t("移除映射","Remove mapping") disabled=move ||blocked.get() on:click=move |_|update(state,|draft|{if index<draft.mappings.len(){draft.mappings.remove(index);}draft.error.clear();})><FileIcon name="delete"/></button></Show>
    </div>}
}
