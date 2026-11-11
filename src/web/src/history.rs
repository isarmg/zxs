mod wire;

use crate::{
    api, format,
    ui::{State, current_session, fail, feedback},
};
use json_codec::Value;
use web_interface::components::{EmptyState, ErrorState, Feedback, Select, SelectOption};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    timestamp: Option<String>,
    slot_id: String,
    path: String,
    action: String,
    principal: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    pub events: crate::snapshot::Snapshot<Vec<Event>>,
    pub loading: bool,
    pub loaded: bool,
    pub failure: Option<Feedback>,
    pub disk: String,
}
pub(crate) fn load(state: RwSignal<State>) {
    if state.with_untracked(|s| !s.session_active() || s.activity.loading) {
        return;
    }
    state.update(|s| s.activity.loading = true);
    let epoch = state.with_untracked(|s| s.session_epoch);
    spawn_local(async move {
        let result = api::request("GET", "/api/file-manager/history", None)
            .await
            .and_then(decode);
        complete_load(state, epoch, result);
    });
}

fn complete_load(state: RwSignal<State>, epoch: u64, result: Result<Vec<Event>, api::Error>) {
    if !current_session(state, epoch) {
        return;
    }
    match result {
        Ok(events) => state.update(|s| {
            s.activity.events = events.into();
            s.activity.loaded = true;
            s.activity.failure = None;
        }),
        Err(error) if error.status == 401 => fail(state, error),
        Err(error) => state.update(|s| s.activity.failure = Some(feedback(&error))),
    }
    state.update(|s| s.activity.loading = false);
}

fn decode(value: Value) -> Result<Vec<Event>, api::Error> {
    let response: wire::Response = json_codec::from_value(value).map_err(|source| {
        let mut error = api::error(
            storage_contract::ErrorCode::InvalidResponse,
            t(
                "文件操作历史响应格式无效。",
                "Invalid activity history response.",
            ),
        );
        error.source = Some(std::sync::Arc::new(api::ErrorSource::ActivityHistory(
            source,
        )));
        error
    })?;
    if response.limit != 200 || response.events.len() > response.limit {
        return Err(api::error(
            storage_contract::ErrorCode::InvalidResponse,
            t(
                "文件操作历史响应范围无效。",
                "Invalid activity history response limit.",
            ),
        ));
    }
    Ok(response
        .events
        .into_iter()
        .map(|event| Event {
            timestamp: event.timestamp,
            slot_id: event.slot_id,
            path: event.path,
            action: event.action,
            principal: event.principal,
        })
        .collect())
}

fn action_label(action: &str) -> String {
    match action {
        "file_branch_pending" | "branch_pending" => t("分支登记", "Registered branch"),
        "upload" | "commit" | "save" | "file_save" | "content" => t("保存文件", "Saved file"),
        "directory.changes.commit" | "directory_commit" => {
            t("提交目录更改", "Committed folder changes")
        }
        "directory.git.initialize" => t("初始化目录仓库", "Initialized folder repository"),
        "directory.git.stage-owned" => t("暂存本仓库更改", "Staged this repository's changes"),
        "directory.git.stage-child" => t("暂存子仓库版本引用", "Staged child repository reference"),
        "directory.git.checkout" => t("切换目录分支", "Switched folder branch"),
        "directory.git.branch-create" => t("创建目录分支", "Created folder branch"),
        "directory.branches.create" => t("登记目录分支", "Registered folder branch"),
        "directory_branch" | "directory_branches" => t("创建目录分支", "Created folder branches"),
        "directory.branches.select" => t("切换目录分支", "Switched folder branch"),
        "directory_selection" | "directory_select" => t("切换目录分支", "Switched folder branches"),
        "directory.branch.rename" => t("重命名目录分支", "Renamed folder branch"),
        "file.delete" | "file.delete.batch" => t("软删除文件", "Soft-deleted file"),
        "file.delete.undo" | "file.delete.batch.undo" => t("恢复文件", "Restored file"),
        "file.path.rename" => t("文件路径变化", "File path changed"),
        "file.path.metadata" => t("更新节点备注", "Updated node note"),
        "file.branch.rename" => t("重命名文件分支", "Renamed file branch"),
        "file.branches.merge" => t("合并文件分支", "Merged file branches"),
        "file.tags.update" => t("更新节点标签", "Updated node tags"),
        "backup.configure" => t("更新备份策略", "Updated backup configuration"),
        "backup.configure-recursive" => t("更新逐级备份", "Updated recursive backups"),
        "directory.scope.initialize" => t("登记目录范围", "Registered folder scope"),
        "directory.namespace.move" => t("批量移动", "Moved selected branches"),
        "directory.namespace.copy" => t("批量复制", "Copied to selected branches"),
        "directory.namespace.delete" => t("批量删除", "Deleted from selected branches"),
        "directory.namespace.rename" => t("批量重命名", "Renamed in selected branches"),
        "directory.tree.write" => t("写入目录子树", "Wrote folder subtree"),
        "directory.default.set" => t("设置默认目录分支", "Set default folder branch"),
        "file.default.set" => t("设置默认文件分支", "Set default file branch"),
        "directory.default.select" => t(
            "切换 SMB 默认目录分支",
            "Switched SMB default folder branch",
        ),
        "mkdir" => t("新建文件夹", "Created folder"),
        "rename" => t("重命名", "Renamed"),
        "delete" | "delete_tree" => t("删除", "Deleted"),
        "branch" | "create_branch" | "file_branch_create" | "file_branch" => {
            t("创建文件分支", "Created file branch")
        }
        "select" | "select_branch" | "file_branch_select" | "file_selection" => {
            t("切换文件分支", "Switched file branch")
        }
        "metadata" | "file_metadata" | "version_metadata" | "file_note" | "version_note"
        | "file.metadata" | "version.metadata" => t("更新备注", "Updated note"),
        _ => t("文件操作", "File operation"),
    }
}

