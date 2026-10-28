//! Inline operation review using shared rows, frames and selects.
use super::actions::{initialize_scope, load_sources, load_target, submit, toggle_source, update};
use super::*;
use crate::icons::FileIcon;
use web_interface::components::{Select, SelectOption};
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let active = Memo::new(move |_| state.with(|s| s.files.batch.is_some()));
    let blocked = Signal::derive(move || {
        state.with(|s| {
            !file_actions::write_allowed(s)
                || s.files
                    .batch
                    .as_ref()
                    .is_some_and(|draft| draft.loading || draft.registration.is_some())
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
    view! {<Show when=move ||active.get()>
        <section class="zxs-content-panel zxs-batch-editor zxs-frame zxs-stack" aria-label=t("批量文件操作","Batch file operation")>
            <div class="zxs-file-heading zxs-flow"><h2 class="zxs-row">{move ||state.with(|s|s.files.batch.as_ref().map(|draft|format!("{} · {} {}",draft.kind.label(),draft.entries.len(),t("项","items"))).unwrap_or_default())}</h2><button class="zxs-button" disabled=move ||state.with(file_actions::pending) on:click=move |_|state.update(|s|s.files.batch=None)>{t("取消","Cancel")}</button></div>
            <p class="zxs-row">{t("仅更新选中的目录分支；未选分支和历史版本保持原样。","Only the selected folder branches change; unselected branches and historical versions stay as they are.")}</p>
            <fieldset class="zxs-batch-scopes"><legend class="zxs-row">{t("源目录分支","Source folder branches")}</legend>
                {move ||sources.get().into_iter().map(move |branch|{
                    let id=branch_id(&branch);let checked_id=id.clone();let toggle_id=id;
                    view!{<label class="zxs-batch-check zxs-row"><input type="checkbox" disabled=move ||blocked.get() prop:checked=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.selected_sources.contains(&checked_id))) on:change=move |event|toggle_source(state,toggle_id.clone(),event_target_checked(&event))/><span>{branch_label(&branch)}</span></label>}
                }).collect_view()}
            </fieldset>
            <Show when=move ||sources.get().is_empty()&&!blocked.get()&&state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.source_resolved))><button class="zxs-button" on:click=move |_|initialize_scope(state,false)>{t("登记源目录分支记录","Register source folder branch records")}</button></Show>
            <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|!draft.source_resolved&&!draft.loading))><button class="zxs-button" disabled=move ||blocked.get() on:click=move |_|load_sources(state)>{t("重试读取源目录分支", "Retry loading source folder branches")}</button></Show>
            <div class="zxs-stack">{move ||entries.get().into_iter().map(move |entry|{
                let id=entry_key(&entry);let value_id=StoredValue::new(id.clone());let input_id=StoredValue::new(id);let name=entry.name().to_owned();let dom_id=StoredValue::new(entry_id(&entry));
                view!{<label class="zxs-batch-entry zxs-flow"><span class="zxs-row">{name}</span>
                    <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.kind!=Kind::Delete))>
                        <input class="zxs-input zxs-input--compact zxs-input--underline" type="text" aria-label=t("目标名称","Destination name") data-entry-id=dom_id.get_value() autocomplete="off" spellcheck="false" disabled=move ||blocked.get() prop:value=move ||state.with(|s|s.files.batch.as_ref().and_then(|draft|draft.names.get(&value_id.get_value())).cloned().unwrap_or_default()) on:input=move |event|update(state,|draft|{draft.names.insert(input_id.get_value(),event_target_value(&event));draft.error.clear();})/>
                    </Show>
                </label>}
            }).collect_view()}</div>
            <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.kind.has_destination()))>
                <div class="zxs-file-toolbar zxs-flow"><label class="zxs-batch-destination zxs-flow"><span class="zxs-row">{t("目标目录（目录内相对路径，留空为根目录）","Destination folder (namespace-relative path; empty for root)")}</span><input class="zxs-input zxs-input--compact zxs-input--underline" type="text" aria-label=t("目标目录","Destination folder") disabled=move ||blocked.get() prop:value=move ||state.with(|s|s.files.batch.as_ref().map(|draft|draft.target_path.clone()).unwrap_or_default()) on:input=move |event|update(state,|draft|{draft.target_path=event_target_value(&event);draft.targets.clear();draft.target_resolved=false;for mapping in &mut draft.mappings{mapping.target.clear();}draft.generation+=1;draft.error.clear();}) on:keydown=move |event:web_sys::KeyboardEvent|if event.key()=="Enter"&&!event.is_composing(){event.prevent_default();load_target(state);}/></label><button class="zxs-button" disabled=move ||blocked.get() on:click=move |_|load_target(state)>{t("读取目标分支","Load destination branches")}</button></div>
                <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.target_resolved&&draft.targets.is_empty()))><button class="zxs-button" disabled=move ||blocked.get() on:click=move |_|initialize_scope(state,true)>{t("登记目标目录分支记录","Register destination folder branch records")}</button></Show>
                <div class="zxs-stack">{move ||(0..mapping_count.get()).map(move |index|mapping_view(state,index,blocked)).collect_view()}</div>
                <button class="zxs-button" disabled=move ||blocked.get()||state.with(|s|s.files.batch.as_ref().is_none_or(|draft|draft.selected_sources.is_empty())) on:click=move |_|update(state,|draft|{if let Some(source)=draft.selected_sources.first(){draft.mappings.push(Mapping{source:source.clone(),target:String::new()});}})>{t("添加分支映射","Add branch mapping")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.files.batch.as_ref().is_some_and(|draft|draft.registration.as_ref().is_some_and(|registration|registration.published)))><button class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||file_actions::pending(s)||s.files.batch.as_ref().is_none_or(|draft|draft.loading)) on:click=move |_|reload_registered_scope(state)>{t("重试读取目录分支", "Retry loading folder branches")}</button></Show>
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
        <button class="zxs-button" aria-label=t("移除映射","Remove mapping") disabled=move ||blocked.get() on:click=move |_|update(state,|draft|{if index<draft.mappings.len(){draft.mappings.remove(index);}draft.error.clear();})><FileIcon name="delete"/></button>
    </div>}
}
