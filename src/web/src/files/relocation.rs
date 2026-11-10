//! Review one ordinary file against its exact listing before retaining the request.
use super::{actions, model::entry_key, page::Entry};
use crate::ui::State;
use storage_contract::{
    file_relocation::{FileRelocation, NamespaceEntryId},
    history_window::{HistoryCommitId, HistoryFileId, HistoryRepositoryId},
};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, PartialEq, Eq)]
struct Basis {
    epoch: u64,
    principal: String,
    generation: u64,
    slot: String,
    directory: String,
    repository: String,
    head: String,
    entry: String,
    file: String,
    source: String,
}
#[derive(Clone)]
struct Review {
    basis: Basis,
    request: FileRelocation,
}
fn selected(state: &State) -> Option<&Entry> {
    let entries = &state.files.entries;
    let entry = if state.files.checked_entries.is_empty() {
        let selected = state.files.selected.as_ref()?;
        entries
            .iter()
            .find(|entry| entry_key(entry) == entry_key(selected))?
    } else if state.files.checked_entries.len() == 1 {
        entries
            .iter()
            .find(|entry| state.files.checked_entries.contains(&entry_key(entry)))?
    } else {
        return None;
    };
    eligible(state, entry).then_some(entry)
}
fn eligible(state: &State, entry: &Entry) -> bool {
    actions::write_allowed(state)
        && state.files.loaded
        && !state.files.directory_branches_supported
        && !entry.file_id().is_empty()
        && !entry.read_only()
        && !entry.backup()
        && !entry.retained_history()
        && !entry.inherited()
        && entry.repository_id() == Some(state.files.repository_id.as_str())
}
pub(super) fn available(state: &State, entry: Option<&Entry>) -> bool {
    match entry {
        Some(entry) => state
            .files
            .entries
            .iter()
            .any(|current| current == entry && eligible(state, current)),
        None => selected(state).is_some(),
    }
}
fn basis(state: &State) -> Option<Basis> {
    let entry = selected(state)?;
    Some(Basis {
        epoch: state.session_epoch,
        principal: state.principal.clone(),
        generation: state.files.generation,
        slot: state.files.slot.clone(),
        directory: state.files.directory.clone(),
        repository: state.files.repository_id.clone(),
        head: state.files.namespace_head.clone(),
        entry: entry.entry_id().to_owned(),
        file: entry.file_id().to_owned(),
        source: entry.repository_path()?.to_owned(),
    })
}
fn request(basis: &Basis, destination: &str) -> storage_contract::Result<FileRelocation> {
    Ok(FileRelocation {
        repository_id: HistoryRepositoryId::parse(&basis.repository)?,
        expected_head: HistoryCommitId::parse(&basis.head)?,
        entry_id: NamespaceEntryId::parse(&basis.entry)?,
        file_id: HistoryFileId::parse(&basis.file)?,
        source: name_policy::PortablePath::parse(&basis.source)?,
        destination: name_policy::PortablePath::parse(destination)?,
    })
}

#[derive(Clone, Copy)]
struct Editor {
    draft: RwSignal<Option<Basis>>,
    destination: RwSignal<String>,
    review: RwSignal<Option<Review>>,
    error: RwSignal<Option<String>>,
}

/// Files and Details open the same editor; only its existing submit path writes.
pub(crate) fn provide(state: RwSignal<State>) {
    let editor = Editor {
        draft: RwSignal::new(None),
        destination: RwSignal::new(String::new()),
        review: RwSignal::new(None),
        error: RwSignal::new(None),
    };
    provide_context(editor);
    Effect::new(move |_| {
        let current = state.with(basis);
        if editor
            .draft
            .with_untracked(|draft| draft.is_some() && draft.as_ref() != current.as_ref())
        {
            editor.draft.set(None);
            editor.review.set(None);
            editor.error.set(None);
        }
    });
}

