//! Container policy is shared mutable metadata, never an implicit history commit.
use crate::{api, ui::{State, accept_read_response}};
use super::{actions, model, review};
use storage_contract::container_edit::{CommitMode, ContainerEdit};
use storage_contract::file_container_tags::{Context, ReadRequest};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    entry: String,
    request: ReadRequest,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() { return None; }
        let entry = state.files.selected.as_ref()?;
        if entry.kind() != storage_contract::NamespaceEntryKind::File { return None; }
        let (repository, path) = entry.context()?;
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            entry: entry.entry_id().to_owned(),
            request: ReadRequest {
                file_id: entry.file_id().parse().ok()?,
                context: Context {
                    repository_id: repository.parse().ok()?,
                    path: name_policy::PortablePath::parse(path).ok()?,
                },
            },
        })
    }
    fn route(&self) -> String {
        format!("/api/file-manager/files/{}/edit-mode", self.request.file_id)
    }
    fn read_route(&self) -> String {
        format!("{}?repository={}&path={}", self.route(),
            api::segment(&self.request.context.repository_id.to_string()),
            api::segment(self.request.context.path.as_str()))
    }
}

#[derive(Clone, Default)]
struct Panel {
    target: Option<Target>,
    generation: u64,
    snapshot: Option<ContainerEdit>,
    loading: bool,
    error: String,
    cancellation: Option<api::ReadCancellation>,
}
impl Panel {
    fn cancel(&mut self) {
        if let Some(cancel) = self.cancellation.take() { cancel.abort(); }
        self.generation += 1;
    }
    fn accepts(&self, target: &Target, generation: u64) -> bool {
        self.target.as_ref() == Some(target) && self.generation == generation
    }
}
fn decode(value: json_codec::Value) -> Result<ContainerEdit, api::Error> {
    json_codec::from_value(value).map_err(|source| api::Error::invalid_response(
        "Invalid container edit state.", api::ErrorSource::ContainerEdit(source)))
}
fn can_switch(edit: &ContainerEdit) -> bool { edit.draft().is_none() }
fn status(edit: &ContainerEdit) -> String {
    match edit {
        ContainerEdit::Automatic { .. } => t(
            "自动：文件更改后生成版本（默认）。", "Automatic: file changes create versions (default)."),
        ContainerEdit::ManualClean { .. } => t(
            "手动：文件更改保留为共享稿，仅显式提交生成版本。",
            "Manual: file changes stay in a shared draft; only explicit commit creates a version."),
        ContainerEdit::ManualDirty { .. } => t(
            "手动：有未提交的共享稿。请先显式提交，再切回自动；更改此设置不会提交文件。",
            "Manual: a shared draft is uncommitted. Explicitly commit it before switching to automatic; changing this setting does not commit the file."),
    }
}
fn reload(state: RwSignal<State>, panel: RwSignal<Panel>, target: Option<Target>) {
    let cancellation = target.as_ref().map(|_| api::ReadCancellation::new()).transpose();
    let generation = panel.try_update(|p| {
        p.cancel();
        p.target = target.clone();
        p.snapshot = None;
        p.error.clear();
        p.loading = target.is_some();
        p.generation
    });
    let (Some(target), Some(generation)) = (target, generation) else { return; };
    let cancellation = match cancellation {
        Ok(Some(value)) => value,
        Ok(None) => return,
        Err(error) => {
            let _ = panel.try_update(|p| { p.loading = false; p.error = error.message; });
            return;
        }
    };
    let _ = panel.try_update(|p| p.cancellation = Some(cancellation.clone()));
    spawn_local(async move {
        let result = api::cancellable_get(&target.read_route(), cancellation).await.and_then(decode);
        if !accept_read_response(state, target.epoch, &result)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&target)
        { return; }
        let _ = panel.try_update(|p| {
            if !p.accepts(&target, generation) { return; }
            p.loading = false;
            p.cancellation = None;
            match result {
                Ok(edit) => p.snapshot = Some(edit),
                Err(error) => p.error = error.message,
            }
        });
    });
}
fn submit(state: RwSignal<State>, panel: RwSignal<Panel>) {
    let command = state.with_untracked(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files) {
            return None;
        }
        let target = Target::capture(s)?;
        panel.with_untracked(|p| {
            if p.loading || p.target.as_ref() != Some(&target) { return None; }
            let edit = p.snapshot.as_ref()?;
            if !can_switch(edit) { return None; }
            let mode = match edit.mode() {
                CommitMode::Automatic => CommitMode::Manual,
                CommitMode::Manual => CommitMode::Automatic,
            };
            Some((target, edit.generation(), mode))
        })
    });
    if let Some((target, generation, mode)) = command {
        review::submit(state, review::Kind::ContainerMode, "POST", target.route(),
            json_codec::json!({"repository":target.request.context.repository_id,
                "path":target.request.context.path,"expected_generation":generation,"mode":mode}));
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let panel = RwSignal::new(Panel::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    let pending = Memo::new(move |_| state.with(|s| s.files.publications
        .get(&review::Kind::ContainerMode).is_some_and(|p| p.operation_id.is_some())));
    Effect::new(move |_| {
        let target = target.get();
        if !pending.get() { reload(state, panel, target); }
    });
    on_cleanup(move || { let _ = panel.try_update(Panel::cancel); });
    view! {
        <section class="zxs-entry-section zxs-stack" aria-label=t("版本提交模式", "Version commit mode")>
            <h3>{t("版本提交模式", "Version commit mode")}</h3>
            <p role="status">{move || panel.with(|p| {
                if p.loading { t("正在读取…", "Loading…") }
                else if !p.error.is_empty() { p.error.clone() }
                else { p.snapshot.as_ref().map(status).unwrap_or_default() }
            })}</p>
            <p role="status">{move || state.with(|s| review::error_text(s, review::Kind::ContainerMode))}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move || pending.get()
                    on:click=move |_| reload(state, panel, state.with_untracked(Target::capture))>
                    {t("重新读取设置", "Reload setting")}
                </button>
                <button class="zxs-button" disabled=move || pending.get()
                    || state.with(|s| !actions::write_allowed(s) || model::selected_entry_read_only(&s.files))
                    || panel.with(|p| p.loading || p.snapshot.as_ref().is_none_or(|e| !can_switch(e)))
                    on:click=move |_| submit(state, panel)>
                    {move || panel.with(|p| if p.snapshot.as_ref().is_some_and(|e| e.mode() == CommitMode::Manual) {
                        t("启用自动版本提交", "Enable automatic version commits")
                    } else { t("改为仅手动提交版本", "Use manual version commits only") })}
                </button>
            </div>
        </section>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/container_edit.rs"]
mod tests;