// Isolate immutable events and the filter from unrelated management polling.
fn visible_events(state: RwSignal<State>) -> Memo<Vec<Event>> {
    let events = Memo::new(move |_| state.with(|s| s.activity.events.clone()));
    let disk = Memo::new(move |_| state.with(|s| s.activity.disk.clone()));
    Memo::new(move |_| {
        #[cfg(test)]
        tests::record_projection();
        events.with(|events| {
            disk.with(|disk| {
                events
                    .iter()
                    .filter(|event| disk.is_empty() || event.slot_id == *disk)
                    .cloned()
                    .collect()
            })
        })
    })
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let options = Signal::derive(move || {
        let mut options = vec![SelectOption::new("", t("所有硬盘", "All disks"))];
        options.extend(
            state
                .with(|s| s.files.disks.clone())
                .into_iter()
                .map(|disk| {
                    let id = disk.slot_id;
                    SelectOption::new(id.clone(), id)
                }),
        );
        options
    });
    let selected = Signal::derive(move || state.with(|s| s.activity.disk.clone()));
    let visible_events = visible_events(state);
    view! {<section class="zxs-content-stack zxs-history-page zxs-stack" aria-busy=move ||state.with(|s|s.activity.loading).to_string()>
        {crate::files::retained_history_discovery(state)}
        <p class="zxs-row">{t("显示最近 200 条已提交的文件操作。选择文件可查看它的版本和分支。","The latest 200 committed file operations are shown. Select a file to view its versions and branches.")}</p>
        <div class="zxs-history-toolbar zxs-flow zxs-flow--end zxs-surface-end"><Select label=t("硬盘","Disk") show_label=false options=options value=selected on_change=Callback::new(move |disk:String|state.update(|s|s.activity.disk=disk))/></div>
        {move ||state.with(|s|s.activity.failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
        <Show when=move || !visible_events.with(Vec::is_empty)><div class="zxs-content-panel zxs-history-list-panel zxs-frame zxs-stack zxs-stack--flush"><ul class="zxs-activity-list zxs-history-list" aria-label=t("文件操作记录","File operation history")>{move ||{
            visible_events.get().into_iter().map(|event|{
                let time=event.timestamp.as_deref().map(format::timestamp).unwrap_or_else(||t("未记录时间","Time not recorded"));
                let path=event.path;let slot=event.slot_id;let principal=event.principal.unwrap_or_default();
                view!{<li class="zxs-history-row zxs-flow zxs-flow--stretch zxs-hover-row"><span class="zxs-history-action zxs-row">{action_label(&event.action)}</span><span class="zxs-history-path zxs-row">{if slot.is_empty(){path}else{format!("{slot}/{path}")}}</span><time class="zxs-row">{time}</time><span class="zxs-history-principal zxs-row">{if principal.is_empty(){"–".into()}else{principal}}</span></li>}
            }).collect_view()
        }}</ul></div></Show>
        <Show when=move ||state.with(|s|s.activity.loaded && s.activity.failure.is_none())&&visible_events.with(Vec::is_empty)><EmptyState message=t("尚无文件操作记录。","No file operations have been recorded.")/></Show>
    </section>}
}

#[cfg(test)]
#[path = "../../../test/web/unit/history/tests.rs"]
mod tests;
