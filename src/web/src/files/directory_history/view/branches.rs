//! Branch operations use admitted history identities, not labels or a preview.
use super::*;
use crate::files::inline;
use storage_contract::directory_history::DirectoryHistoryBranch;

#[derive(Clone, Copy)]
struct Operations {
    state: RwSignal<State>,
    model: RwSignal<Model>,
    target: StoredValue<Target>,
    disabled: Signal<bool>,
    rename_disabled: Signal<bool>,
    working_scope: bool,
    working_heads: Memo<BTreeMap<String, String>>,
    working_disabled: Signal<bool>,
}

pub(super) fn view(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    target: StoredValue<Target>,
    disabled: Signal<bool>,
) -> impl IntoView {
    let rename_disabled = Signal::derive(move || {
        disabled.get()
            || model.with(|model| model.error.is_some())
            || !state.with(|state| target.with_value(|target| target.can_manage_branches(state)))
    });
    let working_scope = target.with_value(|target| {
        matches!(
            &target.scope,
            Scope::CurrentDirectory {
                initialized: true,
                ..
            }
        )
    });
    // One index for the page, not a complete listing scan for each graph row.
    // These are the independently admitted working heads, never commit heads.
    let working_heads = Memo::new(move |_| {
        state.with(|state| {
            state
                .files
                .directory_branches
                .iter()
                .map(|branch| (branch.id().to_owned(), branch.head_tree_id().to_owned()))
                .collect::<BTreeMap<_, _>>()
        })
    });
    let working_disabled = Signal::derive(move || {
        disabled.get()
            || model.with(|model| model.error.is_some())
            || !state.with(|state| {
                target.with_value(|target| target.current_repository(state))
                    && !crate::files::actions::pending(state)
                    && state.files.history_editors == 0
            })
    });
    let operations = Operations {
        state,
        model,
        target,
        disabled,
        rename_disabled,
        working_scope,
        working_heads,
        working_disabled,
    };
    view! {
        <section class="zxs-stack zxs-directory-history-branches">
            <p class="zxs-row" role="heading" {web_interface::attr::custom::custom_attribute("aria-level", "4")}>{t("目录分支", "Folder branches")}</p>
            <p class="zxs-row">{t("已提交快照只读。当前目录可另外浏览分支已保存的工作内容，包括未提交更改；仍须在该目录详情中明确设为默认，才会更新默认读取和已启用的 SMB 共享。只有当前目录的可写详情允许改名。", "Committed snapshots are read-only. From the current folder, you can separately browse a branch's saved work, including uncommitted changes. Set it as default explicitly in that folder's Details to update default reads and enabled SMB shares. Renaming requires the current folder's writable Details.")}</p>
            <Show when=move ||working_scope>
                <p class="zxs-row">{t("工作内容必须与当前目录已读取的分支工作树一致。若入口不可用，请先结束编辑并刷新目录依据。", "Working content must match the branch working tree read for this folder. If unavailable, finish editing and refresh the folder basis.")}</p>
                <button type="button" class="zxs-button" data-refresh-directory-working-basis=""
                    disabled=move ||state.with(|state|!target.with_value(|target|target.current_repository(state))||crate::files::actions::pending(state)||state.files.history_editors!=0)
                    on:click=move |_|{
                        if state.with_untracked(|state|target.with_value(|target|target.current_repository(state))&&!crate::files::actions::pending(state)&&state.files.history_editors==0) {
                            crate::files::actions::load_entries(state);
                        }
                    }>{t("刷新目录工作依据", "Refresh folder working basis")}</button>
            </Show>
            <For
                each=move || model.with(|model| model.branches.as_ref().map(|page| page.branches.clone()).unwrap_or_default())
                key=|branch| (
                    branch.directory_branch_id.clone(),
                    branch.name.clone(),
                    branch.head_tree_id.clone(),
                    branch.committed_tree_id.clone(),
                    branch.staged,
                    branch.materialized,
                    branch.current,
                )
                children=move |branch| row(operations, branch)
            />
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||disabled.get()||model.with(|model|model.branches.as_ref().is_none_or(|page|page.page.previous_cursor.is_none()))
                    on:click=move |_|page(state,model,target,true,false)>{t("上一页目录分支", "Previous folder branch page")}</button>
                <button class="zxs-button" disabled=move ||disabled.get()||model.with(|model|model.branches.as_ref().is_none_or(|page|page.page.next_cursor.is_none()))
                    on:click=move |_|page(state,model,target,true,true)>{t("下一页目录分支", "Next folder branch page")}</button>
            </div>
        </section>
    }
}

