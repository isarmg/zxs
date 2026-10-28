use super::{
    actions, backup, batch, branch, inline,
    model::{
        breadcrumbs, directory_branch_name, directory_branch_value, selected_directory_branch,
    },
    rows::{Row, visible_rows},
};
use crate::{format, icons::FileIcon, ui::State};
use web_interface::components::{EmptyState, LoadingState, Select, SelectOption};
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    inline::provide_context(state);
    super::history_index::provide(state);
    let slot = Memo::new(move |_| state.with(|s| s.files.slot.clone()));
    view! {
        <section class="zxs-content-stack zxs-file-manager zxs-stack">
            {move ||if slot.get().is_empty(){disk_roots(state).into_any()}else{directory(state).into_any()}}
        </section>
    }
}
#[derive(Debug, PartialEq, Eq)]
enum DiskRoots {
    Loading,
    Empty,
    Disks(Vec<super::model::Disk>),
}

fn disk_roots_projection(model: &super::model::Model) -> DiskRoots {
    if model.loading_disks && !model.disks_loaded {
        DiskRoots::Loading
    } else if model.disks_loaded && model.disks.is_empty() {
        DiskRoots::Empty
    } else {
        DiskRoots::Disks(model.disks.clone())
    }
}

fn disk_roots(state: RwSignal<State>) -> impl IntoView {
    view! {
        <p class="zxs-row">{t("选择一个硬盘目录查看文件。","Choose a disk directory to browse its files.")}</p>
        {move ||{
            let disks = match state.with(|s| disk_roots_projection(&s.files)) {
                DiskRoots::Loading => return view!{<LoadingState/>}.into_any(),
                DiskRoots::Empty => return view!{<EmptyState message=t("没有可访问的硬盘目录。","No disk directories are available.")/>}.into_any(),
                DiskRoots::Disks(disks) => disks,
            };
            view!{<div class="zxs-file-layout zxs-stack zxs-surface"><section class="zxs-content-panel zxs-file-list-panel zxs-frame zxs-stack zxs-stack--flush" aria-label=t("硬盘目录","Disk directories")><div class="zxs-table-scroll">
                <table class="zxs-table zxs-file-table zxs-disk-table">
                    <thead><tr><th scope="col">{t("名称","Name")}</th><th scope="col">{t("访问权限","Access")}</th><th scope="col">{t("可用空间","Available space")}</th><th scope="col">{t("操作","Actions")}</th></tr></thead>
                    <tbody>{disks.into_iter().map(move |disk|{
                        let id=disk.slot_id;let data_slot=id.clone();let row_slot=id.clone();let name_slot=id.clone();let action_slot=id.clone();
                        let online=disk.online;let writable=disk.writable;
                        let available=format::bytes(disk.available_bytes);
                        view!{<tr data-online=online.to_string() on:click=move |_|{if online{actions::open_disk(state,row_slot.clone());}}>
                            <td><button class="zxs-button zxs-file-name zxs-disk-entry" data-slot=data_slot disabled=!online on:click=move |event|{event.stop_propagation();actions::open_disk(state,name_slot.clone());}><FileIcon name="folder"/><span>{id}</span></button></td>
                            <td>{if online{if writable{t("可读写","Read and write")}else{t("只读","Read only")}}else{t("离线","Offline")}}</td>
                            <td>{available}</td>
                            <td><div class="zxs-entry-actions zxs-flow"><button class="zxs-button" disabled=!online on:click=move |event|{event.stop_propagation();actions::open_disk(state,action_slot.clone());}>{t("打开","Open")}</button></div></td>
                        </tr>}
                    }).collect_view()}</tbody>
                </table>
            </div></section></div>}.into_any()
        }}
    }
}
fn directory(state: RwSignal<State>) -> impl IntoView {
    let default_scope = Memo::new(move |_| state.with(|s| s.files.default_branch_scope));
    let rename_branch = Memo::new(move |_| state.with(|s| selected_directory_branch(&s.files)));
    let folder_options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .directory_branches
                .iter()
                .map(|branch| {
                    SelectOption::new(
                        directory_branch_value(branch, s.files.default_branch_scope),
                        directory_branch_name(branch),
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    let folder_value = Signal::derive(move || {
        state.with(|s| {
            s.files
                .directory_branches
                .iter()
                .find(|branch| branch.current())
                .map(|branch| directory_branch_value(branch, s.files.default_branch_scope))
                .unwrap_or_default()
        })
    });
    let folder_disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || !s.files.loaded
                || !s.files.directory_branches_supported
                || s.files.directory_branches.is_empty()
        })
    });
    let create_branch_disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s) || !s.files.loaded || !s.files.directory_branches_supported
        })
    });

    let mutation_disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let batch_disabled = Signal::derive(move || {
        state.with(|s| !actions::write_allowed(s) || !batch::available(&s.files))
    });
    view! {
        <nav class="zxs-breadcrumb zxs-flow" aria-label=t("当前位置","Current location")>
            <button class="zxs-button" on:click=move |_|actions::go_root(state)><FileIcon name="home"/>{t("所有硬盘","All disks")}</button><span aria-hidden="true">"/"</span>
            <button class="zxs-button" on:click=move |_|actions::go_directory(state,String::new())>{move ||state.with(|s|s.files.slot.clone())}</button>
            {move ||breadcrumbs(&state.with(|s|s.files.directory.clone())).into_iter().map(move |(name,path)|view!{<span aria-hidden="true">"/"</span><button class="zxs-button" on:click=move |_|actions::go_directory(state,path.clone())>{name}</button>}).collect_view()}
        </nav>
        <div class="zxs-file-toolbar zxs-flow">
            {inline::control(state,t("新建文件夹","New folder"),Some("create"),inline::Target::CreateDirectory,mutation_disabled)}
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Move,None)>{t("移动","Move")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Copy,None)>{t("复制","Copy")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Rename,None)><FileIcon name="rename"/>{t("重命名","Rename")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Delete,None)><FileIcon name="delete"/>{t("删除","Delete")}</button>
            {branch::control(state,t("创建目录分支","Create folder branch"),branch::Target::CurrentDirectory,create_branch_disabled)}
            <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::start_current(state)>{t("备份设置","Backup settings")}</button>
            <label class="zxs-file-search"><span class="zxs-visually-hidden">{t("搜索当前文件夹","Search this folder")}</span><input class="zxs-input zxs-input--compact" type="search" aria-label=t("搜索当前文件夹","Search this folder") placeholder=t("搜索当前文件夹","Search this folder") prop:value=move ||state.with(|s|s.files.query.clone()) on:input=move |event|actions::search(state,event_target_value(&event))/></label>
        </div>
        <div class="zxs-file-toolbar zxs-flow zxs-surface-end">{move ||view!{<Select label=if default_scope.get(){t("当前主分支（SMB 默认）","Current main branch (SMB default)")}else{t("当前目录分支","Current folder branch")} options=folder_options value=folder_value disabled=folder_disabled placeholder=if default_scope.get(){t("主版本","Main")}else{t("尚无目录分支","No folder branches")} on_change=Callback::new(move |value:String|{if state.with_untracked(|s|s.files.default_branch_scope){actions::select_default_branch(state,value)}else{let path=state.with_untracked(|s|s.files.directory.clone());let _=actions::directory_branch(state,path,value,true);}})/>}}
            {move ||rename_branch.get().map(|(repository,branch_id,name)|inline::control(state,t("重命名分支","Rename branch"),Some("rename"),inline::Target::RenameDirectoryBranch{repository,branch_id,name},folder_disabled))}
            <p class="zxs-row">{move ||if default_scope.get(){t("恢复所选文件分支组合，并作为 SMB 默认视图。","Restores the selected file branch combination as the default SMB view.")}else{t("恢复此目录保存的文件分支组合，不改变目录外文件。","Restores this folder’s saved file branch combination without changing files outside it.")}}</p></div>
        <Show when=move ||state.with(|s|s.files.loaded&&s.files.writable&&!s.files.directory_branches_supported)><p class="zxs-row">{t("请选择具体文件夹后管理目录分支；此硬盘根目录包含多个已有的文件目录。","Select a specific folder to manage folder branches; this disk root contains multiple existing file namespaces.")}</p></Show>
        <Show when=move ||state.with(|s|s.files.loaded&&!s.files.writable)><p class="zxs-row">{t("此硬盘目录当前只读。","This disk directory is read only.")}</p></Show>
        {backup::view(state)}
        {batch::view(state)}
        <div class="zxs-file-layout zxs-stack zxs-surface"><section class="zxs-content-panel zxs-file-list-panel zxs-frame zxs-stack zxs-stack--flush">{file_table(state)}</section></div>
    }
}
fn file_table(state: RwSignal<State>) -> impl IntoView {
    let rows = visible_rows(state);
    let disabled = Memo::new(move |_| state.with(|s| !actions::write_allowed(s)));
    let selected =
        Memo::new(move |_| state.with(|s| s.files.selected.as_ref().map(super::model::entry_key)));
    view! {
        <div aria-busy=move ||state.with(|s|s.files.loading).to_string()>
            <Show when=move ||state.with(|s|s.files.loading&&!s.files.loaded)><LoadingState/></Show>
            <div class="zxs-table-scroll"><table class="zxs-table zxs-file-table" aria-label=t("文件列表：每页最多 200 项","Files: up to 200 entries per page")><thead><tr><th scope="col"><label class="zxs-file-check zxs-flow"><input type="checkbox" aria-label=t("选择本页全部条目（最多 200 项）","Select all entries on this page (up to 200)") disabled=move ||disabled.get() prop:checked=move ||rows.with(|rows|!rows.is_empty()&&state.with(|s|rows.iter().all(|row|s.files.checked_entries.contains(&row.check_id)))) on:change=move |event|{
                let checked=event_target_checked(&event);
                rows.with_untracked(|rows|state.update(|s|{for row in rows {if row.current_entry(s).is_some(){if checked{s.files.checked_entries.insert(row.check_id.clone());}else{s.files.checked_entries.remove(&row.check_id);}}}}));
            }/><span>{t("名称","Name")}</span></label></th><th scope="col">{t("类型","Type")}</th><th scope="col">{t("大小","Size")}</th><th scope="col">{t("操作","Actions")}</th></tr></thead>
                <tbody><ForSnapshot each=move ||rows.get() key=|row|row.key.clone() children=move |row|file_row(state,row,disabled,selected)/></tbody>
            </table></div>
            <nav class="zxs-file-pagination zxs-flow" aria-label=t("文件列表分页","File list pages")>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.loading||actions::pending(s)||s.files.previous_cursor.is_none()) on:click=move |_|actions::change_page(state,false)>{t("上一页","Previous page")}</button>
                <span role="status" aria-live="polite">{move ||state.with(|s|if s.files.loading{t("正在读取…","Loading…")}else if s.files.listing_failed{t("读取失败，请重试。","Loading failed. Retry the page.")}else{format!("{} {} · {} {} · {}",t("本页","This page"),s.files.entries.len(),t("匹配总数","Total matches"),s.files.total_entries,t("仅选择本页","Selection is page-local"))})}</span>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.loading||actions::pending(s)||s.files.next_cursor.is_none()) on:click=move |_|actions::change_page(state,true)>{t("下一页","Next page")}</button>
                <Show when=move ||state.with(|s|s.files.listing_failed)><button class="zxs-button" on:click=move |_|actions::load_entries(state)>{t("重试","Retry")}</button></Show>
            </nav>
            <Show when=move ||state.with(|s|s.files.loaded&&s.files.entries.is_empty()&&s.files.query.is_empty())><p class="zxs-file-empty">{t("这个文件夹是空的。","This folder is empty.")}</p></Show>
            <Show when=move ||rows.with(|rows|rows.is_empty())&&state.with(|s|s.files.loaded&&!s.files.query.is_empty())><p class="zxs-file-empty">{t("没有匹配的文件。","No files match your search.")}</p></Show>
        </div>
    }
}

