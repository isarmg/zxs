use super::*;
use web_interface::components::{LoadingState, Select, SelectOption};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.recursive_backup.is_some()));
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .recursive_backup
                    .as_ref()
                    .and_then(Draft::response)
                    .is_none()
        })
    });
    let save_disabled = Signal::derive(move || {
        disabled.get()
            || state.with(|s| {
                s.files
                    .recursive_backup
                    .as_ref()
                    .and_then(Draft::response)
                    .is_none_or(|r| {
                        r.configuration.target_copies == 0
                            || r.configuration.target_copies as usize > r.configuration.capacity()
                    })
            })
    });
    let options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .and_then(Draft::response)
                .map(|r| {
                    (1..=r.configuration.capacity())
                        .map(|count| {
                            SelectOption::new(
                                count.to_string(),
                                format!(
                                    "{count} {}",
                                    t("份（含主盘）", "copies (including primary)")
                                ),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let mode_options = Signal::derive(move || {
        state.with(|s| {
            let mut options = vec![SelectOption::new(
                Mode::Files.value(),
                t("文件（原有模式）", "Files (existing behavior)"),
            )];
            if s.files
                .recursive_backup
                .as_ref()
                .and_then(Draft::response)
                .is_some_and(|r| {
                    r.configuration.scope.kind == storage_contract::NamespaceEntryKind::Directory
                })
            {
                options.push(SelectOption::new(
                    Mode::DirectoryTree.value(),
                    t(
                        "文件与目录（含空目录）",
                        "Files and directories (including empty folders)",
                    ),
                ));
            }
            options
        })
    });
    let mode = Signal::derive(move || {
        state.with(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .map(|draft| draft.mode.value().to_owned())
                .unwrap_or_default()
        })
    });
    let target = Signal::derive(move || {
        state.with(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .and_then(Draft::response)
                .map(|r| r.configuration.target_copies.to_string())
                .unwrap_or_default()
        })
    });
    let prefixes = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .and_then(Draft::response)
                .map(|r| {
                    r.prefixes(r.configuration.target_copies)
                        .into_iter()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let published_paths = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .and_then(Draft::response)
                .map(|r| {
                    r.configuration
                        .backup_paths
                        .clone()
                        .into_iter()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    view! {<Show when=move ||open.get()>
        <section class="zxs-frame zxs-stack zxs-backup-review" aria-label=t("逐级备份设置","Recursive backup settings")>
            <div class="zxs-file-toolbar zxs-flow">
                <h3 class="zxs-row">{t("逐级备份设置","Recursive backup settings")}</h3>
                <button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|close(state)>{t("关闭","Close")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||actions::pending(s)) on:click=move |_|reload(state)>{t("重新读取来源","Reload sources")}</button>
            </div>
            <p class="zxs-row">{move ||state.with(|s|s.files.recursive_backup.as_ref().map(|draft|if draft.path.is_empty(){"/".into()}else{draft.path.clone()}).unwrap_or_default())}</p>
            <p class="zxs-row">{t("保存会替换本范围的自定义备份前缀。每级保留直接来源磁盘的完整逻辑路径，并在前面加该来源盘名。例如：二级 磁盘A/目录/文件，三级 磁盘B/磁盘A/目录/文件。主盘路径不变。","Saving replaces custom backup prefixes for this scope. Each level prefixes its immediate source disk’s name to that source’s full logical path. For example: level 2 DiskA/folder/file; level 3 DiskB/DiskA/folder/file. Primary paths stay unchanged.")}</p>
            <p class="zxs-row">{t("保存重保护当前文件与仍可用的历史版本。原有文件模式仅在整个范围无文件时发布空目录；目录可显式选择“文件与目录”，一次捕获当前文件和空子目录。独立子备份规则保持不变。已捕获路径跟随主逻辑改名或移动；盘名前缀变化或新增空目录仍须重新读取并保存。","Saving re-protects current files and still-available history. Existing file mode publishes empty directories only when the whole scope has no files. For a directory, explicitly choose Files and directories to capture files and empty descendants together. Independent child backup rules remain unchanged. Captured paths follow primary renames and moves; reload and save to update disk-name prefixes or capture newly created empty directories.")}</p>
            <Show when=move ||state.with(|s|s.files.recursive_backup.as_ref().is_some_and(|d|matches!(d.load,Load::Pending)))><LoadingState/></Show>
            <Show when=move ||state.with(|s|s.files.recursive_backup.as_ref().and_then(Draft::response).is_some())>
                <Show when=move ||state.with(|s|s.files.recursive_backup.as_ref().and_then(Draft::response).is_some_and(|r|r.configuration.inherited))><p class="zxs-row">{t("当前继承上级规则；保存会为此范围单独设置逐级规则。","This scope inherits its parent’s rule. Saving sets recursive protection for this scope.")}</p></Show>
                <p class="zxs-row">{t("前缀预览不包含主盘完整路径；有名称的仓库保留仓库名，磁盘根仓库不添加仓库名。来源变化时服务器会拒绝旧依据，请重新读取后复核。","The preview below shows prefixes, not the full primary path. Named repositories retain their name; disk-root repositories do not add one. If sources change, the server rejects the old baseline. Reload and review before saving.")}</p>
                <Select label=t("本次保存模式","Mode for this save") options=mode_options value=mode disabled=disabled on_change=Callback::new(move |value:String|select_mode(state,value))/>
                <p class="zxs-row">{t("此选项控制新的保存请求，不表示服务器已配置的模式。重新读取会恢复原有文件模式。","This choice controls the new request; it does not identify the server’s configured mode. Reloading resets it to existing file mode.")}</p>
                <Show when=move ||state.with(|s|s.files.recursive_backup.as_ref().is_some_and(|d|d.mode==Mode::DirectoryTree))><p class="zxs-row" role="status">{t("文件与目录模式要求所有目录备份目标均为元数据成员。不满足时整笔请求拒绝，不会静默退回只备份文件。","Files and directories mode requires every directory backup target to be a metadata member. If unsupported, the entire request is rejected, never silently reduced to files only.")}</p></Show>
                <Select label=t("目标份数","Target copy count") options=options value=target disabled=disabled on_change=Callback::new(move |value:String|select_copies(state,value))/>
                <Show when=move ||state.with(|s|s.files.recursive_backup.as_ref().and_then(Draft::response).is_some_and(|r|r.configuration.file_count==0))><p class="zxs-row" role="status">{t("此范围没有文件。保存将发布当前目录及其空子目录；目标须为元数据成员，服务器会复核来源和目标。","This scope has no files. Saving publishes this directory and its empty descendants. Targets must be metadata members; the server rechecks sources and targets.")}</p></Show>
                <div class="zxs-stack" aria-label=t("逐级前缀预览","Recursive prefix preview")>
                    {move ||prefixes.get().into_iter().map(|(slot,prefix)|view!{<p class="zxs-row">{format!("{slot}: {prefix}/{}",t("〈主盘完整逻辑路径〉","〈full primary logical path〉"))}</p>}).collect_view()}
                </div>
                <Show when=move ||!published_paths.get().is_empty()>
                    <p class="zxs-row">{t("当前已发布路径（不是新规则预览）","Currently published paths (not the new preview)")}</p>
                    {move ||published_paths.get().into_iter().map(|(slot,path)|view!{<p class="zxs-row">{format!("{slot}: {path}")}</p>}).collect_view()}
                </Show>
                <Show when=move ||state.with(|s|!actions::write_allowed(s))><p class="zxs-row">{t("此时仅可查看；会话、存储或操作状态尚不允许保存。","Read only for now. The session, storage or operation state does not permit saving.")}</p></Show>
                <button class="zxs-button" disabled=move ||save_disabled.get() on:click=move |_|submit(state)>{t("保存逐级备份","Save recursive backups")}</button>
            </Show>
            <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Backup))}</p>
        </section>
    </Show>}
}
