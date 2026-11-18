//! Ordinary folder navigation, placement and backup controls.
use super::super::{actions, backup, inline};
use crate::ui::State;
use web_interface::{i18n::t, prelude::*};

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let mutation_disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let navigation_disabled = Signal::derive(move || {
        state.with(|s| !s.session_active() || s.files.loading || actions::pending(s))
    });
    view! {
        <article class="zxs-content-panel zxs-entry-content zxs-frame zxs-stack" data-zxs-current-directory-controls="true" aria-label=t("当前目录详情", "Current folder details")>
            <h2 class="zxs-row">{t("当前目录详情", "Current folder details")}</h2>
            <nav class="zxs-breadcrumb zxs-flow" aria-label=t("文件夹位置", "Folder location")>
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
                {inline::control(state,t("新建文件夹","New folder"),Some("create"),inline::Target::CreateDirectory,mutation_disabled)}
                <button class="zxs-button" data-zxs-folder-refresh="" disabled=move ||navigation_disabled.get()
                    on:click=move |_|actions::refresh_directory(state)>{t("刷新文件夹","Refresh folder")}</button>
            </div>
            {inline::first_folder_hint(state)}
            <p class="zxs-row">{move ||state.with(|s|super::super::model::join_path(&s.files.slot,&s.files.directory))}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||!actions::write_allowed(s)) on:click=move |_|super::super::placement::start_current(state)>{t("主盘规则","Primary placement")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::recursive::start_current(state)>{t("逐级备份","Recursive backups")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::start_current(state)>{t("自定义备份前缀","Custom backup prefixes")}</button>
            </div>
            {backup::view(state)}
            {super::super::placement::view(state)}
        </article>
    }
}
