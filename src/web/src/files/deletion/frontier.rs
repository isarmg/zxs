//! Branch pages stay attached to the exact deletion record and frozen frontier.
use crate::files::deletion::{Inspection, batch_restore, restore};
use crate::ui::State;
use std::{ops::Range, sync::Arc};
use storage_contract::FileDeletionNode;
use web_interface::{i18n::t, prelude::*};

const PAGE_SIZE: usize = 100;

#[derive(Clone)]
struct Target {
    epoch: u64,
    generation: u64,
    node: Arc<FileDeletionNode>,
}
#[derive(Clone, Copy)]
enum Turn {
    Previous,
    Next,
}
impl Target {
    fn range(&self, state: &State) -> Option<Range<usize>> {
        let model = &state.files.deletion;
        if !state.session_active()
            || state.session_epoch != self.epoch
            || !model.accepts(self.generation)
            || model.loading
            || !model
                .node
                .as_ref()
                .and_then(Inspection::complete)
                .is_some_and(|node| Arc::ptr_eq(node, &self.node))
        {
            return None;
        }
        let start = model.frontier_page * PAGE_SIZE;
        Some(start..(start + PAGE_SIZE).min(self.node.branch_heads.len()))
    }

    fn destination(&self, state: &State, turn: Turn) -> Option<usize> {
        let range = self.range(state)?;
        let page = state.files.deletion.frontier_page;
        match turn {
            Turn::Previous => (range.start > 0).then(|| page - 1),
            Turn::Next => (range.end < self.node.branch_heads.len()).then_some(page + 1),
        }
    }

    fn turn(&self, state: &mut State, turn: Turn) {
        if let Some(page) = self.destination(state, turn) {
            state.files.deletion.frontier_page = page;
        }
    }
}

pub(super) fn view(state: RwSignal<State>, node: Arc<FileDeletionNode>) -> impl IntoView {
    let target = StoredValue::new(Target {
        epoch: state.with_untracked(|s| s.session_epoch),
        generation: state.with_untracked(|s| s.files.deletion.generation),
        node: node.clone(),
    });
    let restore_button = restore::button(state, node.clone());
    let batch_restore_button = batch_restore::button(state, node.clone());
    view! {
        <section
            class="zxs-stack"
            data-deletion-node=node.node_id.clone()
        >
            <h3>
                {format!("{} · {}", t("删除记录", "Deletion record"), node.path.as_str())}
            </h3>
            <p>
                {t(
                    concat!(
                        "独立删除记录保存所有分支的冻结末端。",
                        "它不是内容历史节点，不创建祖先边，也不改变分支 HEAD。",
                    ),
                    concat!(
                        "This independent deletion record retains every frozen branch tip. ",
                        "It is not a content-history node and creates no ancestry edges ",
                        "or changes to branch HEADs.",
                    ),
                )}
            </p>
            {evidence(node.clone())}
            {branch_frontier(state, target)}
            {pagination(state, target)}
            {restore_button}
            {batch_restore_button}
        </section>
    }
}

fn evidence(node: Arc<FileDeletionNode>) -> impl IntoView {
    view! {
        <details class="zxs-stack">
            <summary>{t("核对文件身份与删除依据", "Inspect file identity and deletion evidence")}</summary>
            <dl>
                <dt>{t("文件 ID", "File ID")}</dt>
                <dd>
                    <code>{node.file_id.clone()}</code>
                </dd>
                <dt>{t("仓库 ID", "Repository ID")}</dt>
                <dd>
                    <code>{node.repository_id.clone()}</code>
                </dd>
                <dt>{t("删除记录 ID", "Deletion record ID")}</dt>
                <dd>
                    <code>{node.node_id.clone()}</code>
                </dd>
                <dt>{t("原删除操作", "Original deletion operation")}</dt>
                <dd>
                    <code>{node.operation_id.to_string()}</code>
                </dd>
                <dt>{t("删除主体", "Deleted by")}</dt>
                <dd>{node.author.clone()}</dd>
                <dt>{t("发布序号", "Publication sequence")}</dt>
                <dd>{node.sequence.to_string()}</dd>
                <dt>{t("保留分支 / 去重冻结末端", "Retained branches / distinct frozen tips")}</dt>
                <dd>
                    {format!("{} / {}", node.branch_heads.len(), node.parent_node_ids.len())}
                </dd>
            </dl>
        </details>
    }
}

fn branch_frontier(state: RwSignal<State>, target: StoredValue<Target>) -> impl IntoView {
    view! {
        <div class="zxs-table-scroll">
            <table
                class="zxs-table"
                aria-label=t(
                    "删除记录保存的全部分支冻结前沿",
                    "Complete frozen frontier of the deletion record",
                )
            >
                <thead>
                    <tr>
                        <th>{t("分支", "Branch")}</th>
                        <th>{t("冻结末端节点", "Frozen tip node")}</th>
                        <th>{t("删除记录", "Deletion record")}</th>
                    </tr>
                </thead>
                <tbody>
                    {move || target.with_value(|target| {
                        state.with(|state| target.range(state)).map(|range| {
                            target
                                .node
                                .branch_heads
                                .iter()
                                .skip(range.start)
                                .take(range.len())
                                .map(|(branch, head)| view! {
                                    <tr data-deletion-parent=head.clone()>
                                        <td>
                                            <code>{branch.clone()}</code>
                                        </td>
                                        <td>
                                            <code>{head.clone()}</code>
                                        </td>
                                        <td>
                                            <code>{target.node.node_id.clone()}</code>
                                        </td>
                                    </tr>
                                })
                                .collect_view()
                        })
                    })}
                </tbody>
            </table>
        </div>
    }
}

fn pagination(state: RwSignal<State>, target: StoredValue<Target>) -> impl IntoView {
    let multiple = target.with_value(|target| target.node.branch_heads.len() > PAGE_SIZE);
    view! {
        <p
            role="status"
            data-deletion-branch-range=""
        >
            {move || target.with_value(|target| {
                state.with(|state| target.range(state)).map(|range| format!(
                    "{} {}-{} / {}",
                    t("分支", "Branches"),
                    range.start + 1,
                    range.end,
                    target.node.branch_heads.len(),
                ))
            })}
        </p>
        <Show when=move || multiple>
            <div class="zxs-row">
                {page_button(state, target, Turn::Previous)}
                {page_button(state, target, Turn::Next)}
            </div>
        </Show>
    }
}

fn page_button(
    state: RwSignal<State>,
    target: StoredValue<Target>,
    turn: Turn,
) -> impl IntoView {
    let label = match turn {
        Turn::Previous => t("上一页分支", "Previous branches"),
        Turn::Next => t("下一页分支", "Next branches"),
    };
    view! {
        <button
            class="zxs-button"
            disabled=move || state.with(|state| {
                target.with_value(|target| target.destination(state, turn).is_none())
            })
            on:click=move |_| state.update(|state| {
                target.with_value(|target| target.turn(state, turn))
            })
        >
            {label}
        </button>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/frontier_tests.rs"]
mod tests;
