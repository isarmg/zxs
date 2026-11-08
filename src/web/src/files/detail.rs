use super::page::Entry;
use super::{
    actions, backup, batch, branch, inline, merge,
    model::{
        Model, branch_name, join_path, selected_branch, selected_entry_read_only,
        selected_file_branch_record,
    },
    tags, version_tree,
};
use crate::{icons::FileIcon, ui::State};
use web_interface::components::{EmptyState, Select, SelectOption};
use web_interface::{i18n::t, prelude::*};

mod directory;

pub(crate) fn page(state: RwSignal<State>) -> impl IntoView {
    inline::provide_context(state);
    super::history_index::provide(state);
    let selected = detail_selection(state);
    let current_directory = Memo::new(move |_| {
        state.with(|s| {
            (s.session_active() && s.files.selected.is_none() && !s.files.slot.is_empty()).then(
                || {
                    (
                        s.session_epoch,
                        s.files.slot.clone(),
                        s.files.directory.clone(),
                    )
                },
            )
        })
    });
    view! {
        <section class="zxs-content-stack zxs-entry-page zxs-stack">
            {move ||match (selected.get(), current_directory.get()) {
                (Some(selection), _) => entry_view(state, selection).into_any(),
                (None, Some(_)) => directory::view(state).into_any(),
                (None, None) => view!{<EmptyState message=t("请在文件页面选择一个文件或文件夹，查看详情。","Choose a file or folder on the Files page to view its details.")/>}.into_any(),
            }}
        </section>
    }
}
/// Select the current directory without resetting its query, cursor or branch.
pub(super) fn open_current_directory(state: RwSignal<State>) {
    state.update(|s| {
        if !s.session_active() || s.files.slot.is_empty() || actions::pending(s) {
            return;
        }
        s.files.clear_selection();
        s.change_page(crate::ui::Page::Details);
    });
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
    backup: bool,
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
            backup: entry.backup(),
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

    fn directory_path<'a>(&self, state: &'a State) -> Option<&'a str> {
        self.action_entry(state)
            .filter(|entry| entry.kind() == storage_contract::NamespaceEntryKind::Directory)
            .map(Entry::path)
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
    !model.history_loading && model.file_history.ready() && model.file_history.file_note().is_some()
}

fn viewed_inherited(model: &Model) -> bool {
    if model.file_preview.is_some() {
        model.file_history.current_inherited()
    } else {
        model
            .selected
            .as_ref()
            .is_some_and(Entry::inherited_content)
    }
}

