use super::{
    actions, backup, batch, inline,
    model::breadcrumbs,
    rows::{Row, visible_rows},
};
use crate::{format, icons::FileIcon, ui::State};
use web_interface::components::EmptyState;
use web_interface::{i18n::t, prelude::*};

mod directory_preview;
mod disk_read;
mod disk_status;
use disk_read::DiskRead;

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    inline::provide_context(state);
    super::history_index::provide(state);
    let slot = Memo::new(move |_| state.with(|s| s.files.slot.clone()));
    view! {
        <section class="zxs-content-stack zxs-file-manager zxs-stack" aria-busy=move ||state.with(|s|s.files.loading||s.files.loading_disks).to_string()>
            {super::review::recovery_view(state)}
            {super::deletion::view(state)}
            {move ||if slot.get().is_empty(){disk_roots(state).into_any()}else{directory(state).into_any()}}
        </section>
    }
}
#[derive(Debug, PartialEq, Eq)]
enum DiskRoots {
    Empty,
    Disks(Vec<super::model::Disk>),
}

fn disk_roots_projection(model: &super::model::Model) -> DiskRoots {
    if model.disks_loaded && model.disks.is_empty() {
        DiskRoots::Empty
    } else {
        DiskRoots::Disks(model.disks.clone())
    }
}

fn disk_read_projection(model: &super::model::Model) -> DiskRead {
    match (
        model.loading_disks,
        model.disks_loaded,
        model.disks.is_empty(),
    ) {
        (true, _, true) => DiskRead::Reading,
        (true, _, false) => DiskRead::RefreshingCached,
        (false, true, _) => DiskRead::Current,
        (false, false, true) => DiskRead::Unconfirmed,
        (false, false, false) => DiskRead::UnconfirmedCached,
    }
}

fn disk_available_space(available: Option<u64>) -> String {
    available
        .map(format::bytes)
        .unwrap_or_else(|| t("不可用", "Unavailable"))
}

fn disk_observation_message(
    issue: Option<storage_contract::DiskObservationIssue>,
) -> Option<String> {
    issue.map(|issue| match issue {
        storage_contract::DiskObservationIssue::Seal(_) => {
            t("磁盘身份观测失败", "Disk identity observation failed")
        }
        storage_contract::DiskObservationIssue::Capacity(_) => {
            t("容量观测失败", "Capacity observation failed")
        }
    })
}

// The interface's two supported languages use LTR scrolling. CSS supplies the
// live scrollport width; one inherited offset anchors every row decoration
// without changing native table scroll, focus or hit testing. The view owns
// this listener, so it is removed with the scroll container.
fn sync_row_viewport(event: web_sys::Event) {
    let scroll = event_target::<web_sys::HtmlElement>(&event);
    // The stable web-sys getter returns i32, while CSSOM scrollLeft is a double.
    // Read the native number so high-DPI/zoomed scrolling keeps subpixel offsets.
    let left = js_sys::Reflect::get(scroll.as_ref(), &"scrollLeft".into())
        .expect("invariant: an element exposes its native scrollLeft property")
        .as_f64()
        .expect("invariant: CSSOM scrollLeft is a number");
    scroll
        .style()
        .set_property("--zxs-file-scroll-left", &format!("{left}px"))
        .expect("invariant: a pixel scroll offset is a valid CSS custom property");
}

