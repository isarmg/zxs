use super::*;
use web_interface::i18n::t;
const FRONTIER_PAGE: usize = 100;

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-stack" data-file-deletions="">
            {move ||state.with(|s|s.files.deletion.review.clone()).map(|review| {
                let generation = review.generation;
                let epoch = review.epoch;
                view! {
                    <section class="zxs-surface zxs-stack" data-zxs-file-delete-review="" aria-label=t("确认软删除", "Confirm soft deletion")>
                        <h2>{t("软删除文件", "Soft-delete file")}</h2>
                        <p><code>{review.display_path}</code></p>
                        <p><code>{review.target.repository.to_string()}</code>" · "<code>{review.target.file.to_string()}</code></p>
                        <p>{t("此操作隐藏同一稳定文件的全部路径和分支，并冻结其编辑，不是只删当前目录中的一个名字。正文、目录工作树、HEAD/index 和历史保留，不会永久删除节点或自动提交目录。已初始化目录内有独立文件分支的当前可写文件也走这个入口；全部当前所有者与范围由服务端核验，不支持的选择明确拒绝。", "This hides all paths and branches of the same stable file and freezes editing, not just one name in this folder. Content, working trees, HEAD/index and history remain retained; no nodes are permanently removed and no folder is automatically committed. Current writable files with independent file branches inside initialized folder repositories use this same entrance. The server checks all current owners and scope, and explicitly rejects unsupported selections.")}</p>
                        <div class="zxs-row">
                            <button class="zxs-button" data-zxs-confirm-file-delete="" disabled=move ||!state.with(|s|request::review_allowed(s,generation)) on:click=move |_|request::submit_delete(state,generation)>{t("确认软删除整个文件", "Confirm whole-file soft deletion")}</button>
                            <button class="zxs-button" on:click=move |_|state.update(|s|{
                                if s.session_epoch==epoch && s.files.deletion.review.as_ref().is_some_and(|review|review.generation==generation) {
                                    s.files.deletion.cancel_review();
                                }
                            })>{t("关闭", "Close")}</button>
                        </div>
                        <p>{t("关闭或切换目录不会取消已发送的操作。结果不明时只查询原操作，不重复提交。", "Closing or navigating does not cancel a submitted operation. An unknown outcome is checked using the original operation, without resubmitting.")}</p>
                    </section>
                }
            })}
            <div class="zxs-row"><button class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||s.files.deletion.loading)
                on:click=move |_|load(state,false)>{t("已删除文件", "Deleted files")}</button></div>
            <Show when=move ||state.with(|s|s.files.deletion.open)>
                <section class="zxs-surface zxs-stack" aria-label=t("软删除与恢复", "Soft deletion and restore")>
                    <div class="zxs-row"><h2>{t("全部目录的已删除文件", "Deleted files across folders")}</h2>
                        <button class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||s.files.deletion.loading) on:click=move |_|load(state,false)>{t("重新读取第一页", "Reload the first page")}</button>
                        <button class="zxs-button" on:click=move |_|state.update(|s|s.files.deletion.close())>{t("关闭", "Close")}</button></div>
                    <p>{t("这里读取服务端当前全部软删除记录，不局限于正在浏览的目录。目录仓库内文件也按稳定 file_id 和准确 D 恢复；同名但不同身份的文件不能互相替代。每个文件可单独撤销，即使原来由同一批次删除；此入口不恢复整目录、不原子撤销整批，也不永久删除正文。", "This reads all current soft-deletion records, not only the folder being browsed. Files inside folder repositories are restored by their stable file_id and exact D; equal names never substitute for identity. Each file can be restored individually even after a batch deletion. This entrance does not restore a whole folder, atomically undo a batch or permanently delete content.")}</p>
                    <Show when=move ||state.with(|s|s.files.deletion.loading)><p role="status">{t("读取中…", "Reading…")}</p></Show>
                    {move ||state.with(|s|s.files.deletion.page.clone()).map(|page| {
                        let empty = page.entries.is_empty();
                        let next = page.next_after.is_some();
                        let entries = page.entries.clone();
                        view!{
                            <Show when=move ||empty><p>{t("这一页没有已删除文件。", "No deleted files on this page.")}</p></Show>
                            <ul>{entries.into_iter().map(|summary| {
                                let path = summary.path.as_str().to_owned();
                                let id = summary.file_id.to_string();
                                let identity = format!("{} · {}",summary.repository_id,summary.file_id);
                                view!{<li><button class="zxs-button" data-deleted-file=id disabled=move ||state.with(|s|s.files.deletion.loading)
                                    on:click=move |_|select(state,summary.clone())>{path}</button><code>{identity}</code></li>}
                            }).collect_view()}</ul>
                            <Show when=move ||next><button class="zxs-button" disabled=move ||state.with(|s|s.files.deletion.loading)
                                on:click=move |_|load(state,true)>{t("下一页", "Next page")}</button></Show>
                        }
                    })}
                    <Show when=move ||state.with(|s|!s.files.deletion.loading&&s.files.deletion.page.is_none()&&s.files.deletion.selected.is_none())>
                        <p role="status">{t("列表尚未读取或已因操作结果失效。请重新读取；这里没有记录不代表原操作失败或文件已恢复。", "The list is unread or was invalidated by an operation result. Reload it; an empty panel does not prove that an operation failed or a file was restored.")}</p>
                    </Show>
                    {move ||state.with(|s|s.files.deletion.node.clone()).map(|node| frontier(state,node))}
                    {restore::view(state)}
                    <Show when=move ||state.with(|s|!s.files.deletion.loading&&s.files.deletion.selected.is_some()&&s.files.deletion.node.is_none())>
                        <p>{t("删除记录已变化或未能确认。请重新读取列表，不使用旧记录恢复。", "The deletion changed or could not be confirmed. Reload the list before restoring.")}</p>
                    </Show>
                </section>
            </Show>
        </section>
    }
}

