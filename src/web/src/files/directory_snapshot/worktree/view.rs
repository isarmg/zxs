use super::{available, can_commit, can_create, can_load, history, requests, smb_path};
use crate::ui::State;
use web_interface::{i18n::t, prelude::*};

fn observed(state: RwSignal<State>) -> impl IntoView {
    move || {
        state.with(|state| {
            let model = &state.files.directory_snapshot.worktree;
            model.observed(state).map(|observed| (observed.status.clone(), smb_path(state)))
        }).map(|(status, path)| {
            view! {
                <div class="zxs-stack" data-zxs-worktree-status="">
                    <dl class="zxs-stack">
                        <div>
                            <dt>{t("默认 SMB 共享内路径", "Path within the default SMB share")}</dt>
                            <dd data-zxs-worktree-smb-path="">{path.unwrap_or_else(|| t(
                                "尚未读取当前磁盘名，请刷新目录。",
                                "Refresh the folder to read its current disk name.",
                            ))}</dd>
                        </div>
                        <div>
                            <dt>{t("原始目录节点", "Original directory node")}</dt>
                            <dd>{status.original.node_id.to_string()}</dd>
                        </div>
                        <div>
                            <dt>{t("已保存工作稿代数", "Saved worktree generation")}</dt>
                            <dd>{status.revision.generation.get().to_string()}</dd>
                        </div>
                    </dl>
                    <p class="zxs-row">{t(
                        concat!(
                            "可在上述 SMB 路径保存已有文件。普通保存只更新此实例工作稿，不创建目录历史节点；",
                            "保存后先刷新此实例，再显式提交。当前仅支持同一个原始文件的完整覆盖保存，",
                            "新增、删除和改名不在此入口范围。",
                        ),
                        concat!(
                            "Save an existing file through this SMB path. Ordinary saves update ",
                            "only this instance, not directory history. Refresh this instance ",
                            "after saving, then commit explicitly. This slice supports complete ",
                            "replacement saves of one original file, not add/delete/rename.",
                        ),
                    )}</p>
                    <label class="zxs-stack">
                        {t("worktree 提交备注", "Worktree commit message")}
                        <input
                            type="text"
                            maxlength="128"
                            data-zxs-worktree-message=""
                            prop:value=move ||state.with(|state| {
                                state.files.directory_snapshot.worktree.message.clone()
                            })
                            disabled=move ||!state.with(available)
                            on:input=move |event|state.update(|state| {
                                if available(state) {
                                    state.files.directory_snapshot.worktree.message =
                                        event_target_value(&event);
                                }
                            })
                        />
                    </label>
                    <p class="zxs-row">{t(
                        concat!(
                            "提交只使用上面已打开的精确实例和已读取的工作稿代数，不使用新建实例的历史来源选择。",
                            "原来源仍为其分支 HEAD 时续原分支，否则从该精确来源创建新分支；",
                            "新分支使用该实例已分配的名称。名称超过 128 字节或重名时服务器拒绝，",
                            "不截断、不覆盖；没有修改或布局不受支持也不会改选最新内容。",
                        ),
                        concat!(
                            "Commit uses the exact opened instance and reviewed saved generation, ",
                            "not the source selected for a new instance. If its source remains ",
                            "branch HEAD, the server continues that branch; otherwise it forks ",
                            "from that exact source using this instance's allocated name. Names ",
                            "over 128 bytes or already occupied are rejected, never shortened ",
                            "or replaced. The server never substitutes newer content ",
                            "for this writer.",
                        ),
                    )}</p>
                    <button
                        class="zxs-button"
                        data-zxs-worktree-commit=""
                        disabled=move ||!state.with(can_commit)
                        on:click=move |_|requests::commit(state)
                    >
                        {t("显式提交此 worktree", "Commit this worktree explicitly")}
                    </button>
                </div>
            }
        })
    }
}

pub(in crate::files::directory_snapshot) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-surface zxs-stack" data-zxs-worktree="">
            <h3 class="zxs-row">{t("目录历史与 worktree", "Directory history and worktrees")}</h3>
            <p class="zxs-row">{t(
                concat!(
                    "从上面已读取的 .zxs HEAD 或下方选定的真实历史节点创建独立实例。",
                    "服务器按源的真实分支名自动分配 worktree-分支名-001 至 999，",
                    "已占用或保留名称不会被覆盖。选择来源不会切换已经打开的 worktree。",
                ),
                concat!(
                    "Create from the reviewed .zxs HEAD or an exact historical node below. ",
                    "The server allocates worktree-branch-001 through 999 using the source's ",
                    "real branch name, without replacing occupied or reserved names. ",
                    "Selecting a source does not switch the already opened worktree.",
                ),
            )}</p>
            <p class="zxs-row">{t(
                "当前写入限初始登记根布局。已有 Current 命名空间或配置代次的布局会被服务器拒绝；本页不会切换或迁移布局。",
                concat!(
                    "Writes currently require the initial registered root layout. Published ",
                    "Current namespaces or configuration generations are rejected by the ",
                    "server. This page does not switch or migrate layouts.",
                ),
            )}</p>
            {history::view(state)}
            <button
                class="zxs-button"
                data-zxs-worktree-create=""
                disabled=move ||!state.with(can_create)
                on:click=move |_|requests::create(state)
            >
                {t("自动命名并创建 worktree", "Allocate a name and create a worktree")}
            </button>
            <p class="zxs-row">{t(
                concat!(
                    "此浏览器只记住当前账号、当前目录最近创建或提交的实例。",
                    "刷新后仅查询原实例，不重新创建；这不是全部 worktree 列表，",
                    "无需填写内部身份。名称和 SMB 路径以服务器实际状态为准。",
                ),
                concat!(
                    "This browser remembers only this account and folder's most recently ",
                    "created or committed instance. Refresh only reads that exact instance; ",
                    "it never creates another one. This is not a complete worktree list and ",
                    "no internal identifiers need to be entered. The server supplies its ",
                    "actual name and SMB path.",
                ),
            )}</p>
            <button
                class="zxs-button"
                data-zxs-worktree-refresh=""
                disabled=move ||!state.with(can_load)
                on:click=move |_|requests::load(state)
            >
                {t("读取 / 刷新精确实例", "Read / refresh the exact instance")}
            </button>
            <Show when=move ||state.with(|state| {
                state.files.directory_snapshot.worktree.reading
            })>
                <p role="status">{t("正在读取实例状态…", "Reading instance status…")}</p>
            </Show>
            {observed(state)}
        </section>
    }
}
