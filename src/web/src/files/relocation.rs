//! Review one current object and its destination through the shared move request.
use super::{actions, model::entry_key, page::Entry};
use crate::{
    api,
    ui::{State, accept_read_response},
};
use storage_contract::{
    file_relocation::{NamespaceEntryId, ObjectMoveRequest},
    history_window::HistoryRepositoryId,
};
use web_interface::{
    components::{ErrorState, Select, SelectOption},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

mod folders;
mod target;
mod completion;

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
    source: String,
    display_path: String,
}
#[derive(Clone)]
struct Review {
    basis: Basis,
    destination: String,
    request: ObjectMoveRequest,
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
        && !entry.read_only()
        && !entry.backup()
        && !entry.retained_history()
        && entry.repository_path().is_some_and(|path| !path.is_empty())
        && entry.repository_id().is_some()
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
        repository: entry.repository_id()?.to_owned(),
        head: state.files.namespace_head.clone(),
        entry: entry.entry_id().to_owned(),
        source: entry.repository_path()?.to_owned(),
        display_path: entry.path().to_owned(),
    })
}

#[derive(Clone, Copy)]
struct Editor {
    draft: RwSignal<Option<Basis>>,
    target_slot: RwSignal<String>,
    destination: RwSignal<String>,
    review: RwSignal<Option<Review>>,
    folders: RwSignal<Option<folders::Listing>>,
    error: RwSignal<Option<api::Error>>,
    reading: RwSignal<bool>,
    generation: RwSignal<u64>,
    cancellation: RwSignal<Option<api::ReadCancellation>>,
    completion: RwSignal<Option<completion::Destination>>,
}

impl Editor {
    fn clear_review(self) {
        if let Some(cancellation) = self.cancellation.get_untracked() {
            cancellation.abort();
        }
        self.cancellation.set(None);
        self.generation.update(|generation| *generation += 1);
        self.reading.set(false);
        self.review.set(None);
        self.folders.set(None);
        self.error.set(None);
    }
}

