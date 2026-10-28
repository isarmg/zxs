use super::page::Entry;
use super::{
    actions, backup, batch, branch, inline, merge,
    model::{
        Model, branch_name, branch_writable, history_nodes_ref, join_path, selected_branch,
        selected_entry_read_only, selected_file_branch_record,
    },
    tags, version_tree,
};
use crate::{
    icons::FileIcon,
    ui::{State, list, text},
};
use web_interface::components::{EmptyState, LoadingState, Select, SelectOption};
use web_interface::{i18n::t, prelude::*};

pub(crate) fn page(state: RwSignal<State>) -> impl IntoView {
    inline::provide_context(state);
    super::history_index::provide(state);
    let selected = detail_selection(state);
    view! {
        <section class="zxs-content-stack zxs-entry-page zxs-stack">
            <nav class="zxs-entry-navigation zxs-flow" aria-label=t("详情导航","Details navigation")>
                <button class="zxs-button" on:click=move |_|state.update(|s|{s.page=crate::ui::Page::Files;})><FileIcon name="back"/>{t("返回文件列表","Back to files")}</button>
                <span>{move ||state.with(|s|join_path(&s.files.slot,&s.files.directory))}</span>
            </nav>
            {move ||match selected.get() {
                Some(selection) => entry_view(state, selection).into_any(),
                None => view!{<EmptyState message=t("请在文件页面选择一个文件或文件夹，查看详情。","Choose a file or folder on the Files page to view its details.")/>}.into_any(),
            }}
        </section>
    }
}
/// Detail controls own browser focus and popover state. Refreshing metadata must
/// update their signals without recreating the controls for the same entry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    epoch: u64,
    slot: String,
    directory: String,
    entry_id: String,
    file_id: String,
    path: String,
    kind: storage_contract::NamespaceEntryKind,
    repository: String,
    repository_path: String,
}
impl Selection {
    fn capture(state: &State) -> Option<Self> {
        let entry = state.files.selected.as_ref()?;
        let repository = entry.repository_id().unwrap_or_default().to_owned();
        let repository_path = entry.repository_path().unwrap_or_default().to_owned();
        Some(Self {
            epoch: state.session_epoch,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            entry_id: entry.entry_id().to_owned(),
            file_id: entry.file_id().to_owned(),
            path: entry.path().to_owned(),
            kind: entry.kind(),
            repository,
            repository_path,
        })
    }

    fn entry<'a>(&self, state: &'a State) -> Option<&'a Entry> {
        (Self::capture(state).as_ref() == Some(self))
            .then_some(state.files.selected.as_ref())
            .flatten()
    }

    /// Event handlers must not redirect a queued old control to a new selection.
    fn action_entry<'a>(&self, state: &'a State) -> Option<&'a Entry> {
        state.session_active().then(|| self.entry(state)).flatten()
    }

    fn mutation_entry<'a>(&self, state: &'a State) -> Option<&'a Entry> {
        self.action_entry(state)
            .filter(|entry| actions::write_allowed(state) && !entry.read_only())
    }
}
fn detail_selection(state: RwSignal<State>) -> Memo<Option<Selection>> {
    Memo::new(move |_| state.with(Selection::capture))
}

