use super::*;
use web_interface::i18n::t;

pub(in crate::files::deletion) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-stack" data-directory-deletions="">
            <Show when=move ||state.with(|s|s.files.deletion.directory.review_loading)>
                <p class="zxs-row" role="status">{t(
                    "正在核对普通目录及其当前仓库所有者；尚未发送删除请求。",
                    "Checking the ordinary folder and its current repository owner. No deletion has been sent.",
                )}</p>
                <button class="zxs-button" on:click=move |_|state.update(|s|s.files.deletion.cancel_review())>
                    {t("取消核对", "Cancel review")}
                </button>
            </Show>
            {move ||state.with(|s|s.files.deletion.directory.review.clone()).map(|review| {
                let generation = review.generation;
                let epoch = review.selection.location.epoch;
                let path = format!("{}/{}", review.selection.location.slot, review.selection.display_path);
                let request = review.request;
                view! {
                    <section class="zxs-surface zxs-stack" data-zxs-directory-delete-review="">
                        <h2>{t("软删除整个普通目录", "Soft-delete this whole ordinary folder")}</h2>
                        <p><code>{path}</code></p>
                        <p><code>{request.repository().to_string()}</code>" · "<code>{request.directory().to_owned()}</code></p>
                        <details>
                            <summary>{t("核对原目录来源", "Inspect the original folder source")}</summary>
                            <dl>
                                <dt>{t("所有者目录", "Owner directory")}</dt><dd><code>{request.owner().to_owned()}</code></dd>
                                <dt>{t("工作分支", "Working branch")}</dt><dd><code>{request.branch().to_owned()}</code></dd>
                                <dt>{t("分支 CAS 树", "Branch CAS tree")}</dt><dd><code>{request.tree().to_owned()}</code></dd>
                            </dl>
                        </details>
                        <p>{t(
                            "一次原生操作隐藏这个目录及其普通子树，空目录也保留独立删除记录。同一稳定文件的全部路径与分支同时隐藏。正文、完整历史、目录工作树和 HEAD/index 保留，不自动提交目录，不永久删除数据。先前已删除的文件保持原来的删除状态。",
                            "One native operation hides this folder and its ordinary subtree. Empty folders also retain a distinct deletion record. All paths and branches of each stable file are hidden together. Content, complete history, working trees and HEAD/index remain retained; no folder is committed and no data is permanently deleted. Previously deleted files keep their original deletion state.",
                        )}</p>
                        <p>{t(
                            "当前入口仅支持已初始化所有者目录下的一个普通子目录，不支持目录仓库本身、嵌套初始化仓库、只读快照、跨盘或多个目录。服务端在发布时重新核对完整子树、当前身份和权限；最多 100 个不同文件、4,096 个分支/父边，超限整笔拒绝，不拆成逐文件删除。",
                            "This entrance supports one ordinary child of the currently initialized owner, not an initialized repository itself, nested initialized repositories, read-only snapshots, cross-disk selections or multiple folders. The server rechecks the complete subtree, current identity and permission when publishing. Limits are 100 distinct files and 4,096 branches/parent edges; an oversized request is rejected as a whole, never split into file deletions.",
                        )}</p>
                        <div class="zxs-row">
                            <button class="zxs-button" data-zxs-confirm-directory-delete=""
                                disabled=move ||state.with(|s|reviewed(s,generation).is_none())
                                on:click=move |_|confirm_delete(state,generation)>
                                {t("确认软删除整个目录", "Confirm whole-folder soft deletion")}
                            </button>
                            <button class="zxs-button" on:click=move |_|state.update(|s| {
                                if s.session_epoch==epoch && s.files.deletion.review_generation==generation {
                                    s.files.deletion.cancel_review();
                                }
                            })>{t("取消，不发送删除请求", "Cancel without sending deletion")}</button>
                        </div>
                        <p>{t(
                            "发送前保存完整原请求。关闭或切换页面不取消已发送的操作；结果未知时只查询或明确续发同一个原请求，不生成新编号。",
                            "The complete original request is saved before sending. Closing or navigating does not cancel a submitted operation. An unknown outcome is checked or explicitly retried with that same request and operation ID.",
                        )}</p>
                    </section>
                }
            })}
            <div class="zxs-row">
                <button class="zxs-button" data-zxs-open-directory-deletions=""
                    disabled=move ||!state.with(read_allowed)
                    on:click=move |_|load(state,false)>
                    {t("当前所有者的已删除目录", "Deleted folders in this owner")}
                </button>
            </div>
            <Show when=move ||state.with(|s|s.files.deletion.directory.open)>
                <section class="zxs-surface zxs-stack" data-zxs-directory-deletions-panel="">
                    <div class="zxs-row">
                        <h2>{t("普通目录整体恢复", "Restore an ordinary folder as a whole")}</h2>
                        <button class="zxs-button" disabled=move ||!state.with(read_allowed)
                            on:click=move |_|load(state,false)>{t("重新核对当前所有者", "Reload the current owner")}</button>
                        <button class="zxs-button" on:click=move |_|state.update(|s| {
                            s.files.deletion.cancel_review();
                            s.files.deletion.directory.close();
                        })>{t("关闭", "Close")}</button>
                    </div>
                    <p>{t(
                        "这里列出当前目录所属初始化仓库的目录删除记录，每页最多 100 项。恢复一笔准确目录 D，连同它真正创建的文件 D 一起撤销；原先已删除的文件不复活。其它活动目录仍引用的文件须先解除后一个目录删除，不支持把成员单独移出。",
                        "This lists directory deletions in the initialized repository that owns the current folder, up to 100 per page. Restore one exact directory D together with the file deletions it actually created; previously deleted files remain deleted. If another active directory deletion still references a file, restore that later directory first. Members cannot escape the group individually.",
                    )}</p>
                    <Show when=move ||state.with(|s|s.files.deletion.directory.loading)>
                        <p class="zxs-row" role="status">{t("读取中…", "Reading…")}</p>
                    </Show>
                    {move ||state.with(|s|s.files.deletion.directory.scope.clone()).map(|scope| {
                        let path = if scope.owner_path.is_empty() {
                            t("仓库根", "Repository root")
                        } else {
                            scope.owner_path.clone()
                        };
                        view!{<p><code>{scope.location.repository}</code>" · "<code>{scope.owner}</code>" · "{path}</p>}
                    })}
                    <Show when=move ||state.with(|s| {
                        let model=&s.files.deletion.directory;
                        model.scope.is_some() && scope_current(s).is_none()
                    })>
                        <p class="zxs-row" role="status">{t(
                            "位置或列表已改变。此记录仅供查看，请重新核对当前所有者后再选择恢复。",
                            "The location or listing changed. These records are display-only; reload the current owner before selecting a restore.",
                        )}</p>
                    </Show>
                    {move ||state.with(|s|s.files.deletion.directory.page.clone()).map(|page| {
                        let empty = page.entries.is_empty();
                        let next = page.next_after.is_some();
                        let entries = page.entries.clone();
                        view! {
                            <Show when=move ||empty><p>{t("本页没有已删除目录。", "No deleted folders on this page.")}</p></Show>
                            <ul>{entries.into_iter().map(|summary| {
                                let path = summary.path.as_str().to_owned();
                                let identity = summary.directory_id.clone();
                                let deletion = summary.deletion_node_id.clone();
                                view!{<li>
                                    <button class="zxs-button" data-deleted-directory=identity.clone()
                                        disabled=move ||state.with(|s|s.files.deletion.directory.loading||scope_current(s).is_none())
                                        on:click=move |_|select(state,summary.clone())>{path}</button>
                                    <code>{identity}</code>" · "<code>{deletion}</code>
                                </li>}
                            }).collect_view()}</ul>
                            <Show when=move ||next><button class="zxs-button"
                                disabled=move ||state.with(|s|s.files.deletion.directory.loading||scope_current(s).is_none())
                                on:click=move |_|load(state,true)>{t("下一页", "Next page")}</button></Show>
                        }
                    })}
                    {move ||state.with(|s|s.files.deletion.directory.node.clone()).map(|node| node_view(state,node))}
                    <Show when=move ||state.with(|s| {
                        let model=&s.files.deletion.directory;
                        !model.loading && model.page.is_none() && model.selected.is_none()
                    })>
                        <p role="status">{t(
                            "列表尚未读取或已因操作结果失效。没有显示记录不证明原请求失败或目录已经恢复。",
                            "The list has not been read or was invalidated by an operation. No displayed record does not prove the original request failed or the folder was restored.",
                        )}</p>
                    </Show>
                    <Show when=move ||state.with(|s| {
                        let model=&s.files.deletion.directory;
                        !model.loading && model.selected.is_some() && model.node.is_none()
                    })>
                        <p role="status">{t(
                            "准确删除节点已变化或无法确认。请重新读取，不使用旧节点恢复。",
                            "The exact deletion node changed or could not be confirmed. Reload instead of restoring an old node.",
                        )}</p>
                    </Show>
                    {move ||state.with(|s|s.files.deletion.directory.restore_review.clone()).map(|review| {
                        let generation=review.generation;
                        let node=review.node;
                        view! {
                            <section class="zxs-surface zxs-stack" data-zxs-directory-restore-review="">
                                <h3>{t("确认撤销准确目录删除", "Confirm undoing this exact directory deletion")}</h3>
                                <p><code>{node.path.as_str().to_owned()}</code>" · "<code>{node.node_id.clone()}</code></p>
                                <p>{t(
                                    "刚才已重新读取并完整比较同一目录 D，包括原操作和全部成员。确认只撤销这笔目录删除及它创建的文件 D，不覆盖新路径、不提交目录、不复活之前已删除的文件。服务端仍会重新核对全部保留依据和权限。",
                                    "The same complete directory D was just read and compared again, including its original operation and every member. Confirmation only undoes this directory deletion and the file D records it created. It does not overwrite new paths, commit the directory or revive previously deleted files. The server still rechecks all retained evidence and permission.",
                                )}</p>
                                <div class="zxs-row">
                                    <button class="zxs-button" data-zxs-confirm-directory-restore=""
                                        disabled=move ||state.with(|s|restore_reviewed(s,generation).is_none())
                                        on:click=move |_|confirm_restore(state,generation)>{t("确认整体恢复", "Confirm whole-folder restore")}</button>
                                    <button class="zxs-button" on:click=move |_|state.update(|s| {
                                        if s.files.deletion.review_generation==generation {
                                            s.files.deletion.cancel_review();
                                        }
                                    })>{t("取消，不发送恢复请求", "Cancel without sending restore")}</button>
                                </div>
                            </section>
                        }
                    })}
                </section>
            </Show>
        </section>
    }
}
fn node_view(state: RwSignal<State>, node: Arc<DirectoryDeletionNode>) -> impl IntoView {
    let expected = StoredValue::new(Arc::clone(&node));
    let created = node
        .files
        .values()
        .filter(|member| member.created_by_directory)
        .count();
    let members = node.files.iter().map(|(file, member)| {
        view!{<li><code>{file.clone()}</code>" → "<code>{member.deletion_node_id.clone()}</code>" · "{
            if member.created_by_directory {
                t("由此目录操作创建，随目录恢复", "Created by this operation; restored with the folder")
            } else {
                t("先前已删除，恢复目录后仍保留", "Previously deleted; remains deleted after restore")
            }
        }</li>}
    }).collect_view();
    view! {
        <section class="zxs-stack" data-directory-deletion-node=node.node_id.clone()>
            <h3>{node.path.as_str().to_owned()}</h3>
            <dl>
                <dt>{t("目录 / 删除节点", "Directory / deletion node")}</dt>
                <dd><code>{node.directory_id.clone()}</code>" · "<code>{node.node_id.clone()}</code></dd>
                <dt>{t("原操作 / 主体 / 发布序号", "Original operation / author / publication sequence")}</dt>
                <dd><code>{node.operation_id.to_string()}</code>" · "{node.author.clone()}" · "{node.sequence.to_string()}</dd>
                <dt>{t("新建删除 / 先前已删除文件", "New deletions / previously deleted files")}</dt>
                <dd>{format!("{} / {}", created, node.files.len()-created)}</dd>
            </dl>
            <details><summary>{t("完整成员身份（最多 100 个文件）", "Complete member identities (up to 100 files)")}</summary><ul>{members}</ul></details>
            <button class="zxs-button" data-zxs-review-directory-restore=""
                disabled=move ||!state.with(|s|expected.with_value(|node|restore_button_current(s,node)))
                on:click=move |_|review_restore(state,expected.get_value())>{t("重新核对并恢复这个目录…", "Review restoring this folder…")}</button>
        </section>
    }
}