fn row(operations: Operations, branch: DirectoryHistoryBranch) -> impl IntoView {
    let Operations {
        state,
        model,
        target,
        disabled,
        rename_disabled,
        working_scope,
        working_heads,
        working_disabled,
    } = operations;
    let tree = StoredValue::new(branch.committed_tree_id);
    let working_tree = StoredValue::new(branch.head_tree_id);
    let branch_id = StoredValue::new(branch.directory_branch_id);
    let working_matches = Memo::new(move |_| {
        working_heads.with(|heads| {
            branch_id.with_value(|id| working_tree.with_value(|tree| heads.get(id) == Some(tree)))
        })
    });
    let rename = (!branch.current).then(|| {
        inline::control(
            state,
            t("重命名分支", "Rename branch"),
            Some("rename"),
            inline::Target::RenameDirectoryBranch {
                repository: target.with_value(|target| target.repository.clone()),
                branch_id: branch_id.get_value(),
                name: branch.name.clone(),
            },
            rename_disabled,
        )
    });
    view! {
        <div class="zxs-file-toolbar zxs-flow zxs-hover-row" data-directory-branch-id=branch_id.get_value()>
            <span class="zxs-row">{branch.name}</span>
            {branch.staged.then(||view!{<span class="zxs-row">{t("有待提交更改", "Staged changes")}</span>})}
            {branch.current.then(||view!{<span class="zxs-row">{t("默认", "Default")}</span>})}
            {(!branch.materialized).then(||view!{<span class="zxs-row">{t("尚未写入，引用父节点", "Unwritten, references parent node")}</span>})}
            <button class="zxs-button" disabled=move ||disabled.get()||tree.get_value().is_none()
                on:click=move |_|{
                    if disabled.get_untracked() { return; }
                    if let Some(id)=tree.get_value() {
                        load(state,model,target,Read::Nodes{cursor:None,anchor:Some(id)});
                    }
                }>{t("定位分支节点", "Locate branch node")}</button>
            <button class="zxs-button" data-directory-branch-snapshot=branch_id.get_value()
                disabled=move ||disabled.get()||tree.get_value().is_none()||state.with(|state|state.files.loading||crate::files::actions::pending(state))
                on:click=move |_|{
                    if disabled.get_untracked() { return; }
                    let path=target.with_value(|target|state.with_untracked(|state|target.current(state)).then(||target.path.clone()));
                    if let (Some(path),Some(tree))=(path,tree.get_value()) {
                        crate::files::actions::preview_directory_snapshot(state,path,branch_id.get_value(),tree);
                    }
                }>{t("浏览此分支已提交快照", "Browse this branch's committed snapshot")}</button>
            {working_scope.then(move ||view!{
                <button type="button" class="zxs-button" data-directory-branch-working=branch_id.get_value()
                    disabled=move ||working_disabled.get()||!working_matches.get()
                    on:click=move |_|{
                        if working_disabled.get_untracked()||!working_matches.get_untracked() { return; }
                        let current=state.with_untracked(|state|{
                            target.with_value(|target|target.current_repository(state))
                                && state.files.directory_branches.iter().any(|branch|{
                                    branch.id()==branch_id.get_value()
                                        && branch.head_tree_id()==working_tree.get_value()
                                })
                        });
                        if current {
                            crate::files::actions::preview_directory_branch(state,branch_id.get_value());
                            crate::ui::navigate(state,crate::ui::Page::Files);
                        }
                    }>{t("浏览此分支工作内容", "Browse this branch's working contents")}</button>
            })}
            {rename}
        </div>
    }
}
