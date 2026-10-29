use super::actions;
use crate::ui::State;
use web_interface::{html, i18n::t, prelude::*};

/// All branch consumers share one replaceable candidate page. The selected
/// action identities live in their drafts, not in an accumulated page cache.
pub(super) fn paging(state: RwSignal<State>) -> impl IntoView {
    use super::history::state::BranchReadState;
    let paging = Memo::new(move |_| {
        state.with(|s| {
            let history = &s.files.history;
            let page = history.branch_page();
            (
                page.and_then(|page| page.previous_cursor.clone()),
                page.and_then(|page| page.next_cursor.clone()),
                history.root().is_none() || matches!(history.branch_read, BranchReadState::Loading),
            )
        })
    });
    let error = Memo::new(move |_| {
        state.with(|s| match &s.files.history.branch_read {
            BranchReadState::Failed(error) => Some(error.to_string()),
            _ => None,
        })
    });
    view! {
        <div class="zxs-file-toolbar zxs-flow" aria-label=t("分支分页", "Branch pages")>
            <button class="zxs-button" disabled=move ||paging.with(|(previous,_,busy)|previous.is_none()||*busy)
                on:mousedown=move |event|actions::history::preserve_editor_focus(state,&event)
                on:click=move |_|if let Some(cursor)=paging.with_untracked(|(previous,_,_)|previous.clone()){actions::load_history_branches(state,Some(cursor));}>
                {t("上一页分支", "Previous branches")}
            </button>
            <button class="zxs-button" disabled=move ||paging.with(|(_,next,busy)|next.is_none()||*busy)
                on:mousedown=move |event|actions::history::preserve_editor_focus(state,&event)
                on:click=move |_|if let Some(cursor)=paging.with_untracked(|(_,next,_)|next.clone()){actions::load_history_branches(state,Some(cursor));}>
                {t("下一页分支", "Next branches")}
            </button>
            <Show when=move ||error.with(Option::is_some)>
                <span class="zxs-row zxs-inline-error" role="status">{move ||error.get().unwrap_or_default()}</span>
                <button class="zxs-button" disabled=move ||paging.with(|(_,_,busy)|*busy)
                    on:mousedown=move |event|actions::history::preserve_editor_focus(state,&event)
                    on:click=move |_|{let cursor=state.with_untracked(|s|s.files.history.branch_cursor.clone());actions::load_history_branches(state,cursor);}>
                    {t("重试分支读取", "Retry branches")}
                </button>
            </Show>
        </div>
    }
}

#[derive(Clone)]
pub(super) enum Target {
    CurrentDirectory,
    Directory { path: String, select: bool },
    CurrentFileVersion,
    HistoryNode(String),
}

impl Target {
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
            let entry = model.selected.as_ref()?;
            Some((
                entry.version_id()?.to_owned(),
                entry.node_id().map(str::to_owned),
            ))
        }
        Target::HistoryNode(id) => {
            // Only a current (not stale) resident page can source a branch.
            let record = model.history.loaded()?.node(id)?;
            Some((record.version_id.clone(), Some(record.node_id.clone())))
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
    let draft = super::inline::Draft::new(state);
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