fn file_note_ready(model: &Model) -> bool {
    !model.history_loading && model.file_history["note"].is_string()
}
fn entry_view(state: RwSignal<State>, selection: Selection) -> impl IntoView {
    let is_file = selection.kind == storage_contract::NamespaceEntryKind::File;
    let folder_path = selection.path.clone();
    let folder_select_path = folder_path.clone();
    let selection = StoredValue::new(selection);
    let entry = Memo::new(move |_| {
        state.with(|s| selection.with_value(|selection| selection.entry(s).cloned()))
    });
    let options = Signal::derive(move || {
        list(&state.with(|s| s.files.file_history.clone()), "branches")
            .into_iter()
            .map(|branch| {
                let readonly = !branch_writable(&branch);
                let label = if readonly {
                    format!("{} · {}", branch_name(&branch), t("只读", "Read only"))
                } else {
                    branch_name(&branch)
                };
                let mut option = SelectOption::new(text(&branch, &["branch_id"]), label);
                option.disabled = readonly;
                option
            })
            .collect::<Vec<_>>()
    });
    let value = Signal::derive(move || state.with(|s| selected_branch(&s.files)));
    let rename_branch = Memo::new(move |_| state.with(|s| selected_file_branch_record(&s.files)));
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files.history_loading
                || list(&s.files.file_history, "branches").is_empty()
        })
    });
    let merge_disabled = Signal::derive(move || {
        state.with(|s| !actions::write_allowed(s) || !merge::available(&s.files))
    });
    let can_modify = Signal::derive(move || {
        state.with(|s| s.files.writable && !selected_entry_read_only(&s.files))
    });
    let mutation_disabled =
        Signal::derive(move || !can_modify.get() || !state.with(actions::write_allowed));
    let rename_disabled = Signal::derive(move || {
        mutation_disabled.get() || state.with(|s| !batch::available(&s.files))
    });
    let note_disabled = Signal::derive(move || {
        mutation_disabled.get() || state.with(|s| !file_note_ready(&s.files))
    });
    let content_ready = Signal::derive(move || {
        state.with(|s| {
            s.session_active()
                && s.files
                    .selected
                    .as_ref()
                    .is_some_and(Entry::visible_content)
        })
    });
    let inherited = Signal::derive(move || {
        state.with(|s| {
            s.files
                .selected
                .as_ref()
                .is_some_and(Entry::inherited_content)
        })
    });
    let current_branch_disabled = Signal::derive(move || {
        mutation_disabled.get() || !content_ready.get() || state.with(|s| s.files.loading)
    });
    view! {
        <article class="zxs-content-panel zxs-entry-content zxs-frame zxs-stack" aria-label=t("文件详情","File details")>
            <div class="zxs-file-heading zxs-flow"><h2 class="zxs-row">{move ||entry.with(|entry|entry.as_ref().map(|entry|entry.name().to_owned()).unwrap_or_default())}</h2><div class="zxs-entry-actions zxs-flow">
                {(!is_file).then(move ||view!{<button class="zxs-button" on:click=move |_|if let Some(path)=state.with_untracked(|s|selection.with_value(|key|key.action_entry(s).map(|entry|entry.path().to_owned()))){actions::go_directory(state,path);}><FileIcon name="folder"/>{t("打开文件夹","Open folder")}</button>})}
                <Show when=move ||can_modify.get()><button class="zxs-button" disabled=move ||rename_disabled.get() on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.mutation_entry(s).cloned())){batch::start(state,batch::Kind::Rename,Some(entry));}><FileIcon name="rename"/>{t("重命名","Rename")}</button></Show>
                <button class="zxs-button" disabled=move ||state.with(|s|actions::pending(s)||selection.with_value(|key|key.action_entry(s).and_then(|entry|super::model::file_context(entry))).is_none()) on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.action_entry(s).cloned())){backup::start_entry(state,entry);}>{t("备份设置","Backup settings")}</button>
                <Show when=move ||can_modify.get()><button class="zxs-button" disabled=move ||mutation_disabled.get() on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.mutation_entry(s).cloned())){batch::start(state,batch::Kind::Delete,Some(entry));}><FileIcon name="delete"/>{t("删除","Delete")}</button></Show>
            </div></div>
            {backup::view(state)}
            <dl class="zxs-file-properties"><div><dt class="zxs-row">{t("位置","Location")}</dt><dd class="zxs-row">{move ||entry.with(|entry|entry.as_ref().map(|entry|join_path(&state.with(|s|s.files.slot.clone()),entry.path())).unwrap_or_default())}</dd></div><div><dt class="zxs-row">{t("类型","Type")}</dt><dd class="zxs-row">{if is_file{t("文件","File")}else{t("文件夹","Folder")}}</dd></div></dl>
            {(!is_file).then(move ||view!{<Show when=move ||can_modify.get()><section class="zxs-entry-section zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("目录分支","Folder branches")}</h3><p class="zxs-row">{t("目录分支记录此目录及子目录的文件分支组合，不创建目录私有的文件分支。修改只在当前分支追加子树节点；切换恢复所选组合，未写入的文件继续显示父版本，目录外文件保持不变。","A folder branch records the shared file branch combination in this folder and its subfolders. Changes append subtree nodes within the current branch. Switching restores the selected combination; unwritten files inherit their parent versions and files outside the folder are unchanged.")}</p><div class="zxs-file-toolbar zxs-flow">{branch::control(state,t("创建目录分支","Create folder branch"),branch::Target::Directory{path:folder_path.clone(),select:false},mutation_disabled)}{branch::control(state,t("切换目录分支","Switch folder branch"),branch::Target::Directory{path:folder_select_path.clone(),select:true},mutation_disabled)}</div><p class="zxs-row">{t("目录分支合并待确认，暂不提供操作。","Folder branch merging awaits confirmation.")}</p></section></Show>})}
            {is_file.then(move ||view! {
                <section class="zxs-entry-section zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("文件备注","File note")}</h3><Show when=move ||can_modify.get() fallback=move ||view!{<p class="zxs-row">{move ||state.with(|s|text(&s.files.file_history,&["note"]))}</p>}>{inline::control(state,t("编辑文件备注","Edit file note"),None,inline::Target::FileNote,note_disabled)}</Show></section>
                <Show when=move ||inherited.get()><p class="zxs-row" role="status">{t("此分支尚未写入，正在显示父版本的元数据；通过 SMB 写入后才保存本分支自己的版本。","This unwritten branch displays its parent version’s metadata. Writing through SMB creates this branch’s own version.")}</p></Show>
                <Show when=move ||can_modify.get()><section class="zxs-entry-section zxs-file-branches zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("文件分支","File branches")}</h3><p class="zxs-row">{t("创建时只登记分支并引用父节点，不创建占位文件或复制正文。首次写入后形成自己的版本；切换只更新当前目录的文件引用。","Creating a branch only registers a reference to its parent node, without placeholders or copied content. The first write creates its own version; switching updates only this folder’s file reference.")}</p>
                    <div class="zxs-file-toolbar zxs-flow"><Select label=t("当前分支","Current branch") options=options value=value disabled=disabled placeholder=t("主版本","Main") on_change=Callback::new(move |branch:String|actions::select_branch(state,branch))/>
                        {move ||rename_branch.get().map(|(file,branch_id,name)|inline::control(state,t("重命名文件分支","Rename file branch"),Some("rename"),inline::Target::RenameFileBranch{file,branch_id,name},disabled))}
                    </div>
                    <div class="zxs-file-toolbar zxs-flow">{branch::control(state,t("从当前内容创建分支","Branch from current content"),branch::Target::CurrentFileVersion,current_branch_disabled)}<button class="zxs-button" disabled=move ||merge_disabled.get() on:click=move |_|merge::start(state)>{t("合并文件分支","Merge file branches")}</button></div>
                    {merge::view(state)}
                </section></Show>
                <section class="zxs-entry-section zxs-entry-version-history zxs-stack"><h3 class="zxs-row">{t("版本历史","Version history")}</h3><Show when=move ||state.with(|s|s.files.history_loading)><LoadingState/></Show>
                    <Show when=move ||can_modify.get()>{tags::view(state)}</Show>
                    {version_tree::view(state)}
                    {merge::history()}
                    <Show when=move ||state.with(|s|!s.files.history_loading&&history_nodes_ref(&s.files.file_history).is_empty())><p class="zxs-row">{t("尚无版本记录。","No versions have been recorded.")}</p></Show>
                </section>
            })}
        </article>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/detail/detail_tests.rs"]
mod detail_tests;