fn frontier(
    state: RwSignal<State>,
    node: Arc<storage_contract::FileDeletionNode>,
) -> impl IntoView {
    let display = node.clone();
    let pages = node.branch_heads.len().div_ceil(FRONTIER_PAGE);
    let restore_button = restore::button(state, node.clone());
    view! {
        <section class="zxs-stack" data-deletion-node=node.node_id.clone()>
            <h3>{format!("{} · {}",t("删除节点", "Deletion node"),node.path.as_str())}</h3>
            <p>{t("无正文节点。箭头表示该分支保留的最新节点连接到删除节点；没有更改分支 HEAD。", "Bodyless node. Each arrow joins the retained branch tip to the deletion node; branch HEADs are unchanged.")}</p>
            <details class="zxs-stack">
                <summary>{t("核对文件身份与删除依据", "Inspect file identity and deletion evidence")}</summary>
                <dl>
                    <dt>{t("文件 ID", "File ID")}</dt><dd><code>{node.file_id.clone()}</code></dd>
                    <dt>{t("仓库 ID", "Repository ID")}</dt><dd><code>{node.repository_id.clone()}</code></dd>
                    <dt>{t("删除节点", "Deletion node")}</dt><dd><code>{node.node_id.clone()}</code></dd>
                    <dt>{t("原删除操作", "Original deletion operation")}</dt><dd><code>{node.operation_id.to_string()}</code></dd>
                    <dt>{t("删除主体", "Deleted by")}</dt><dd>{node.author.clone()}</dd>
                    <dt>{t("发布序号", "Publication sequence")}</dt><dd>{node.sequence.to_string()}</dd>
                    <dt>{t("保留分支 / 去重父边", "Retained branches / distinct parents")}</dt><dd>{format!("{} / {}",node.branch_heads.len(),node.parent_node_ids.len())}</dd>
                </dl>
            </details>
            <div class="zxs-table-scroll"><table class="zxs-table" aria-label=t("所有分支末端到删除节点的连线", "Edges from every branch tip to the deletion node")>
                <thead><tr><th>{t("分支", "Branch")}</th><th>{t("末端节点", "Tip node")}</th><th>{t("连接", "Edge")}</th><th>{t("删除", "Deleted")}</th></tr></thead>
                <tbody>{move || {
                    let page = state.with(|s|s.files.deletion.frontier_page);
                    display.branch_heads.iter().skip(page*FRONTIER_PAGE).take(FRONTIER_PAGE).map(|(branch,head)|view!{
                        <tr data-deletion-parent=head.clone()><td><code>{branch.clone()}</code></td><td><code>{head.clone()}</code></td><td>"→"</td><td><code>{display.node_id.clone()}</code></td></tr>
                    }).collect_view()
                }}</tbody>
            </table></div>
            <Show when=move || {pages > 1}><div class="zxs-row">
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.deletion.frontier_page==0) on:click=move |_|state.update(|s|{if s.files.deletion.frontier_page>0{s.files.deletion.frontier_page-=1;}})>{t("上一页分支", "Previous branches")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.deletion.frontier_page+1>=pages) on:click=move |_|state.update(|s|{if s.files.deletion.frontier_page+1<pages{s.files.deletion.frontier_page+=1;}})>{t("下一页分支", "Next branches")}</button>
            </div></Show>
            {restore_button}
        </section>
    }
}
