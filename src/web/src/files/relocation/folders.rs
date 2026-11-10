//! Bounded target-folder navigation only edits the existing move draft.
use super::*;
use crate::files::page::PAGE_SIZE;

#[derive(Clone)]
pub(super) struct Listing {
    basis: Basis,
    generation: u64,
    slot: String,
    path: String,
    folders: Vec<Entry>,
    next: Option<String>,
    previous: Option<String>,
    writable: bool,
}

fn open(state: RwSignal<State>, editor: Editor, path: String, cursor: Option<String>) {
    let Some(read) = begin_read(state, editor) else {
        return;
    };
    spawn_local(async move {
        let result = target::folder_page(
            &read.slot, &path, PAGE_SIZE, cursor.as_deref(), read.cancellation.clone(),
        ).await;
        if !accept_read_response(state, read.basis.epoch, &result) || !read.current(state, editor) {
            return;
        }
        editor.reading.set(false);
        editor.cancellation.set(None);
        match result {
            Ok(page) => editor.folders.set(Some(Listing {
                basis: read.basis,
                generation: read.generation,
                slot: read.slot,
                path,
                folders: page.entries.into_iter().filter(|entry| {
                    entry.kind() == storage_contract::NamespaceEntryKind::Directory
                        && !entry.backup() && !entry.retained_history()
                }).collect(),
                next: page.next,
                previous: page.previous,
                writable: page.writable,
            })),
            Err(error) => editor.error.set(Some(error)),
        }
    });
}

fn navigate(
    state: RwSignal<State>,
    editor: Editor,
    generation: u64,
    path: String,
    cursor: Option<String>,
) {
    if editor.generation.get_untracked() == generation {
        open(state, editor, path, cursor);
    }
}

fn choose(state: RwSignal<State>, editor: Editor, generation: u64) {
    let destination = editor.folders.with_untracked(|listing| {
        let listing = listing.as_ref()?;
        if !listing.writable
            || listing.generation != generation
            || editor.generation.get_untracked() != generation
            || editor.target_slot.get_untracked() != listing.slot
            || state.with_untracked(basis).as_ref() != Some(&listing.basis)
        {
            return None;
        }
        let name = listing.basis.display_path.rsplit('/').next()
            .expect("invariant: an admitted move source has a name");
        Some(super::super::model::join_path(&listing.path, name))
    });
    if let Some(destination) = destination {
        editor.destination.set(destination);
        // Selection is not publication. Re-read the chosen parent through the
        // same review path before the user can confirm the original move.
        review_destination(state, editor);
    }
}

fn page_view(state: RwSignal<State>, editor: Editor, listing: Listing) -> impl IntoView {
    let generation = listing.generation;
    let writable = listing.writable;
    let empty = listing.folders.is_empty();
    let has_next = listing.next.is_some();
    let has_previous = listing.previous.is_some();
    let label = format!("{}/{}", listing.slot, listing.path);
    let at_root = listing.path.is_empty();
    let parent = listing.path.rsplit_once('/').map_or("", |(parent, _)| parent).to_owned();
    let path = StoredValue::new(listing.path);
    let next = StoredValue::new(listing.next);
    let previous = StoredValue::new(listing.previous);
    let folders = StoredValue::new(listing.folders);
    let disabled = Signal::derive(move || !state.with(actions::write_allowed) || editor.reading.get());
    view! {
        <section class="zxs-frame zxs-stack" data-zxs-move-folder-picker="">
            <p class="zxs-row">{label}</p>
            <p class="zxs-row">{t(
                "选择目录时保留源名称；审阅前后均可继续编辑完整目标路径。选择不会移动文件，也不会创建目录。",
                "Choosing a folder keeps the source name. You can still edit the full destination path. Selection does not move files or create folders.",
            )}</p>
            <div class="zxs-flow">
                <button type="button" class="zxs-button" disabled=move ||disabled.get() || at_root
                    on:click=move |_|navigate(state,editor,generation,parent.clone(),None)>
                    {t("上一级", "Parent folder")}
                </button>
                <button type="button" class="zxs-button" disabled=move ||disabled.get() || at_root
                    on:click=move |_|navigate(state,editor,generation,String::new(),None)>
                    {t("磁盘根目录", "Disk root")}
                </button>
                <button type="button" class="zxs-button" disabled=move ||disabled.get() || !writable
                    on:click=move |_|choose(state,editor,generation)>
                    {t("选择此目录并审阅", "Use this folder and review")}
                </button>
            </div>
            <Show when=move ||!writable>
                <p class="zxs-row">{t("此目录不能作为移动目标，仍可浏览子目录。", "This folder cannot receive the move. You can still browse its subfolders.")}</p>
            </Show>
            <Show when=move ||empty>
                <p class="zxs-row">{t("本页没有子目录。", "There are no subfolders on this page.")}</p>
            </Show>
            <For each=move ||folders.get_value() key=entry_key children=move |entry| {
                let name = entry.name().to_owned();
                let destination = StoredValue::new(entry.path().to_owned());
                view! {
                    <button type="button" class="zxs-button" disabled=move ||disabled.get()
                        on:click=move |_|navigate(state,editor,generation,destination.get_value(),None)>{name}</button>
                }
            }/>
            <div class="zxs-flow">
                <button type="button" class="zxs-button"
                    disabled=move ||disabled.get() || !has_previous
                    on:click=move |_| {
                        if let Some(cursor) = previous.get_value() {
                            navigate(state,editor,generation,path.get_value(),Some(cursor));
                        }
                    }>{t("上一页", "Previous page")}</button>
                <button type="button" class="zxs-button"
                    disabled=move ||disabled.get() || !has_next
                    on:click=move |_| {
                        if let Some(cursor) = next.get_value() {
                            navigate(state,editor,generation,path.get_value(),Some(cursor));
                        }
                    }>{t("下一页", "Next page")}</button>
                <button type="button" class="zxs-button" on:click=move |_| {
                    if editor.generation.get_untracked() == generation { editor.clear_review(); }
                }>
                    {t("收起目录选择", "Close folder picker")}
                </button>
            </div>
        </section>
    }
}

pub(super) fn control(state: RwSignal<State>, editor: Editor) -> impl IntoView {
    view! {
        <button type="button" class="zxs-button"
            disabled=move ||!state.with(actions::write_allowed) || editor.reading.get()
            on:click=move |_|open(state,editor,String::new(),None)>
            {t("浏览目标磁盘的目录", "Browse destination folders")}
        </button>
        {move ||editor.folders.get().map(|listing|page_view(state,editor,listing))}
    }
}
