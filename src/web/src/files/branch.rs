use super::actions;
use crate::ui::State;
use web_interface::{html, i18n::t, prelude::*};

#[derive(Clone)]
pub(super) enum Target {
    CurrentDirectory,
    Directory { path: String, select: bool },
    CurrentFileVersion,
    HistoryNode(String),
}

impl Target {
    fn scope(&self) -> super::inline::EditScope {
        match self {
            Self::CurrentDirectory | Self::Directory { .. } => super::inline::EditScope::Location,
            Self::CurrentFileVersion | Self::HistoryNode(_) => super::inline::EditScope::History,
        }
    }

    fn submit(self, state: RwSignal<State>, name: String) -> Option<String> {
        match self {
            Self::CurrentDirectory => {
                let path = state.with_untracked(|s| s.files.directory.clone());
                actions::directory_branch(state, path, name, false)
            }
            Self::Directory { path, select } => {
                actions::directory_branch(state, path, name, select)
            }
            Self::CurrentFileVersion | Self::HistoryNode(_) => {
                let (version, node) = state.with_untracked(|s| file_source(&s.files, &self))?;
                actions::create_branch(state, version, node, name)
            }
        }
    }
}

fn file_source(model: &super::model::Model, target: &Target) -> Option<(String, Option<String>)> {
    match target {
        Target::CurrentFileVersion => {
            if !model.file_history.ready() {
                return None;
            }
            Some((
                model.file_history.current_version_id()?.to_owned(),
                model.file_history.current_node_id().map(str::to_owned),
            ))
        }
        Target::HistoryNode(id) => {
            if !model.file_history.ready() {
                return None;
            }
            let record = model.file_history.node(id)?;
            Some((
                record.version_id()?.to_owned(),
                Some(record.id().to_owned()),
            ))
        }
        Target::CurrentDirectory | Target::Directory { .. } => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Name {
    Empty,
    Invalid,
    Valid(String),
}

fn branch_name(value: &str) -> Name {
    let controls = value.chars().any(char::is_control);
    let value = value.trim();
    if value.is_empty() {
        Name::Empty
    } else if !controls
        && value.len() <= super::inline::MAX_TEXT_BYTES
        && name_policy::validate_name(value).is_ok()
    {
        Name::Valid(value.to_owned())
    } else {
        Name::Invalid
    }
}

/// The editor owns only local focus and text. Metadata refreshes leave it mounted;
/// navigating to a different disk, folder or file cancels the unfinished name.
pub(super) fn control(
    state: RwSignal<State>,
    label: String,
    target: Target,
    disabled: Signal<bool>,
) -> impl IntoView {
    let draft = super::inline::Draft::new(state, target.scope());
    let editing = draft.editing;
    let value = draft.value;
    let error = draft.error;
    let label = StoredValue::new(label);
    let target = StoredValue::new(target);

    view! {
        <span class="zxs-inline-branch zxs-flow zxs-flow--inline">
            {move || if editing.get() {
                // Each edit receives its own ref so focus also works when a
                // cancelled editor is opened again.
                let input = NodeRef::<html::Input>::new();
                input.on_load(|input| { if let Err(error) = input.focus() { web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error); } });
                view! {
                    <input class="zxs-branch-name zxs-input zxs-input--compact zxs-input--underline" node_ref=input type="text"
                        aria-label=t("分支名称","Branch name")
                        aria-invalid=move || error.get().to_string()
                        title=t("最多 128 个 UTF-8 字节。","Up to 128 UTF-8 bytes.") data-max-bytes="128" autocomplete="off"
                        autocapitalize="none" spellcheck="false"
                        prop:value=move || value.get()
                        disabled=move || disabled.get() || draft.pending()
                        on:input=move |event| { value.set(event_target_value(&event)); error.set(false); }
                        on:blur=move |_| draft.cancel_idle()
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            if event.is_composing() { return; }
                            match event.key().as_str() {
                                "Escape" => {
                                    event.prevent_default(); event.stop_propagation();
                                    draft.cancel_idle();
                                }
                                "Enter" => {
                                    event.prevent_default(); event.stop_propagation();
                                    if disabled.get_untracked() || draft.pending() { return; }
                                    match branch_name(&value.get_untracked()) {
                                        Name::Invalid => error.set(true),
                                        Name::Empty => draft.cancel_idle(),
                                        Name::Valid(name) => draft.submit(|| target.get_value().submit(state, name)),
                                    }
                                }
                                _ => {}
                            }
                        }/>
                }.into_any()
            } else {
                view! {
                    <button class="zxs-button" type="button" disabled=move || disabled.get()
                        on:click=move |_| if !disabled.get_untracked() { draft.start(String::new()); }>
                        {label.get_value()}
                    </button>
                }.into_any()
            }}
            <Show when=move ||error.get()>
                <span class="zxs-branch-error zxs-row" role="status">{t("分支名称无效，最多 128 个 UTF-8 字节。","Invalid branch name. Use at most 128 UTF-8 bytes.")}</span>
            </Show>
        </span>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/branch/tests.rs"]
mod tests;