fn disk_roots(state: RwSignal<State>) -> impl IntoView {
    let read = Memo::new(move |_| state.with(|s| disk_read_projection(&s.files)));
    view! {
        <p class="zxs-row">{t("选择一个硬盘目录查看文件。","Choose a disk directory to browse its files.")}</p>
        <p class="zxs-row">{t("备份盘只在 Web 中只读浏览，不出现在 SMB 的磁盘列表。目录模式不代表当前写入授权，实际操作仍由服务端校验。","Backup disks are browsed read-only in Web and do not appear in the SMB disk list. Directory mode is not current write authority; the server checks each operation.")}</p>
        {move || read.get().message().map(|message| view!{
            <div class="zxs-stack" data-disk-list-read=move ||read.get().marker()>
                <p class="zxs-row" role="status">{message}</p>
                <Show when=move ||read.get().can_retry()>
                    <div class="zxs-row"><button class="zxs-button" data-disk-list-retry=""
                        disabled=move ||state.with(|s|!s.session_active()||s.files.loading_disks)
                        on:click=move |_|actions::load_disks(state)>{t("重新读取","Read again")}</button></div>
                </Show>
            </div>
        })}
        {move ||{
            let disks = match state.with(|s| disk_roots_projection(&s.files)) {
                DiskRoots::Empty => return view!{<EmptyState message=t("没有可访问的硬盘目录。","No disk directories are available.")/>}.into_any(),
                DiskRoots::Disks(disks) => disks,
            };
            view!{<div class="zxs-file-layout zxs-stack zxs-surface"><section class="zxs-content-panel zxs-file-list-panel zxs-frame zxs-stack zxs-stack--flush" aria-label=t("硬盘目录","Disk directories")><div class="zxs-table-scroll" {web_interface::ev::on(web_interface::ev::Custom::new("scroll"), sync_row_viewport)}>
                <table class="zxs-table zxs-file-table zxs-disk-table">
                    <thead><tr><th scope="col"><span class="zxs-file-row-decoration" aria-hidden="true"></span>{t("名称","Name")}</th><th scope="col">{t("层级","Level")}</th><th scope="col">{t("目录模式","Directory mode")}</th><th scope="col">{t("观测状态","Observed status")}</th><th scope="col">{t("可用空间","Available space")}</th><th scope="col">{t("操作","Actions")}</th></tr></thead>
                    <tbody>{disks.into_iter().map(move |disk|disk_row(state,disk)).collect_view()}</tbody>
                </table>
            </div></section></div>}.into_any()
        }}
    }
}

