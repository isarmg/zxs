use super::*;
use web_interface::i18n::t;

pub(in crate::files::deletion) fn retained_view(request: &Request) -> impl IntoView + use<> {
    let scope = format!(
        "{} · {} · {} · {}",
        request.slot, request.repository, request.branch, request.tree
    );
    let entries = request
        .entries
        .iter()
        .map(|entry| format!("{} · {} · {}", entry.path.as_str(), entry.file, entry.entry))
        .collect::<Vec<_>>();
    view! {
        <details class="zxs-stack">
            <summary>{t("核对这批原路径、文件身份和目录依据", "Inspect this batch's original paths, file identities and folder baseline")}</summary>
            <p><code>{scope}</code></p>
            <ul>{entries.into_iter().map(|label|view!{<li><code>{label}</code></li>}).collect_view()}</ul>
        </details>
    }
}
pub(in crate::files::deletion) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s|s.files.deletion.batch_review.clone()).map(|review| {
            let epoch = review.epoch;
            let generation = review.generation;
            let count = review.request.entries.len();
            let files = review.request.targets().len();
            view! {
                <section class="zxs-surface zxs-stack" data-zxs-file-delete-batch-review="">
                    <h2>{t("确认同一批软删除", "Confirm one soft-deletion batch")}</h2>
                    <p>{format!("{} · {count} {} · {files} {}", review.branch_name, t("已选路径", "selected paths"), t("稳定文件", "stable files"))}</p>
                    <p>{t(
                        "这批在一个原子操作中软删除所有列出的稳定文件，隐藏它们在全部路径和分支的出现并冻结编辑，不是只删当前分支的名字。同一文件的多个别名只产生一个 D；正文、目录工作树、HEAD/index 和历史保留，不自动提交目录。",
                        "One atomic operation soft-deletes all listed stable files, hiding their occurrences across every path and branch and freezing editing, not just names in this branch. Multiple aliases of one file create one D. Content, folder working trees, HEAD/index and history remain retained; no folder commit is created.",
                    )}</p>
                    {retained_view(&review.request)}
                    <p>{t(
                        "任一文件、依据或权限不符时整批拒绝，不静默跳过、不拆成逐文件请求。刷新、切换目录或改变本页选择后请重新核对；已发送操作不会因关闭而取消。",
                        "A mismatched file, baseline or permission rejects the batch, without skipping files or splitting requests. Review again after refreshing, navigation or changing the selection. Closing does not cancel a sent operation.",
                    )}</p>
                    <div class="zxs-row">
                        <button class="zxs-button" data-zxs-confirm-file-delete-batch=""
                            disabled=move ||state.with(|s|reviewed(s,epoch,generation).is_none())
                            on:click=move |_|submit(state,epoch,generation)>{t("确认整批软删除", "Confirm atomic soft deletion")}</button>
                        <button class="zxs-button" on:click=move |_|state.update(|s| {
                            if s.session_epoch==epoch && s.files.deletion.batch_review.as_ref().is_some_and(|r|r.generation==generation) {
                                s.files.deletion.cancel_review();
                            }
                        })>{t("关闭，不提交", "Close without submitting")}</button>
                    </div>
                    <p>{t(
                        "结果未知时保留同一原请求并只查询其回执。成功后的恢复仍逐个选择准确 D，不冒充整批原子撤销，也不恢复目录容器。",
                        "An unknown result retains this same request and queries its receipt. Restoration after success still selects each exact D individually; this does not atomically undo a batch or restore a folder container.",
                    )}</p>
                </section>
            }
        })}
    }
}