pub(super) fn start(state: RwSignal<State>, entry: Option<Entry>) {
    let entry = state.with_untracked(|state| match entry.as_ref() {
        Some(entry) => state
            .files
            .entries
            .iter()
            .find(|current| *current == entry && eligible(state, current))
            .cloned(),
        None => selected(state).cloned(),
    });
    let Some(entry) = entry else {
        return;
    };
    state.update(|state| {
        state.files.clear_selection();
        state.files.checked_entries.clear();
        state.files.selected = Some(entry);
        state.files.batch = None;
        state.change_page(crate::ui::Page::Files);
    });
    let captured = state
        .with_untracked(basis)
        .expect("invariant: opening a move selects its admitted ordinary file");
    let editor = expect_context::<Editor>();
    editor.destination.set(captured.source.clone());
    editor.draft.set(Some(captured));
    editor.review.set(None);
    editor.error.set(None);
}

pub(super) fn controls(state: RwSignal<State>) -> impl IntoView {
    let Editor {
        draft,
        destination,
        review,
        error,
    } = expect_context::<Editor>();
    view! {
        <Show when=move ||state.with(|s|s.files.loaded&&!s.files.directory_branches_supported)>
            <div class="zxs-stack">
                <Show when=move ||draft.with(|draft|draft.is_some())>
                    <section class="zxs-surface zxs-stack" data-zxs-ordinary-file-relocation="">
                        <p>{move ||draft.with(|draft|draft.as_ref().map(|draft|format!(
                            "{} · {} · {}", draft.source, draft.file, draft.slot,
                        )))}</p>
                        <p>{t(
                            "目标是当前仓库内的完整相对文件路径。父目录须已存在，不覆盖其他文件，不创建或初始化目录仓库。修改路径后先审阅，再确认发送。",
                            "Enter the complete destination file path relative to this repository. Its parent must exist. No overwrite or folder-repository initialization. Review the changed path before confirming.",
                        )}</p>
                        <input class="zxs-input" type="text" autocomplete="off" spellcheck="false"
                            aria-label=t("目标完整相对路径", "Complete relative destination path")
                            prop:value=move ||destination.get()
                            on:input=move |event| {
                                destination.set(event_target_value(&event));
                                review.set(None);
                                error.set(None);
                            }/>
                        <button class="zxs-button" on:click=move |_| {
                            let captured = draft.get_untracked();
                            if captured.as_ref() != state.with_untracked(basis).as_ref() {
                                draft.set(None);
                                review.set(None);
                                return;
                            }
                            if let Some(captured) = captured {
                                match request(&captured, &destination.get_untracked()) {
                                    Ok(request) if request.source != request.destination => {
                                        review.set(Some(Review { basis: captured, request }));
                                        error.set(None);
                                    }
                                    Ok(_) => error.set(Some(t("目标路径未改变。", "The destination is unchanged."))),
                                    Err(source) => error.set(Some(source.to_string())),
                                }
                            }
                        }>{t("审阅原文件与目标", "Review original file and destination")}</button>
                        {move ||review.get().map(|captured| {
                            let label = format!("{} → {}", captured.request.source.as_str(), captured.request.destination.as_str());
                            let captured = StoredValue::new(captured);
                            view! {
                                <p>{label}</p>
                                <button class="zxs-button" on:click=move |_| {
                                    let captured = captured.get_value();
                                    if state.with_untracked(basis).as_ref() != Some(&captured.basis)
                                        || destination.get_untracked() != captured.request.destination.as_str()
                                    {
                                        review.set(None);
                                        error.set(Some(t("依据已变，请重新审阅。", "The basis changed. Review again.")));
                                        return;
                                    }
                                    if super::mkdir::start_relocation(state, captured.request).is_some() {
                                        draft.set(None);
                                        review.set(None);
                                    }
                                }>{t("确认移动这个原文件", "Confirm moving this original file")}</button>
                            }
                        })}
                        <button class="zxs-button" on:click=move |_| {
                            draft.set(None);
                            review.set(None);
                            error.set(None);
                        }>{t("取消", "Cancel")}</button>
                        {move ||error.get().map(|error|view!{<p role="alert">{error}</p>})}
                    </section>
                </Show>
            </div>
        </Show>
    }
}
