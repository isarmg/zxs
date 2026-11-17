use crate::files::directory_snapshot::worktree::history::{
    can_read, load, next, next_cursor, select, select_head, source,
};
use crate::ui::State;
use storage_contract::snapshot_worktree::history::SnapshotHistoryNode;
use web_interface::{i18n::t, prelude::*};

fn row(state: RwSignal<State>, node: SnapshotHistoryNode) -> impl IntoView {
    let target = node.snapshot;
    let parent = match node.parent {
        Some(parent) => parent.node_id.to_string(),
        None => t("初始化节点，无父节点", "Initial node, no parent"),
    };
    let head = if node.is_head {
        t("分支 HEAD", "Branch HEAD")
    } else {
        String::new()
    };
    view! {
        <li class="zxs-stack" data-zxs-snapshot-history-node="">
            <p>{format!("{} · {} {}", node.branch_name.as_str(), target.node_id, head)}</p>
            <p>{format!("{} {}", t("父节点：", "Parent:"), parent)}</p>
            <button
                class="zxs-button"
                data-zxs-snapshot-history-select=""
                disabled=move ||!state.with(can_read)
                on:click=move |_|state.update(|state| select(state, target))
            >
                {t("选此节点作为新实例来源", "Use this node for a new instance")}
            </button>
        </li>
    }
}

fn rows(state: RwSignal<State>) -> impl IntoView {
    move || {
        state
            .with(|state| {
                state
                    .files
                    .directory_snapshot
                    .worktree
                    .history
                    .page(state)
                    .map(|page| page.value.nodes.clone())
            })
            .map(|nodes| {
                if nodes.is_empty() {
                    view! {
                    <p>{t(
                        "没有可列出的已完成节点；这不是重新初始化或重建的许可。",
                        "No completed nodes were listed. This does not authorize initialization.",
                    )}</p>
                }.into_any()
                } else {
                    view! {
                        <ul class="zxs-stack">
                            {nodes.into_iter().map(|node| row(state, node)).collect_view()}
                        </ul>
                    }
                    .into_any()
                }
            })
    }
}

fn selection(state: &State) -> String {
    let model = &state.files.directory_snapshot.worktree.history;
    if let Some(selected) = &model.selected {
        let label = format!(
            "{} {} · {}",
            t("已选历史来源：", "Selected historical source:"),
            selected.node.branch_name.as_str(),
            selected.node.snapshot.node_id,
        );
        if source(state).is_some() {
            label
        } else {
            format!(
                "{label} {}",
                t(
                    "当前根已变，请重读后重新选择；不会自动换成 HEAD。",
                    "The root changed. Read and select again; HEAD is not substituted.",
                )
            )
        }
    } else if let Some((snapshot, _)) = source(state) {
        format!(
            "{} {}",
            t("使用上面已读取的 HEAD：", "Using the reviewed HEAD:"),
            snapshot.node_id
        )
    } else {
        t(
            "先读取当前根的真实分支。",
            "Read this root's actual branch first.",
        )
    }
}

pub(in crate::files::directory_snapshot::worktree) fn view(
    state: RwSignal<State>,
) -> impl IntoView {
    view! {
        <div class="zxs-stack" data-zxs-snapshot-history="">
            <p>{t(
                concat!(
                    "历史按真实节点发布顺序分批读取，每页最多 100 项。可选择非 HEAD 的旧节点；",
                    "创建请求保留完整来源身份。分页期间有新写入时，请从首页重新读取，不拼接旧页。",
                ),
                concat!(
                    "History is paged by actual node publication order, up to 100 nodes per ",
                    "page. Older non-HEAD nodes can be selected; creation retains the complete ",
                    "source identity. If writes change the page prefix, restart from the first ",
                    "page rather than combining generations.",
                ),
            )}</p>
            <button
                class="zxs-button"
                data-zxs-snapshot-history-first=""
                disabled=move ||!state.with(can_read)
                on:click=move |_|load(state, None)
            >
                {t("读取 / 刷新历史首页", "Read / refresh history from the start")}
            </button>
            <button
                class="zxs-button"
                data-zxs-snapshot-history-next=""
                disabled=move ||state.with(next_cursor).is_none()
                on:click=move |_|next(state)
            >
                {t("下一页历史节点", "Next page of historical nodes")}
            </button>
            <Show when=move ||state.with(|state| {
                state.files.directory_snapshot.worktree.history.reading
            })>
                <p role="status">{t("正在读取真实历史…", "Reading actual history…")}</p>
            </Show>
            {rows(state)}
            <p data-zxs-snapshot-history-selection="">{move ||state.with(selection)}</p>
            <button
                class="zxs-button"
                data-zxs-snapshot-history-head=""
                disabled=move ||!state.with(can_read)
                on:click=move |_|state.update(select_head)
            >
                {t("改用上面已读取的 HEAD", "Use the reviewed HEAD instead")}
            </button>
        </div>
    }
}
