//! Discovery selects a stable retained file, never invents a live directory row.
use super::{actions, history::DecodeError, model::Navigation, page::Entry};
use crate::{api, ui::{State, accept_read_response}};
use storage_contract::detached_history::{File, Page, Request};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone, Default)]
struct Model {
    epoch: Option<u64>,
    generation: u64,
    loading: bool,
    cancellation: Option<api::ReadCancellation>,
    page: Option<Page>,
    error: Option<api::Error>,
}
impl Model {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() { cancellation.abort(); }
        self.generation += 1;
        self.loading = false;
    }
}
fn endpoint(request: &Request) -> String {
    match (&request.snapshot, &request.after_file) {
        (Some(snapshot), Some(after)) => format!("/api/file-manager/detached-history?snapshot={}&after_file={}", api::segment(&snapshot.to_string()), api::segment(after.as_str())),
        (None, None) => "/api/file-manager/detached-history".into(),
        _ => unreachable!("invariant: discovery controls construct complete continuation pairs"),
    }
}
fn load(state: RwSignal<State>, model: RwSignal<Model>, next: bool) {
    let Some(epoch) = state.with_untracked(|s| s.session_active().then_some(s.session_epoch)) else { return; };
    let request = model.with_untracked(|model| {
        if model.loading { return None; }
        if next {
            let page = model.page.as_ref()?;
            Some(Request { snapshot: Some(page.snapshot), after_file: Some(page.next_after.clone()?) })
        } else { Some(Request::default()) }
    });
    let Some(request) = request else { return; };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => { model.update(|model| model.error = Some(error)); return; }
    };
    let generation = model.try_update(|model| {
        model.cancel(); model.epoch = Some(epoch); model.loading = true; model.error = None;
        model.cancellation = Some(cancellation.clone()); model.generation
    }).expect("invariant: mounted discovery panel owns its state");
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(&request), cancellation).await.and_then(|value| {
            let page: Page = json_codec::from_value(value).map_err(DecodeError::Wire)?;
            page.validate(&request).map_err(DecodeError::Name)?;
            Ok(page)
        });
        if !accept_read_response(state, epoch, &result)
            || model.try_with_untracked(|model| model.generation == generation) != Some(true) { return; }
        model.update(|model| {
            model.loading = false; model.cancellation = None;
            match result {
                Ok(page) => { model.page = Some(page); model.error = None; }
                Err(error) => { model.page = None; model.error = Some(error); }
            }
        });
    });
}
fn open(state: RwSignal<State>, epoch: u64, file: &File) {
    if !state.with_untracked(|s| s.session_active() && s.session_epoch == epoch && !actions::pending(s)) { return; }
    state.update(|s| {
        s.files.navigate(Navigation::Root);
        s.files.selected = Some(Entry::retained(file));
        s.change_page(crate::ui::Page::Details);
    });
    // Obtain a fresh exact history root. The discovery root is not an old-root
    // lease, a current namespace context, or permission to submit retirement.
    actions::load_file_history(state);
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    on_cleanup(move || { let _ = model.try_update(Model::cancel); });
    Effect::new(move |_| {
        let epoch = state.with(|s|s.session_active().then_some(s.session_epoch));
        if model.with_untracked(|model|model.epoch != epoch) {
            model.update(|model| { model.cancel(); model.epoch = epoch; model.page = None; model.error = None; });
        }
    });
    view! {
        <details class="zxs-entry-section zxs-stack" data-zxs-detached-history="true">
            <summary class="zxs-row">{t("查找未在当前目录出现的保留文件", "Find retained files absent from current directories")}</summary>
            <p class="zxs-row">{t("按稳定文件身份发现当前仓库历史中仍有可用/已清理正文版本、但不在任何当前主目录且没有 D 的文件。普通软删除仍从删除历史撤销，不会列入此处。此列表不是永久删除许可：其他分支、目录/Git、工作区和租约仍可能阻止退役。", "Find stable files with available or purged versions in current repository histories, absent from every current main namespace and without a D node. Ordinary soft deletion is still undone from deletion history and is not listed here. Discovery is not retirement permission; other branches, directory/Git references, workspaces and leases may still block it.")}</p>
            <div class="zxs-flow">
                <button class="zxs-button" disabled=move ||!state.with(State::session_active)||model.with(|model|model.loading) on:click=move |_|load(state,model,false)>{t("读取 / 从第一页重新开始", "Load / restart from the first page")}</button>
                <button class="zxs-button" disabled=move ||!state.with(State::session_active)||model.with(|model|model.loading||model.page.as_ref().is_none_or(|page|page.next_after.is_none())) on:click=move |_|load(state,model,true)>{t("下一页保留文件", "Next retained files")}</button>
                <button class="zxs-button" disabled=move ||!model.with(|model|model.loading) on:click=move |_|model.update(Model::cancel)>{t("取消本次读取", "Cancel this read")}</button>
            </div>
            <Show when=move ||model.with(|model|model.loading)><p class="zxs-row" role="status">{t("正在读取准确当前目录与保留历史…", "Reading current namespaces and retained history…")}</p></Show>
            <For each=move ||{let epoch=state.with(|s|s.session_active().then_some(s.session_epoch));model.with(|model|model.epoch.filter(|saved|Some(*saved)==epoch).and_then(|saved|model.page.as_ref().map(|page|page.files.iter().cloned().map(|file|(saved,file)).collect::<Vec<_>>())).unwrap_or_default())} key=|row|(row.0,row.1.file_id.clone()) children=move |(epoch,file)| {
                let label = format!("{} · {} · {} {}", file.recorded_name, file.file_id, file.retained_versions, t("个保留正文版本", "retained body versions"));
                let file = StoredValue::new(file);
                view! { <button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|file.with_value(|file|open(state,epoch,file))>{label}</button> }
            }/>
            <Show when=move ||model.with(|model|!model.loading&&model.page.as_ref().is_some_and(|page|page.files.is_empty()))><p class="zxs-row">{t("此页没有符合条件的保留文件。", "No retained files match this page.")}</p></Show>
            {move ||model.with(|model|model.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
        </details>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/detached_history_tests.rs"]
mod tests;
