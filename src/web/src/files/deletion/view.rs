use crate::files::deletion::{
    Inspection, batch, batch_restore, directory, frontier, listing, load, request, restore, summary,
};
use crate::ui::State;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-stack" data-file-deletions="">
            {directory::view(state)}
            {batch::view(state)}
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
            <div class="zxs-row">
                <button
                    class="zxs-button"
                    disabled=move ||state.with(|s|!s.session_active()||s.files.deletion.loading)
                    on:click=move |_|load(state,listing::Read::First)
                >
                    {t("已删除文件", "Deleted files")}
                </button>
            </div>
            <Show when=move ||state.with(|s|s.files.deletion.open)>
                <section class="zxs-surface zxs-stack" aria-label=t("软删除与恢复", "Soft deletion and restore")>
                    <div class="zxs-row"><h2>{t("全部目录的已删除文件", "Deleted files across folders")}</h2>
                        <button
                            class="zxs-button"
                            disabled=move ||state.with(|s|!s.session_active()||s.files.deletion.loading)
                            on:click=move |_|load(state,listing::Read::First)
                        >
                            {t("重新读取第一页", "Reload the first page")}
                        </button>
                        <button class="zxs-button" on:click=move |_|state.update(|s|s.files.deletion.close())>{t("关闭", "Close")}</button></div>
                    <p>{t("这里读取服务端当前全部软删除记录，不局限于正在浏览的目录。目录仓库内文件也按稳定 file_id 和准确 D 恢复；同名但不同身份的文件不能互相替代。普通文件批次可逐个核对准确 D，或先核对原批次后原子撤销整批；活动目录删除的成员须从上方目录恢复入口整体恢复。此文件入口不恢复整目录，也不永久删除正文。", "This reads all current soft-deletion records, not only the folder being browsed. Files inside folder repositories are restored by their stable file_id and exact D; equal names never substitute for identity. Ordinary file batches can be restored one exact D at a time, or atomically after reviewing the complete original batch. Members of an active directory deletion must be restored together through the directory restore entrance above. This file entrance does not restore a whole folder or permanently delete content.")}</p>
                    <Show when=move ||state.with(|s|s.files.deletion.loading)><p role="status">{t("读取中…", "Reading…")}</p></Show>
                    {move ||state.with(|s|s.files.deletion.page.clone())
                        .map(|page| listing::view(state,page))}
                    <Show when=move ||state.with(|s|!s.files.deletion.loading&&s.files.deletion.page.is_none()&&s.files.deletion.selected.is_none())>
                        <p role="status">{t("列表尚未读取或已因操作结果失效。请重新读取；这里没有记录不代表原操作失败或文件已恢复。", "The list is unread or was invalidated by an operation result. Reload it; an empty panel does not prove that an operation failed or a file was restored.")}</p>
                    </Show>
                    {move ||state.with(|s|s.files.deletion.node.as_ref()
                        .and_then(Inspection::complete).cloned())
                        .map(|node| frontier::view(state,node))}
                    {summary::view(state)}
                    {restore::view(state)}
                    {batch_restore::view(state)}
                    <Show when=move ||state.with(|s|!s.files.deletion.loading&&s.files.deletion.selected.is_some()&&s.files.deletion.node.is_none())>
                        <p>{t("删除记录已变化或未能确认。请重新读取列表，不使用旧记录恢复。", "The deletion changed or could not be confirmed. Reload the list before restoring.")}</p>
                    </Show>
                </section>
            </Show>
        </section>
    }
}
