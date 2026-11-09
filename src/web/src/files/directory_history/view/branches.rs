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
    let operations = Operations {
        state,
        model,
        target,
        disabled,
        rename_disabled,
    };
    view! {
        <section class="zxs-stack zxs-directory-history-branches">
            <p class="zxs-row" role="heading" {web_interface::attr::custom::custom_attribute("aria-level", "4")}>{t("目录分支", "Folder branches")}</p>
            <p class="zxs-row">{t("定位或浏览分支最后一次真实提交，不会切换当前工作内容或默认分支。只有当前目录的可写详情允许改名；默认分支在目录控件中改名。", "Locate or browse a branch's last real commit without switching working contents or the default branch. Renaming requires the current folder's writable Details; use its folder controls to rename the default branch.")}</p>
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
    } = operations;
    let tree = StoredValue::new(branch.committed_tree_id);
    let branch_id = StoredValue::new(branch.directory_branch_id);
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
            {rename}
        </div>
    }
}