fn disk_row(state: RwSignal<State>, disk: super::model::Disk) -> impl IntoView {
    let level = disk_status::level(disk.level);
    let access = disk_status::access(disk.writable);
    let status = disk_status::observation(&disk);
    let available = disk_available_space(disk.available_bytes);
    let observation = disk_observation_message(disk.observation_failure);
    let row_slot = disk.slot_id.clone();
    let name_slot = disk.slot_id.clone();
    let action_slot = disk.slot_id.clone();
    let online = disk.online;
    view! {
        <tr data-online=online.to_string() data-disk-level=disk.level.to_string()
            on:click=move |_| if online { actions::open_disk(state, row_slot.clone()); }>
            <td>
                <span class="zxs-file-row-decoration" aria-hidden="true"><span class="zxs-file-row-surface" aria-hidden="true"></span></span>
                <button class="zxs-button zxs-file-name zxs-disk-entry" data-slot=disk.slot_id disabled=!online
                    on:click=move |event| {
                        event.stop_propagation();
                        actions::open_disk(state, name_slot.clone());
                    }>
                    <FileIcon name="folder"/><span>{disk.name}</span>
                </button>
            </td>
            <td>{level}</td><td>{access}</td><td>{status}</td>
            <td title=observation>{available}</td>
            <td><div class="zxs-entry-actions zxs-flow">
                <button class="zxs-button" disabled=!online on:click=move |event| {
                    event.stop_propagation();
                    actions::open_disk(state, action_slot.clone());
                }>{t("打开","Open")}</button>
            </div></td>
        </tr>
    }
}
fn directory(state: RwSignal<State>) -> impl IntoView {
    let mutation_disabled = Signal::derive(move || state.with(|s| !actions::write_allowed(s)));
    let batch_disabled = Signal::derive(move || {
        state.with(|s| !actions::write_allowed(s) || !batch::available(&s.files))
    });
    view! {
        <nav class="zxs-breadcrumb zxs-flow" aria-label=t("当前位置","Current location")>
            <button class="zxs-button" on:click=move |_|actions::go_root(state)><FileIcon name="home"/>{t("所有硬盘","All disks")}</button><span aria-hidden="true">"/"</span>
            <button class="zxs-button" on:click=move |_|actions::go_directory(state,String::new())>{move ||state.with(|s|s.files.disks.iter().find(|disk|disk.slot_id==s.files.slot).map(|disk|disk.name.clone()))}</button>
            {move ||breadcrumbs(&state.with(|s|s.files.directory.clone())).into_iter().map(move |(name,path)|view!{<span aria-hidden="true">"/"</span><button class="zxs-button" on:click=move |_|actions::go_directory(state,path.clone())>{name}</button>}).collect_view()}
        </nav>
        <div class="zxs-file-toolbar zxs-flow">
            <button class="zxs-button" data-zxs-refresh-directory="" disabled=move ||state.with(|s|!s.session_active()||s.files.loading||actions::pending(s)) on:click=move |_|actions::refresh_directory(state)>{t("刷新文件夹","Refresh folder")}</button>
            {inline::control(state,t("新建文件夹","New folder"),Some("create"),inline::Target::CreateDirectory,mutation_disabled)}
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Move,None)>{t("移动","Move")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Copy,None)>{t("复制","Copy")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get()||state.with(|s|!batch::separation_available(&s.files)) on:click=move |_|batch::start(state,batch::Kind::Separate,None)>{t("分离为独立文件","Separate file")}</button>
            <button class="zxs-button" disabled=move ||batch_disabled.get() on:click=move |_|batch::start(state,batch::Kind::Rename,None)><FileIcon name="rename"/>{t("重命名","Rename")}</button>
            <button class="zxs-button" title=t("单个当前文件，或当前目录仓库本页的 2–100 个文件；整批保留一个原操作，目录项不可选", "One current file, or 2–100 files on this folder repository page; one original operation per batch, no folder entries") disabled=move ||!state.with(super::deletion::available) on:click=move |_|batch::start(state,batch::Kind::Delete,None)><FileIcon name="delete"/>{t("删除","Delete")}</button>
            <button class="zxs-button" data-zxs-delete-selected-directory="" title=t("当前已初始化所有者下的一个普通子目录，包含空目录；不支持仓库本身或多个目录", "One ordinary child of the currently initialized owner, including an empty folder; not the repository itself or multiple folders") disabled=move ||!state.with(super::deletion::directory_available) on:click=move |_|super::deletion::start_directory(state,None)>{t("软删除整个目录…","Soft-delete whole folder…")}</button>
            <button class="zxs-button" data-zxs-current-directory-details="true" disabled=move ||state.with(|s|!s.session_active()||actions::pending(s)) on:click=move |_|super::detail::open_current_directory(state)>{t("当前目录详情","Current folder details")}</button>
            <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||!actions::write_allowed(s)) on:click=move |_|super::placement::start_current(state)>{t("主盘规则","Primary placement")}</button>
            <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::recursive::start_current(state)>{t("逐级备份","Recursive backups")}</button>
            <button class="zxs-button" disabled=move ||state.with(|s|s.files.repository_id.is_empty()||actions::pending(s)) on:click=move |_|backup::start_current(state)>{t("自定义备份前缀","Custom backup prefixes")}</button>
            <label class="zxs-file-search zxs-form-grid">
                <input class="zxs-input zxs-input--compact" type="search"
                    aria-label=t("搜索当前文件夹","Search this folder")
                    placeholder=t("搜索当前文件夹","Search this folder")
                    prop:value=move ||state.with(|s|s.files.query_draft.clone())
                    on:input=move |event|actions::edit_search(state,event_target_value(&event))
                    on:keydown=move |event:web_sys::KeyboardEvent|{
                        if event.key()=="Enter"&&!event.is_composing(){
                            event.prevent_default();
                            if !event.repeat(){
                                let query=state.with_untracked(|s|s.files.query_draft.clone());
                                actions::search(state,query);
                            }
                        }
                    }/>
            </label>
        </div>

        {directory_preview::view(state)}
        <Show when=move ||state.with(|s|s.files.checked_entries.len()>1)>
            <p class="zxs-row">{t(
                "多选软删除仅支持当前已初始化目录工作分支内本页的 2–100 个文件，不包含目录或未加载项。同一稳定文件的全部路径都会隐藏；整批只保留一个原操作，恢复时逐个核对准确 D。",
                "Multi-file soft deletion supports 2–100 files on this page in the current initialized folder workspace, excluding folders and unloaded entries. All paths of each stable file become hidden. One original operation owns the batch; restoration checks each exact D individually.",
            )}</p>
        </Show>
        {super::relocation::controls(state)}
        <Show when=move ||state.with(|s|s.files.loaded&&s.files.writable&&!s.files.directory_branches_supported)><p class="zxs-row">{t("目录分支管理需要先手动初始化当前目录的独立仓库。","Initialize this directory as an independent repository before managing its branches.")}</p></Show>
        <Show when=move ||state.with(|s|s.files.loaded&&!s.files.writable&&s.files.directory_preview.is_none())><p class="zxs-row">{t("此硬盘目录当前只读。","This disk directory is read only.")}</p></Show>
        {backup::view(state)}
        {super::placement::view(state)}
        {batch::view(state)}
        <div class="zxs-file-layout zxs-stack zxs-surface"><section class="zxs-content-panel zxs-file-list-panel zxs-frame zxs-stack zxs-stack--flush">{file_table(state)}</section></div>
    }
}
fn file_table(state: RwSignal<State>) -> impl IntoView {
    let rows = visible_rows(state);
    let disabled = Memo::new(move |_| state.with(|s| !actions::write_allowed(s)));
    view! {
        <div aria-busy=move ||state.with(|s|s.files.loading).to_string()>
            <div class="zxs-table-scroll" {web_interface::ev::on(web_interface::ev::Custom::new("scroll"), sync_row_viewport)}><table class="zxs-table zxs-file-table" aria-label=t("文件列表：每页最多 200 项","Files: up to 200 entries per page")><thead><tr><th scope="col"><span class="zxs-file-row-decoration" aria-hidden="true"></span><label class="zxs-file-check zxs-flow"><input type="checkbox" aria-label=t("选择本页全部条目（最多 200 项）","Select all entries on this page (up to 200)") disabled=move ||disabled.get() prop:checked=move ||rows.with(|rows|!rows.is_empty()&&state.with(|s|rows.iter().all(|row|s.files.checked_entries.contains(&row.check_id)))) on:change=move |event|{
                let checked=event_target_checked(&event);
                rows.with_untracked(|rows|state.update(|s|{for row in rows {if row.current_entry(s).is_some(){if checked{s.files.checked_entries.insert(row.check_id.clone());}else{s.files.checked_entries.remove(&row.check_id);}}}}));
            }/><span>{t("名称","Name")}</span></label></th><th scope="col">{t("类型","Type")}</th><th scope="col">{t("大小","Size")}</th><th scope="col">{t("操作","Actions")}</th></tr></thead>
                <tbody><ForSnapshot each=move ||rows.get() key=|row|row.key.clone() children=move |row|file_row(state,row,disabled)/></tbody>
            </table></div>
            <nav class="zxs-file-pagination zxs-flow" aria-label=t("文件列表分页","File list pages")>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.loading||actions::pending(s)||s.files.previous_cursor.is_none()) on:click=move |_|actions::change_page(state,false)>{t("上一页","Previous page")}</button>
                <span role="status" aria-live="polite">{move ||state.with(|s|if s.files.listing_failed{t("读取失败，请重试。","Loading failed. Retry the page.")}else{format!("{} {} · {} {} · {}",t("本页","This page"),s.files.entries.len(),t("匹配总数","Total matches"),s.files.total_entries,t("仅选择本页","Selection is page-local"))})}</span>
                <Show when=move ||state.with(|s|s.files.loaded&&s.files.entries.is_empty()&&s.files.query.is_empty())><span class="zxs-file-empty" role="status" aria-live="polite">{t("这个文件夹是空的。","This folder is empty.")}</span></Show>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.loading||actions::pending(s)||s.files.next_cursor.is_none()) on:click=move |_|actions::change_page(state,true)>{t("下一页","Next page")}</button>
                <Show when=move ||state.with(|s|s.files.listing_failed)><button class="zxs-button" on:click=move |_|actions::load_entries(state)>{t("重试","Retry")}</button></Show>
            </nav>
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
) -> impl IntoView {
    let checked = Memo::new(move |_| {
        row.with(|row| state.with(|s| s.files.checked_entries.contains(&row.check_id)))
    });
    view! {
        <tr class:is-selected=move ||checked.get() aria-selected=move ||checked.get().to_string() on:click=move |_|activate_row(state,row,false)>
            <td><span class="zxs-file-row-decoration" aria-hidden="true"><span class="zxs-file-row-surface" aria-hidden="true"></span></span><div class="zxs-file-select-name zxs-flow"><input class="zxs-entry-checkbox" type="checkbox" aria-label=move ||row.with(|row|format!("{} {}",t("选择","Select"),row.name)) disabled=move ||disabled.get() prop:checked=move ||checked.get() on:click=move |event|event.stop_propagation() on:change=move |event|{
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
