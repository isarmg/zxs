use super::*;
use web_interface::i18n::t;
const FRONTIER_PAGE: usize = 100;

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-stack" data-file-deletions="">
            <div class="zxs-row"><button class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||s.files.deletion.loading)
                on:click=move |_|load(state,false)>{t("已删除文件", "Deleted files")}</button></div>
            <Show when=move ||state.with(|s|s.files.deletion.open)>
                <section class="zxs-surface zxs-stack" aria-label=t("软删除与恢复", "Soft deletion and restore")>
                    <div class="zxs-row"><h2>{t("已删除文件", "Deleted files")}</h2>
                        <button class="zxs-button" on:click=move |_|state.update(|s|s.files.deletion.close())>{t("关闭", "Close")}</button></div>
                    <p>{t("正文和历史仍保留。删除节点连接各分支末端；删除此节点撤销删除，并恢复 SMB 可见性。此入口目前只处理未初始化目录仓库的磁盘工作区文件。", "Content and history remain retained. The deletion node joins every branch tip. Removing it restores SMB visibility. This currently covers disk-workspace files outside initialized folder repositories.")}</p>
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
                                view!{<li><button class="zxs-button" data-deleted-file=id disabled=move ||state.with(|s|s.files.deletion.loading)
                                    on:click=move |_|select(state,summary.clone())>{path}</button></li>}
                            }).collect_view()}</ul>
                            <Show when=move ||next><button class="zxs-button" disabled=move ||state.with(|s|s.files.deletion.loading)
                                on:click=move |_|load(state,true)>{t("下一页", "Next page")}</button></Show>
                        }
                    })}
                    {move ||state.with(|s|s.files.deletion.node.clone()).map(|node| frontier(state,node))}
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
    view! {
        <section class="zxs-stack" data-deletion-node=node.node_id.clone()>
            <h3>{format!("{} · {}",t("删除节点", "Deletion node"),node.path.as_str())}</h3>
            <p>{t("无正文节点。箭头表示该分支保留的最新节点连接到删除节点；没有更改分支 HEAD。", "Bodyless node. Each arrow joins the retained branch tip to the deletion node; branch HEADs are unchanged.")}</p>
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
            <button class="zxs-button" data-undo-file-deletion="" disabled=move ||!state.with(undo_allowed) on:click=move |_|undo(state)>{t("删除此节点，撤销文件删除", "Remove this node and restore the file")}</button>
        </section>
    }
}