/// Files and Details open the same editor; only its existing submit path writes.
pub(crate) fn provide(state: RwSignal<State>) {
    let editor = Editor {
        draft: RwSignal::new(None),
        target_slot: RwSignal::new(String::new()),
        destination: RwSignal::new(String::new()),
        review: RwSignal::new(None),
        folders: RwSignal::new(None),
        error: RwSignal::new(None),
        reading: RwSignal::new(false),
        generation: RwSignal::new(0),
        cancellation: RwSignal::new(None),
        completion: RwSignal::new(None),
    };
    provide_context(editor);
    Effect::new(move |_| {
        let owner = state.with(|state| {
            state.session_active().then(|| (state.session_epoch, state.principal.clone()))
        });
        if editor.completion.with_untracked(|completion| {
            completion.as_ref().is_some_and(|destination| !destination.owned_by(owner.as_ref()))
        }) {
            editor.completion.set(None);
        }
    });
    Effect::new(move |_| {
        let current = state.with(basis);
        if editor
            .draft
            .with_untracked(|draft| draft.is_some() && draft.as_ref() != current.as_ref())
        {
            editor.draft.set(None);
            editor.clear_review();
        }
    });
    on_cleanup(move || {
        if let Some(cancellation) = editor.cancellation.try_with_untracked(Clone::clone).flatten() {
            cancellation.abort();
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
        .expect("invariant: opening a move selects its admitted current object");
    let editor = expect_context::<Editor>();
    editor.clear_review();
    editor.completion.set(None);
    editor.target_slot.set(captured.slot.clone());
    editor.destination.set(captured.display_path.clone());
    editor.draft.set(Some(captured));
}

struct ReadTarget {
    basis: Basis,
    slot: String,
    cancellation: api::ReadCancellation,
    generation: u64,
}
impl ReadTarget {
    fn current(&self, state: RwSignal<State>, editor: Editor) -> bool {
        editor.generation.try_with_untracked(|generation| *generation) == Some(self.generation)
            && editor.target_slot.try_with_untracked(|slot| slot == &self.slot) == Some(true)
            && state.with_untracked(basis).as_ref() == Some(&self.basis)
    }
}

fn begin_read(state: RwSignal<State>, editor: Editor) -> Option<ReadTarget> {
    editor.clear_review();
    let captured = editor.draft.get_untracked()?;
    if state.with_untracked(basis).as_ref() != Some(&captured) {
        return None;
    }
    let slot = editor.target_slot.get_untracked();
    if !state.with_untracked(|state| {
        state
            .files
            .disks
            .iter()
            .any(|disk| disk.slot_id == slot && disk.online && disk.writable)
    }) {
        editor.error.set(Some(api::error(
            storage_contract::ErrorCode::InvalidRequest,
            t("请选择在线可写的目标磁盘。", "Choose an online writable destination disk."),
        )));
        return None;
    }
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            editor.error.set(Some(error));
            return None;
        }
    };
    editor.reading.set(true);
    editor.cancellation.set(Some(cancellation.clone()));
    Some(ReadTarget {
        basis: captured,
        slot,
        cancellation,
        generation: editor.generation.get_untracked(),
    })
}

fn review_destination(state: RwSignal<State>, editor: Editor) {
    let destination = match name_policy::PortablePath::parse(&editor.destination.get_untracked()) {
        Ok(path) => path,
        Err(source) => {
            editor.clear_review();
            editor.error.set(Some(target::input_error(source.into())));
            return;
        }
    };
    let Some(read) = begin_read(state, editor) else {
        return;
    };
    spawn_local(async move {
        let result = target::review(
            read.basis.clone(), read.slot.clone(), destination, read.cancellation.clone(),
        ).await;
        if !accept_read_response(state, read.basis.epoch, &result) || !read.current(state, editor) {
            return;
        }
        editor.reading.set(false);
        editor.cancellation.set(None);
        match result {
            Ok(review) => editor.review.set(Some(review)),
            Err(error) => editor.error.set(Some(error)),
        }
    });
}

pub(super) fn controls(state: RwSignal<State>) -> impl IntoView {
    let editor = expect_context::<Editor>();
    let slots = Signal::derive(move || {
        state.with(|state| {
            state.files.disks.iter().filter(|disk| disk.online && disk.writable)
                .map(|disk| SelectOption::new(disk.slot_id.clone(), disk.name.clone()))
                .collect::<Vec<_>>()
        })
    });
    let selected_slot = Signal::derive(move || editor.target_slot.get());
    let disabled = Signal::derive(move || !state.with(actions::write_allowed));
    view! {
        {completion::view(state, editor.completion)}
        <Show when=move ||editor.draft.with(|draft|draft.is_some())>
            <section class="zxs-surface zxs-stack" data-zxs-object-move="">
                <p>{move ||editor.draft.with(|draft|draft.as_ref().map(|draft|format!(
                    "{} / {} · {}", draft.slot, draft.display_path, draft.entry,
                )))}</p>
                <p>{t(
                    "将所选的一个文件或整个目录连同完整历史一起移动。选择目标磁盘，填写相对磁盘根的完整目标路径。除空盘根以外，目标父目录须已存在且可写；不覆盖已有条目，普通目录不会因此变成独立目录仓库。",
                    "Move the selected file or whole folder together with its complete history. Choose a disk and enter the complete destination path relative to its root. Except for a new disk root, its parent must exist and be writable. Existing entries are not replaced, and ordinary folders do not become independent repositories.",
                )}</p>
                <Select label=t("目标磁盘", "Destination disk") options=slots value=selected_slot disabled=disabled
                    placeholder=t("选择目标磁盘", "Choose destination disk")
                    on_change=Callback::new(move |slot:String| {
                        editor.clear_review();
                        editor.target_slot.set(slot);
                    })/>
                {folders::control(state, editor)}
                <input class="zxs-input" type="text" autocomplete="off" spellcheck="false"
                    aria-label=t("目标完整磁盘相对路径", "Complete disk-relative destination path")
                    prop:value=move ||editor.destination.get() disabled=move ||disabled.get()
                    on:input=move |event| {
                        editor.clear_review();
                        editor.destination.set(event_target_value(&event));
                    }/>
                <button class="zxs-button" disabled=move ||disabled.get()
                    on:click=move |_| {
                        let Some(draft) = editor.draft.get_untracked() else { return; };
                        let name = draft.display_path.rsplit('/').next()
                            .expect("invariant: an admitted object path has a name");
                        editor.clear_review();
                        editor.destination.set(name.to_owned());
                    }>{t("保留名称，放到目标磁盘根目录", "Keep name and use destination disk root")}</button>
                <button class="zxs-button" disabled=move ||disabled.get()||editor.reading.get()
                    on:click=move |_|review_destination(state,editor)>{t("读取目标并审阅移动", "Read destination and review move")}</button>
                <Show when=move ||editor.reading.get()>
                    <p role="status">{t("正在读取目标父目录…", "Reading destination parent folder…")}</p>
                </Show>
                {move ||editor.review.get().map(|captured| {
                    let label = format!("{} / {} → {} / {}", captured.basis.slot,
                        captured.basis.display_path, captured.request.target_slot_id, captured.destination);
                    let target_context = captured.request.target_repository_id.as_ref().map_or_else(
                        || t("目标是空盘：根仓库会随本次移动一同创建，无须先初始化。",
                            "The destination disk is empty. Its root repository will be created as part of this move; no separate initialization is needed.").to_string(),
                        |repository| format!("{}: {}", t("目标所属仓库", "Destination repository"), repository.as_str()),
                    );
                    let captured = StoredValue::new(captured);
                    view! {
                        <p>{label}</p>
                        <p>{target_context}</p>
                        <button class="zxs-button" disabled=move ||disabled.get()
                            on:click=move |_| {
                                let captured = captured.get_value();
                                if state.with_untracked(basis).as_ref() != Some(&captured.basis)
                                    || editor.target_slot.get_untracked() != captured.request.target_slot_id
                                    || editor.destination.get_untracked() != captured.destination
                                {
                                    editor.clear_review();
                                    editor.error.set(Some(api::error(storage_contract::ErrorCode::InvalidRequest,
                                        t("依据已变，请重新审阅。", "The basis changed. Review again."))));
                                    return;
                                }
                                if let Some(operation) = super::mkdir::start_relocation(state, captured.request.clone()) {
                                    editor.completion.set(Some(completion::Destination::submitted(captured, operation)));
                                    editor.draft.set(None);
                                    editor.clear_review();
                                }
                            }>{t("确认移动所选对象", "Confirm moving the selected object")}</button>
                    }
                })}
                <button class="zxs-button" on:click=move |_| {
                    editor.draft.set(None);
                    editor.clear_review();
                }>{t("取消", "Cancel")}</button>
                {move ||editor.error.get().map(|error| {
                    let feedback=crate::ui::feedback(&error);
                    view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}
                })}
            </section>
        </Show>
    }
}
