//! Actual directory branch and staging controls, owned by current-folder Details.
use super::super::model::{
    directory_branch_name, directory_branch_value, selected_directory_branch,
};
use super::super::{actions, backup, inline};
use crate::ui::State;
use web_interface::components::{Select, SelectOption};
use web_interface::{i18n::t, prelude::*};

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let default_disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::default_write_allowed(s)
                || !s.files.loaded
                || !s.files.directory_branches_supported
                || s.files.directory_preview.as_ref().is_none_or(|preview| {
                    preview.branch().is_none() || !preview.at_directory(&s.files.directory)
                })
        })
    });
    let rename_branch = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .directory_branches_supported
                .then(|| selected_directory_branch(&s.files))
                .flatten()
        })
    });
    let folder_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .directory_branches
                .iter()
                .map(|branch| {
                    SelectOption::new(
                        directory_branch_value(branch),
                        directory_branch_name(branch),
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    let folder_value = Signal::derive(move || {
        state.with(|s| {
            super::super::model::viewed_directory_branch(&s.files)
                .map(|branch| branch.id().to_owned())
                .unwrap_or_default()
        })
    });
    let folder_disabled = Signal::derive(move || {
        state.with(|s| {
            !s.session_active()
                || actions::pending(s)
                || s.files.loading
                || !s.files.loaded
                || !s.files.directory_branches_supported
                || s.files.directory_branches.is_empty()
        })
    });
    let mutation_disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let navigation_disabled = Signal::derive(move || {
        state.with(|s| !s.session_active() || s.files.loading || actions::pending(s))
    });
    view! {
        <article class="zxs-content-panel zxs-entry-content zxs-frame zxs-stack" data-zxs-current-directory-controls="true" aria-label=t("当前目录详情", "Current folder details")>
            <h2 class="zxs-row">{t("当前目录详情", "Current folder details")}</h2>
            <nav class="zxs-breadcrumb zxs-flow" aria-label=t("目录仓库位置", "Folder repository location")>
                <button class="zxs-button" disabled=move ||navigation_disabled.get()
                    on:click=move |_|actions::go_directory_details(state,String::new())>{move ||state.with(|s|s.files.disks.iter().find(|disk|disk.slot_id==s.files.slot).map(|disk|disk.name.clone()))}</button>
                {move ||super::super::model::breadcrumbs(&state.with(|s|s.files.directory.clone())).into_iter().map(move |(name,path)|view!{
                    <span aria-hidden="true">"/"</span>
                    <button class="zxs-button" disabled=move ||navigation_disabled.get()
                        on:click=move |_|actions::go_directory_details(state,path.clone())>{name}</button>
                }).collect_view()}
            </nav>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" data-zxs-directory-files="" disabled=move ||navigation_disabled.get()
                    on:click=move |_|crate::ui::navigate(state,crate::ui::Page::Files)>{t("查看本目录文件","View this folder’s files")}</button>
                <button class="zxs-button" data-directory-git-refresh="" disabled=move ||navigation_disabled.get()
                    on:click=move |_|actions::refresh_directory(state)>{t("刷新目录与仓库状态","Refresh folder and repository status")}</button>
                <Show when=move ||state.with(|s|s.files.directory_preview.is_some())>
                    <button class="zxs-button" data-open-preview-origin="" disabled=move ||navigation_disabled.get()
                        on:click=move |_|actions::open_preview_origin(state)>{t("返回原目录详情", "Return to source folder Details")}</button>
                </Show>
            </div>
            <p class="zxs-row">{move ||state.with(|s|super::super::model::join_path(&s.files.slot,&s.files.directory))}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||!actions::write_allowed(s)) on:click=move |_|super::super::placement::start_current(state)>{t("主盘规则","Primary placement")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::recursive::start_current(state)>{t("逐级备份","Recursive backups")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::start_current(state)>{t("自定义备份前缀","Custom backup prefixes")}</button>
            </div>
        <div class="zxs-file-toolbar zxs-flow zxs-surface-end">
            <Select label=t("浏览目录分支","Browse folder branch") options=folder_options value=folder_value disabled=folder_disabled placeholder=t("尚无目录分支","No folder branches") on_change=Callback::new(move |value:String|actions::preview_directory_branch(state,value))/>
            <button class="zxs-button" data-zxs-set-directory-default="true" disabled=move ||default_disabled.get() on:click=move |_|if let Some(id)=state.with_untracked(|s|super::super::model::viewed_directory_branch(&s.files).map(|branch|branch.id().to_owned())){actions::select_default_branch(state,id);}>{t("设为默认分支","Set as default branch")}</button>
            {move ||rename_branch.get().map(|(repository,branch_id,name)|inline::control(state,t("重命名分支","Rename branch"),Some("rename"),inline::Target::RenameDirectoryBranch{repository,branch_id,name},mutation_disabled))}
            <Show when=move ||state.with(|s|s.files.directory_preview.is_some())><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|actions::return_to_default(state)>{t("返回默认视图","Return to default view")}</button></Show>
            <p class="zxs-row">{t("切换仅浏览分支，不改变默认分支。设为默认后才更新默认读取，已启用的 SMB 共享使用该视图；目录外文件保持不变。","Switching only browses a branch. Set it as default to update default reads, including enabled SMB shares; files outside the folder stay unchanged.")}</p>
        </div>
            <p class="zxs-row">{t("初始化、暂存、提交及 checkout 都针对这里显示的目录。目录合并尚未提供；不会调用旧整树历史分支或手工合并连线。", "Initialization, staging, commits and checkout apply to the folder shown here. Folder merging is not available; legacy whole-history branches and manual merge links are not used.")}</p>
            {super::super::staging::view(state)}
            {backup::view(state)}
            {super::super::placement::view(state)}
        </article>
    }
}
