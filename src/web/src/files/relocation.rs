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
    if !actions::write_allowed(state) || !state.files.loaded
        || state.files.directory_branches_supported
    {
        return None;
    }
    let entries = &state.files.entries;
    let entry = if state.files.checked_entries.is_empty() {
        let selected = state.files.selected.as_ref()?;
        entries.iter().find(|entry| entry_key(entry) == entry_key(selected))?
    } else if state.files.checked_entries.len() == 1 {
        entries.iter().find(|entry| state.files.checked_entries.contains(&entry_key(entry)))?
    } else {
        return None;
    };
    (!entry.file_id().is_empty() && !entry.read_only() && !entry.backup()
        && !entry.retained_history() && !entry.inherited()
        && entry.repository_id() == Some(state.files.repository_id.as_str()))
        .then_some(entry)
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

pub(super) fn controls(state: RwSignal<State>) -> impl IntoView {
    let draft = RwSignal::new(None::<Basis>);
    let destination = RwSignal::new(String::new());
    let review = RwSignal::new(None::<Review>);
    let error = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let current = state.with(basis);
        if draft.with_untracked(|draft| draft.is_some() && draft.as_ref() != current.as_ref()) {
            draft.set(None);
            review.set(None);
            error.set(None);
        }
    });
    view! {
        <Show when=move ||state.with(|s|s.files.loaded&&!s.files.directory_branches_supported)>
            <div class="zxs-stack">
                <button class="zxs-button" disabled=move ||state.with(|s|basis(s).is_none())
                    on:click=move |_| {
                        if let Some(captured) = state.with_untracked(basis) {
                            destination.set(captured.source.clone());
                            draft.set(Some(captured));
                            review.set(None);
                            error.set(None);
                        }
                    }>{t("重命名／同盘移动文件", "Rename / move file on this disk")}</button>
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
