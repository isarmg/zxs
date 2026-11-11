use crate::files::deletion::{
    batch_restore::{Request, begin, confirm, reviewed},
    restore,
};
use crate::ui::State;
use std::sync::Arc;
use storage_contract::FileDeletionNode;
use web_interface::{i18n::t, prelude::*};

pub(in crate::files::deletion) fn retained_view(request: &Request) -> impl IntoView + use<> {
    let original = request.original().to_string();
    let nodes = request
        .nodes()
        .iter()
        .map(|node| {
            format!(
                "{} · {} · {}",
                node.path.as_str(),
                node.file_id,
                node.node_id
            )
        })
        .collect::<Vec<_>>();
    view! {
        <details class="zxs-stack" {web_interface::attr::custom::custom_attribute("open", "")}>
            <summary>{t("完整原批次的文件与删除节点", "Files and deletion nodes of the complete original batch")}</summary>
            <p><code>{original}</code></p>
            <ul>{nodes.into_iter().map(|label| view! {<li><code>{label}</code></li>}).collect_view()}</ul>
        </details>
    }
}

pub(in crate::files::deletion) fn button(
    state: RwSignal<State>,
    node: Arc<FileDeletionNode>,
) -> impl IntoView {
    let target = StoredValue::new(restore::Target {
        epoch: state.with_untracked(|s| s.session_epoch),
        generation: state.with_untracked(|s| s.files.deletion.generation),
        node,
    });
    view! {
        <button class="zxs-button" data-zxs-review-batch-restore=""
            disabled=move ||!state.with(|s| target.with_value(|target| target.current(s)))
            on:click=move |_|begin(state, target.get_value())>
            {t("核对并恢复此文件的原删除批次…", "Review restoring this file's original deletion batch…")}
        </button>
    }
}

pub(in crate::files::deletion) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s| s.files.deletion.restore_review.as_ref().and_then(restore::Review::batch).cloned()).map(|review| {
            let epoch = review.target.epoch;
            let generation = review.generation;
            view! {
                <section class="zxs-surface zxs-stack" data-zxs-batch-restore-review="">
                    <h3>{t("确认原子恢复完整删除批次", "Confirm atomic restoration of the complete deletion batch")}</h3>
                    {retained_view(&review.request)}
                    <p>{t("一次操作撤销原批次列出的全部准确删除节点，恢复这些文件的 SMB 可见性并解除编辑冻结。任一节点已变更、已恢复、受目录删除约束或权限不符时整批拒绝，不跳过文件、不拆成逐文件请求。正文、目录工作树及历史保留。", "One operation removes every exact deletion node in the original batch, restoring SMB visibility and unfreezing these files. Any changed or already restored node, directory deletion constraint or permission failure rejects the whole batch. No files are skipped and no per-file requests are sent. Content, folder working trees and history remain retained.")}</p>
                    <div class="zxs-row">
                        <button class="zxs-button" data-zxs-confirm-batch-restore=""
                            disabled=move ||state.with(|s| reviewed(s, epoch, generation).is_none())
                            on:click=move |_|confirm(state, epoch, generation)>
                            {t("确认恢复完整原批次", "Confirm restoring the complete original batch")}
                        </button>
                        <button class="zxs-button" on:click=move |_|state.update(|s| {
                            if s.session_epoch == epoch && s.files.deletion.restore_review.as_ref().and_then(restore::Review::batch).is_some_and(|r| r.generation == generation) {
                                s.files.deletion.cancel_review();
                            }
                        })>{t("取消，不发送请求", "Cancel without sending")}</button>
                    </div>
                    <p>{t("发送前保留原请求、原批次及完整删除节点。未知结果仅查询同一操作回执；重新发送必须再次明确确认原请求。关闭不会取消已发送操作。", "The original request, batch and complete deletion nodes are retained before sending. Unknown outcomes are checked using the same operation receipt; retry requires explicit review of the original request. Closing does not cancel a sent operation.")}</p>
                </section>
            }
        })}
    }
}
