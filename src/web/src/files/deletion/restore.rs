//! An explicit restore review reads the same complete D again before allowing
//! a new undo request. Only the existing original-request path sends a write.
use super::*;
use web_interface::i18n::t;

#[derive(Clone)]
struct Target {
    epoch: u64,
    generation: u64,
    node: Arc<storage_contract::FileDeletionNode>,
}
impl Target {
    fn current(&self, state: &State) -> bool {
        undo_allowed(state)
            && state.session_epoch == self.epoch
            && state.files.deletion.generation == self.generation
            && state
                .files
                .deletion
                .node
                .as_ref()
                .is_some_and(|node| Arc::ptr_eq(node, &self.node))
    }
}
#[derive(Clone)]
pub(super) struct Review {
    target: Target,
    generation: u64,
}

fn begin(state: RwSignal<State>, target: Target) {
    let summary = state.with_untracked(|s| {
        target
            .current(s)
            .then(|| s.files.deletion.selected.clone())
            .flatten()
    });
    let Some(summary) = summary else {
        return;
    };
    let (generation, review_generation) = state
        .try_update(|s| {
            let model = &mut s.files.deletion;
            let generation = model.begin();
            (generation, model.review_generation)
        })
        .expect("invariant: mounted restore control owns its application state");
    spawn_local(async move {
        let endpoint = format!(
            "/api/files/{}/deletion",
            api::segment(summary.file_id.as_str())
        );
        let result = api::validated_request("GET", &endpoint, None, |value| {
            wire::unchanged_node(value, &summary, &target.node)
        })
        .await;
        if !accept_read_response(state, target.epoch, &result)
            || !state.with_untracked(|s| s.files.deletion.accepts(generation))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        if !state.with_untracked(|s| s.files.deletion.review_generation == review_generation) {
            return;
        }
        match result {
            Ok(Some(node)) => state.update(|s| {
                let node = Arc::new(node);
                s.files.deletion.node = Some(node.clone());
                s.files.deletion.restore_review = Some(Review {
                    target: Target {
                        epoch: target.epoch,
                        generation,
                        node,
                    },
                    generation: review_generation,
                });
            }),
            Ok(None) => {}
            Err(error) => fail(state, error),
        }
    });
}

fn reviewed(
    state: &State,
    epoch: u64,
    generation: u64,
) -> Option<Arc<storage_contract::FileDeletionNode>> {
    let review = state.files.deletion.restore_review.as_ref()?;
    (review.target.epoch == epoch
        && review.generation == generation
        && state.files.deletion.review_generation == generation
        && review.target.current(state))
    .then(|| review.target.node.clone())
}
fn confirm(state: RwSignal<State>, epoch: u64, generation: u64) {
    let Some(node) = state.with_untracked(|s| reviewed(s, epoch, generation)) else {
        return;
    };
    state.update(|s| s.files.deletion.cancel_review());
    // This preserves save-before-send, exact receipt matching and unknown-result
    // recovery. The fresh GET is not a receipt or permission to bypass them.
    undo(state, &node.file_id, &node.node_id);
}

pub(super) fn button(
    state: RwSignal<State>,
    node: Arc<storage_contract::FileDeletionNode>,
) -> impl IntoView {
    let target = StoredValue::new(Target {
        epoch: state.with_untracked(|s| s.session_epoch),
        generation: state.with_untracked(|s| s.files.deletion.generation),
        node,
    });
    view! {
        <button class="zxs-button" data-zxs-review-file-restore=""
            disabled=move ||!state.with(|s|target.with_value(|target|target.current(s)))
            on:click=move |_|begin(state,target.get_value())>
            {t("核对并恢复此文件…", "Review restoring this file…")}
        </button>
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s|s.files.deletion.restore_review.clone()).map(|review| {
            let epoch = review.target.epoch;
            let generation = review.generation;
            let node = review.target.node;
            view! {
                <section class="zxs-surface zxs-stack" data-zxs-file-restore-review="">
                    <h3>{t("确认撤销此删除节点", "Confirm undoing this deletion node")}</h3>
                    <p><code>{node.path.as_str().to_owned()}</code></p>
                    <p><code>{node.file_id.clone()}</code>" · "<code>{node.node_id.clone()}</code></p>
                    <p>{t("刚才已重新读取并完整核对同一个删除节点。确认后只撤销此 D，解除同一文件全部保留路径与分支的删除冻结；不会创建正文、切换目录分支、提交目录或回滚其它文件。当前所有者、权限和准确节点仍由服务端在发布时核验。", "The same complete deletion node was read and checked again. Confirmation only removes this D and releases the deletion freeze for all retained paths and branches of this file. It does not create content, switch folder branches, commit a folder or roll back other files. The server rechecks current owners, permission and the exact node when publishing.")}</p>
                    <div class="zxs-row">
                        <button class="zxs-button" data-undo-file-deletion=""
                            disabled=move ||state.with(|s|reviewed(s,epoch,generation).is_none())
                            on:click=move |_|confirm(state,epoch,generation)>
                            {t("确认恢复这个文件", "Confirm restoring this file")}
                        </button>
                        <button class="zxs-button" on:click=move |_|state.update(|s| {
                            if s.session_epoch==epoch && s.files.deletion.restore_review.as_ref().is_some_and(|review|review.generation==generation) {
                                s.files.deletion.cancel_review();
                            }
                        })>{t("取消，不发送恢复请求", "Cancel without sending a restore request")}</button>
                    </div>
                    <p>{t("发送前先持久保留原请求。结果未知时保留同一操作 ID，只查询或经另一次明确审阅续发原请求；关闭这里不撤销已经发出的操作。", "The original request is retained before sending. An unknown outcome keeps the same operation ID for checking, or an explicitly reviewed original-request retry. Closing this panel does not cancel a request already sent.")}</p>
                </section>
            }
        })}
    }
}