fn activate_row(
    state: RwSignal<State>,
    row: ReadSignal<std::sync::Arc<Row>>,
    open_directory: bool,
) {
    let entry = row.with_untracked(|row| state.with_untracked(|s| row.current_entry(s).cloned()));
    if let Some(entry) = entry {
        if open_directory && !super::model::file_kind(&entry) {
            actions::go_directory(state, entry.path().to_owned());
        } else {
            actions::select_entry(state, entry);
        }
    }
}

fn file_row(
    state: RwSignal<State>,
    row: ReadSignal<std::sync::Arc<Row>>,
    disabled: Memo<bool>,
    selected: Memo<Option<String>>,
) -> impl IntoView {
    let is_selected = Memo::new(move |_| {
        row.with(|row| selected.with(|selected| selected.as_ref() == Some(&row.selection_id)))
    });
    let checked = Memo::new(move |_| {
        row.with(|row| state.with(|s| s.files.checked_entries.contains(&row.check_id)))
    });
    view! {
        <tr class:is-selected=move ||is_selected.get() aria-selected=move ||is_selected.get().to_string() on:click=move |_|activate_row(state,row,false)>
            <td><div class="zxs-file-select-name zxs-flow"><input class="zxs-entry-checkbox" type="checkbox" aria-label=move ||row.with(|row|format!("{} {}",t("选择","Select"),row.name)) disabled=move ||disabled.get() prop:checked=move ||checked.get() on:click=move |event|event.stop_propagation() on:change=move |event|{
                let checked=event_target_checked(&event);
                row.with_untracked(|row|state.update(|s|{if row.current_entry(s).is_some(){if checked{s.files.checked_entries.insert(row.check_id.clone());}else{s.files.checked_entries.remove(&row.check_id);}}}));
            }/><button class="zxs-button zxs-file-name" title=move ||row.with(|row|row.name.clone()) on:click=move |event|{event.stop_propagation();activate_row(state,row,true)}>{move ||row.with(|row|view!{<FileIcon name=if row.directory{"folder"}else{"file"}/>})}<span>{move ||row.with(|row|row.name.clone())}</span></button></div></td>
            <td>{move ||row.with(|row|if row.directory{t("文件夹","Folder")}else{t("文件","File")})}</td><td>{move ||row.with(|row|if row.directory{"—".into()}else{row.size.map(format::bytes).unwrap_or_else(||"—".into())})}</td>
            <td><div class="zxs-entry-actions zxs-flow"><button class="zxs-button" on:click=move |event|{event.stop_propagation();activate_row(state,row,false)}>{t("详情","Details")}</button></div></td>
        </tr>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/view/tests.rs"]
mod tests;
