use crate::files::directory_snapshot::worktree::instances::{can_read, load, open};
use crate::ui::State;
use storage_contract::snapshot_worktree::WorktreeStatus;
use web_interface::{i18n::t, prelude::*};

fn row(state: RwSignal<State>, status: WorktreeStatus) -> impl IntoView {
    let target = status.target;
    view! {
        <li class="zxs-stack" data-zxs-worktree-instance="">
            <p>{status.name.as_str().to_owned()}</p>
            <p>{format!(
                "{} {} · {} {}",
                t("来源节点：", "Source node:"), status.original.node_id,
                t("已保存代数：", "Saved generation:"), status.revision.generation,
            )}</p>
            <button
                class="zxs-button"
                data-zxs-worktree-instance-open=""
                disabled=move ||!state.with(can_read)
                on:click=move |_|open(state, target)
            >
                {t("打开此已有实例", "Open this existing instance")}
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
                    .instances
                    .listing(state)
                    .map(|listing| listing.value.instances.clone())
            })
            .map(|instances| {
                if instances.is_empty() {
                    view! {<p>{t(
                        "当前根没有可列出的已完成实例。",
                        "No completed instances are listed for this root.",
                    )}</p>}
                    .into_any()
                } else {
                    view! {
                        <ul class="zxs-stack">
                            {instances.into_iter().map(|status| row(state, status)).collect_view()}
                        </ul>
                    }
                    .into_any()
                }
            })
    }
}

pub(in crate::files::directory_snapshot::worktree) fn view(
    state: RwSignal<State>,
) -> impl IntoView {
    view! {
        <div class="zxs-stack" data-zxs-worktree-instances="">
            <p>{t(
                concat!(
                    "也可以打开本根以前创建的实例，不限最近一个。这里返回完整列表，上限 100 个；",
                    "超过上限或存在未完成登记时暂拒绝，不截断、不隐藏后续项。",
                    "读取会核验各实例及来源内容，可能需要等待。",
                ),
                concat!(
                    "Open an earlier instance from this root, not only the most recent one. ",
                    "This complete list is limited to 100 instances. Over-limit or pending ",
                    "registrations are rejected, not truncated or hidden. Reading verifies ",
                    "each instance and its source content, which can take time.",
                ),
            )}</p>
            <button
                class="zxs-button"
                data-zxs-worktree-instances-refresh=""
                disabled=move ||!state.with(can_read)
                on:click=move |_|load(state)
            >
                {t("读取 / 刷新已有实例", "Read / refresh existing instances")}
            </button>
            <Show when=move ||state.with(|state| {
                state.files.directory_snapshot.worktree.instances.reading
            })>
                <p role="status">{t("正在核验已有实例…", "Verifying existing instances…")}</p>
            </Show>
            {rows(state)}
            <p>{t(
                "打开只查询精确实例的最新状态，不创建、不保存、不提交，也不改变新建实例的历史来源选择。",
                concat!(
                    "Opening only reads that exact instance's current status. It does not ",
                    "create, save or commit anything, or change the history source selected ",
                    "for a new instance.",
                ),
            )}</p>
        </div>
    }
}
