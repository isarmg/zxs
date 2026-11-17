use super::{available, can_commit, can_create, can_load, edit_handle, requests, smb_path};
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
                        "提交使用上面精确实例和已读取的代数。仅原来源仍为当前 HEAD 时可提交；没有修改、文件基线变化或不受支持的布局会由服务器拒绝，不改选最新内容。",
                        concat!(
                            "Commit uses this exact instance and the reviewed generation. ",
                            "Its original source must still be HEAD. The server rejects ",
                            "unchanged content, changed file bases or unsupported layouts; ",
                            "it never substitutes newer content.",
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
            <h3 class="zxs-row">{t("当前节点的 worktree", "Worktrees from the current node")}</h3>
            <p class="zxs-row">{t(
                "仅从上面读取的真实 .zxs HEAD 创建独立实例，不使用旧目录 Git。这里尚不浏览任意历史节点，也不自动分配 001–999 名称。",
                concat!(
                    "Create an independent instance from the actual .zxs HEAD above, not ",
                    "legacy directory Git. Arbitrary history browsing and automatic ",
                    "001–999 name allocation are not provided here.",
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
            <label class="zxs-stack">
                {t("手动填写新 worktree 名称", "Enter the new worktree name manually")}
                <input
                    type="text"
                    data-zxs-worktree-name=""
                    prop:value=move ||state.with(|state| {
                        state.files.directory_snapshot.worktree.name.clone()
                    })
                    disabled=move ||!state.with(available)
                    on:input=move |event|state.update(|state| {
                        if available(state) {
                            state.files.directory_snapshot.worktree.name =
                                event_target_value(&event);
                        }
                    })
                />
            </label>
            <button
                class="zxs-button"
                data-zxs-worktree-create=""
                disabled=move ||!state.with(can_create)
                on:click=move |_|requests::create(state)
            >
                {t("从当前真实节点创建 worktree", "Create a worktree from the actual current node")}
            </button>
            <p class="zxs-row">{t(
                "名称按实际目标预算校验，重名不覆盖。下列两项共同标识已创建实例；可复制保留，重新打开时填写后只读查询。名称本身不是实例身份。",
                concat!(
                    "The server checks the target name budget and never replaces a collision. ",
                    "Keep both identifiers below to reopen this exact instance with a read-only ",
                    "lookup. A display name is not its identity.",
                ),
            )}</p>
            <label class="zxs-stack">
                {t("worktree 身份", "Worktree identity")}
                <input
                    type="text"
                    autocomplete="off"
                    data-zxs-worktree-id=""
                    prop:value=move ||state.with(|state| {
                        state.files.directory_snapshot.worktree.handle_text.clone()
                    })
                    disabled=move ||!state.with(available)
                    on:input=move |event|edit_handle(state, false, event_target_value(&event))
                />
            </label>
            <label class="zxs-stack">
                {t("创建完成回执身份", "Creation completion identity")}
                <input
                    type="text"
                    autocomplete="off"
                    data-zxs-worktree-publication=""
                    prop:value=move ||state.with(|state| {
                        state.files.directory_snapshot.worktree.publication_text.clone()
                    })
                    disabled=move ||!state.with(available)
                    on:input=move |event|edit_handle(state, true, event_target_value(&event))
                />
            </label>
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