fn branch_options(model: &Model) -> Vec<SelectOption> {
    let current = model.file_history.current_branch().filter(|current| {
        !model
            .file_history
            .branches()
            .iter()
            .any(|branch| branch.id() == current.id())
    });
    let mut options = model
        .file_history
        .branches()
        .iter()
        .chain(current)
        .chain(model.file_preview.as_ref().filter(|preview| {
            !model
                .file_history
                .branches()
                .iter()
                .chain(model.file_history.current_branch())
                .any(|branch| branch.id() == preview.id())
        }))
        .map(|branch| {
            SelectOption::new(
                branch.id(),
                if branch.writable() {
                    branch_name(branch)
                } else {
                    format!("{} · {}", branch_name(branch), t("只读", "Read only"))
                },
            )
        })
        .collect::<Vec<_>>();
    let selected = selected_branch(model);
    if !selected.is_empty() && !options.iter().any(|option| option.value == selected) {
        let mut option = SelectOption::new(
            selected.clone(),
            format!(
                "{} · {selected}",
                t(
                    "已选分支（不在本页）",
                    "Selected branch (outside this page)"
                )
            ),
        );
        option.disabled = true;
        options.push(option);
    }
    options
}
fn entry_view(state: RwSignal<State>, selection: Selection) -> impl IntoView {
    let is_file = selection.kind == storage_contract::NamespaceEntryKind::File;
    let selection = StoredValue::new(selection);
    let entry = Memo::new(move |_| {
        state.with(|s| selection.with_value(|selection| selection.entry(s).cloned()))
    });
    let options = Signal::derive(move || state.with(|s| branch_options(&s.files)));
    let value = Signal::derive(move || state.with(|s| selected_branch(&s.files)));
    let rename_branch = Memo::new(move |_| state.with(|s| selected_file_branch_record(&s.files)));
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::history_navigation_ready(s)
                || actions::history_aux_loading(s)
                || branch_options(&s.files).is_empty()
        })
    });
    let merge_disabled = Signal::derive(move || {
        state.with(|s| !actions::history_write_allowed(s) || !merge::available(&s.files))
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
    let inherited = Signal::derive(move || state.with(|s| viewed_inherited(&s.files)));
    let current_branch_disabled = Signal::derive(move || {
        mutation_disabled.get()
            || !content_ready.get()
            || state.with(|s| s.files.loading || !actions::history_write_allowed(s))
    });
    let branch_page_disabled = Signal::derive(move || {
        state.with(|s| !actions::history_navigation_ready(s) || actions::history_aux_loading(s))
    });
    let history_page_disabled =
        Signal::derive(move || state.with(|s| !actions::history_navigation_ready(s)));
    let folder_navigation_disabled = Signal::derive(move || {
        state.with(|s| selection.with_value(|key| key.directory_path(s).is_none()))
    });
    view! {
        <article class="zxs-content-panel zxs-entry-content zxs-frame zxs-stack" aria-label=if is_file{t("文件详情","File details")}else{t("文件夹详情","Folder details")}>
            <div class="zxs-file-heading zxs-flow"><h2 class="zxs-row">{move ||entry.with(|entry|entry.as_ref().map(|entry|entry.name().to_owned()).unwrap_or_default())}</h2><div class="zxs-entry-actions zxs-flow">
                {(!is_file).then(move ||view!{<button class="zxs-button" disabled=move ||folder_navigation_disabled.get() on:click=move |_|if let Some(path)=state.with_untracked(|s|selection.with_value(|key|key.directory_path(s).map(str::to_owned))){actions::go_directory(state,path);}><FileIcon name="folder"/>{t("打开文件夹","Open folder")}</button>})}
                <Show when=move ||can_modify.get()><button class="zxs-button" disabled=move ||rename_disabled.get() on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.mutation_entry(s).cloned())){batch::start(state,batch::Kind::Rename,Some(entry));}><FileIcon name="rename"/>{t("重命名","Rename")}</button></Show>
                {(!is_file).then(move ||view!{<button class="zxs-button" disabled=move ||mutation_disabled.get() on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.mutation_entry(s).cloned())){super::placement::start_entry(state,entry);}>{t("主盘规则","Primary placement")}</button>})}
                <button class="zxs-button" disabled=move ||state.with(|s|actions::pending(s)||selection.with_value(|key|key.action_entry(s).and_then(super::model::file_context)).is_none()) on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.action_entry(s).cloned())){backup::recursive::start_entry(state,entry);}>{t("逐级备份","Recursive backups")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|actions::pending(s)||selection.with_value(|key|key.action_entry(s).and_then(super::model::file_context)).is_none()) on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.action_entry(s).cloned())){backup::start_entry(state,entry);}>{t("自定义备份前缀","Custom backup prefixes")}</button>
                <Show when=move ||can_modify.get()><button class="zxs-button" disabled=move ||mutation_disabled.get() on:click=move |_|if let Some(entry)=state.with_untracked(|s|selection.with_value(|key|key.mutation_entry(s).cloned())){batch::start(state,batch::Kind::Delete,Some(entry));}><FileIcon name="delete"/>{t("删除","Delete")}</button></Show>
            </div></div>
            {backup::view(state)}
            {super::placement::view(state)}
            <dl class="zxs-file-properties"><div><dt class="zxs-row">{t("位置","Location")}</dt><dd class="zxs-row">{move ||entry.with(|entry|entry.as_ref().map(|entry|join_path(&state.with(|s|s.files.slot.clone()),entry.path())).unwrap_or_default())}</dd></div><div><dt class="zxs-row">{t("类型","Type")}</dt><dd class="zxs-row">{if is_file{t("文件","File")}else{t("文件夹","Folder")}}</dd></div></dl>
            {(!is_file).then(move ||view!{
                {super::directory_history::view(state)}
                <section class="zxs-entry-section zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("目录分支与节点","Folder branches and nodes")}</h3><p class="zxs-row">{t("进入此目录的详情，管理独立仓库初始化、分支、暂存和提交。父仓库不会替子仓库提交。", "Open this folder's Details to manage repository initialization, branches, staging and commits. A parent never commits an independent child.")}</p><div class="zxs-file-toolbar zxs-flow"><button class="zxs-button" disabled=move ||folder_navigation_disabled.get()||state.with(actions::pending) on:click=move |_|if let Some(path)=state.with_untracked(|s|selection.with_value(|key|key.directory_path(s).map(str::to_owned))){actions::go_directory(state,path);crate::ui::navigate(state,crate::ui::Page::Details);}>{t("管理此目录的分支与提交", "Manage this folder's branches and commits")}</button></div></section>
            })}
            {is_file.then(move ||view! {
                <section class="zxs-entry-section zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("文件备注","File note")}</h3><Show when=move ||can_modify.get() fallback=move ||view!{<p class="zxs-row">{move ||state.with(|s|s.files.file_history.file_note().unwrap_or_default().to_owned())}</p>}>{inline::control(state,t("编辑文件备注","Edit file note"),None,inline::Target::FileNote,note_disabled)}</Show></section>
                <Show when=move ||inherited.get()><p class="zxs-row" role="status">{t("此分支尚未写入，正在显示父版本的元数据；通过 SMB 写入后才保存本分支自己的版本。","This unwritten branch displays its parent version’s metadata. Writing through SMB creates this branch’s own version.")}</p></Show>
                <section class="zxs-entry-section zxs-file-branches zxs-stack zxs-stack--flush"><h3 class="zxs-row">{t("文件分支","File branches")}</h3><p class="zxs-row">{t("创建时只登记分支并引用父节点，不创建占位文件或复制正文。首次写入后形成自己的版本；切换只浏览历史，设为默认后才更新当前目录的文件引用。","Creating a branch only registers a reference to its parent node, without placeholders or copied content. The first write creates its own version. Switching only browses history; setting the default updates this folder’s file reference.")}</p>
                    <div class="zxs-file-toolbar zxs-flow"><Select label=t("浏览文件分支","Browse file branch") options=options value=value disabled=disabled placeholder=t("请选择本页分支","Choose a branch on this page") on_change=Callback::new(move |branch:String|actions::select_branch(state,branch))/>
                        <button class="zxs-button" data-zxs-set-file-default="true" disabled=move ||state.with(|s|!actions::default_write_allowed(s)||s.files.file_preview.as_ref().is_none_or(|branch|!branch.writable()||branch.head().is_none())||s.files.directory_preview.is_some()||s.files.history_loading||!s.files.file_history.ready()||selected_entry_read_only(&s.files)) on:click=move |_|actions::set_default_file_branch(state)>{t("设为默认分支","Set as default branch")}</button>
                        <Show when=move ||state.with(|s|s.files.file_preview.is_some())><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|actions::return_to_default_file(state)>{t("返回默认视图","Return to default view")}</button></Show>
                        <Show when=move ||can_modify.get()>
                        {move ||rename_branch.get().map(|(file,branch_id,name)|inline::control(state,t("重命名文件分支","Rename file branch"),Some("rename"),inline::Target::RenameFileBranch{file,branch_id,name},disabled))}
                        </Show>
                    </div>
                    <div class="zxs-file-toolbar zxs-flow">
                        <button class="zxs-button" disabled=move ||branch_page_disabled.get()||state.with(|s|s.files.file_history.previous_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,false)>{t("上一页分支","Previous branch page")}</button>
                        <button class="zxs-button" disabled=move ||branch_page_disabled.get()||state.with(|s|s.files.file_history.next_branch_cursor().is_none()) on:click=move |_|actions::page_history_branches(state,true)>{t("下一页分支","Next branch page")}</button>
                    </div>
                    <Show when=move ||can_modify.get()>
                        <div class="zxs-file-toolbar zxs-flow">{branch::control(state,t("从当前内容创建分支","Branch from current content"),branch::Target::CurrentFileVersion,current_branch_disabled)}<button class="zxs-button" disabled=move ||merge_disabled.get() on:click=move |_|merge::start_history(state)>{t("合并分支","Merge branches")}</button></div>
                        {merge::view(state)}
                    </Show>
                </section>
                <section class="zxs-entry-section zxs-entry-version-history zxs-stack" aria-busy=move ||state.with(|s|s.files.history_loading).to_string()><h3 class="zxs-row">{t("版本历史","Version history")}</h3>
                    <div class="zxs-file-toolbar zxs-flow">
                        <button class="zxs-button" disabled=move ||history_page_disabled.get()||state.with(|s|s.files.file_history.previous_cursor().is_none()) on:click=move |_|actions::page_history_nodes(state,false)>{t("上一页历史","Previous history page")}</button>
                        <button class="zxs-button" disabled=move ||history_page_disabled.get()||state.with(|s|s.files.file_history.next_cursor().is_none()) on:click=move |_|actions::page_history_nodes(state,true)>{t("下一页历史","Next history page")}</button>
                        <Show when=move ||state.with(|s|s.files.file_history.current_node_id().is_some_and(|id|!s.files.file_history.nodes().iter().any(|node|node.id()==id)))>
                            <button class="zxs-button" disabled=move ||history_page_disabled.get() on:click=move |_|if let Some(id)=state.with_untracked(|s|s.files.file_history.current_node_id().map(str::to_owned)){actions::anchor_history_node(state,id);}>{t("定位当前节点","Locate current node")}</button>
                        </Show>
                    </div>
                    <Show when=move ||can_modify.get()>{tags::view(state)}</Show>
                    {version_tree::view(state)}
                    {super::content_matches::view(state)}
                    {super::shared_history::view(state)}
                    {super::history::auxiliary_view(state)}
                    {merge::history(state)}
                    <Show when=move ||state.with(|s|!s.files.history_loading&&!s.files.file_history.loaded())><p class="zxs-row" role="status">{t("历史尚未加载。","History has not been loaded.")}</p></Show>
                    <Show when=move ||state.with(|s|s.files.file_history.loaded()&&!s.files.file_history.ready())><p class="zxs-row" role="status">{t("当前历史依据已失效，请刷新后继续。","This history window is out of date. Refresh to continue.")}</p></Show>
                    <Show when=move ||state.with(|s|!s.files.history_loading&&s.files.file_history.ready()&&s.files.file_history.nodes().is_empty())><p class="zxs-row">{t("此历史窗口没有节点。","This history window has no nodes.")}</p></Show>
                </section>
            })}
        </article>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/detail/detail_tests.rs"]
mod detail_tests;
