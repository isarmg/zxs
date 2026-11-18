use crate::files::directory_snapshot::{
    Candidate, Phase, Publication, Target, blocks_writes, can_initialize, can_register, can_review,
    can_submit, requests, toggle,
};
use crate::ui::State;
use storage_contract::snapshot_worktree::discovery::SnapshotHeadDiscovery;
use storage_contract::snapshot_worktree::draft_commit::DirectoryDraftExpectation;
use web_interface::{i18n::t, prelude::*};

fn candidate_view(state: RwSignal<State>, candidate: Candidate, generation: u64) -> impl IntoView {
    let entry = candidate.selected.target.entry;
    let label = match candidate.selected.expected {
        DirectoryDraftExpectation::Born { .. } => t("已有历史的修改稿", "Modified historical file"),
        DirectoryDraftExpectation::Unborn { .. } => {
            t("新文件，首次历史提交", "New file, first history")
        }
    };
    view! {
        <label class="zxs-frame zxs-flow" data-zxs-directory-draft=entry.to_string()>
            <input
                type="checkbox"
                aria-label=candidate.selected.target.path.as_str().to_owned()
                disabled=move ||!state.with(can_review)
                prop:checked=move ||state.with(|state| {
                    state.files.directory_snapshot.reviewed(state)
                        .is_some_and(|review| review.selected.contains(&entry))
                })
                on:change=move |event|toggle(state, entry, generation, event_target_checked(&event))
            />
            <span>{candidate.selected.target.path.as_str().to_owned()}</span>
            <span>{format!("{} · {}", label, crate::format::bytes(candidate.size))}</span>
        </label>
    }
}

fn review_view(state: RwSignal<State>) -> impl IntoView {
    move || {
        state.with(|state| {
        state.files.directory_snapshot.reviewed(state).map(|review| {
            (
                review.target.folder.is_none(),
                review.head.clone(),
                review.candidates.clone(),
                state.files.directory_snapshot.read_generation,
            )
        })
    }).map(|(at_root, head, candidates, generation)| {
        match head.head {
            SnapshotHeadDiscovery::NotInitialized => view! {
                <div class="zxs-stack" data-zxs-directory-snapshot-state="not_initialized">
                    <p class="zxs-row">
                        {t("此根目录尚未初始化 .zxs。", "This root has no .zxs repository yet.")}
                    </p>
                    <p class="zxs-row">{t(
                        "初始化当前支持空根目录，或本页完整列出的直属已有历史文件。若有子目录、无历史新文件、筛选或分页，请先使用空根目录初始化；不会替文件隐式提交。",
                        concat!(
                            "Initialize an empty root or a complete, unfiltered page of direct ",
                            "files with history. Subfolders, unborn files and pagination are ",
                            "not admitted. No file is implicitly committed.",
                        ),
                    )}</p>
                    <Show when=move ||at_root fallback=move ||view! {
                        <p role="status">{t("请返回仓库根目录初始化；不会在子目录创建另一个快照仓库。","Return to the repository root to initialize it. No snapshot repository is created in this folder.")}</p>
                    }><button
                        class="zxs-button"
                        data-zxs-directory-snapshot-initialize=""
                        disabled=move ||!state.with(can_initialize)
                        on:click=move |_|requests::initialize(state)
                    >
                        {t("初始化 .zxs 目录快照", "Initialize .zxs snapshots")}
                    </button></Show>
                </div>
            }.into_any(),
            SnapshotHeadDiscovery::Pending { operation } => view! {
                <p class="zxs-row" role="status" data-zxs-directory-snapshot-state="pending">
                    {format!("{} {operation}", t(
                        "已有初始化操作尚待完成，保留原操作并恢复，不重复初始化：",
                        concat!(
                            "Initialization is pending. Recover its original operation ",
                            "instead of initializing again:",
                        ),
                    ))}
                </p>
            }.into_any(),
            SnapshotHeadDiscovery::Head { snapshot, branch_name } => {
                let empty = candidates.is_empty();
                view! {
                    <div class="zxs-stack" data-zxs-directory-snapshot-state="head">
                        <dl class="zxs-stack">
                            <div>
                                <dt>{t("真实 .zxs 分支", "Actual .zxs branch")}</dt>
                                <dd>{branch_name.as_str().to_owned()}</dd>
                            </div>
                            <div>
                                <dt>{t("分支身份", "Branch identity")}</dt>
                                <dd>{snapshot.branch_id.to_string()}</dd>
                            </div>
                            <div>
                                <dt>{t("当前节点", "Current node")}</dt>
                                <dd>{snapshot.node_id.to_string()}</dd>
                            </div>
                        </dl>
                        <p class="zxs-row">{t(
                            "本页只读取直接文件。勾选后可保留准确的稿并前往同仓库其他文件夹；不会自动递归、选择未勾选的稿，或替换先前保留的修订。一次提交同时发布整个显式选集。",
                            "Review direct files on this page. Keep checked drafts to add files from other folders in this repository. No recursive selection or revision replacement. One commit publishes the explicit set.",
                        )}</p>
                        {empty.then(||view! {
                            <p class="zxs-row" role="status">{t(
                                "本页没有待提交工作稿。保存文件后刷新目录，再读取工作稿。",
                                concat!(
                                    "No pending drafts on this page. Save files, refresh the ",
                                    "folder, then review again.",
                                ),
                            )}</p>
                        })}
                        {candidates.into_iter().map(|candidate| {
                            candidate_view(state, candidate, generation)
                        }).collect_view()}
                        {super::picks::controls(state)}
                        <label class="zxs-stack">
                            {t("目录提交备注", "Directory commit message")}
                            <input
                                type="text"
                                maxlength="128"
                                prop:value=move ||state.with(|state| {
                                    state.files.directory_snapshot.message.clone()
                                })
                                disabled=move ||!state.with(can_review)
                                on:input=move |event|state.update(|state| {
                                    if can_review(state) {
                                        state.files.directory_snapshot.message =
                                            event_target_value(&event);
                                    }
                                })
                            />
                        </label>
                        <p class="zxs-row" role="status">{move ||state.with(|state| {
                            state.files.directory_snapshot.reviewed(state).map(|review| format!(
                                "{} {}", t("已选择", "Selected"), review.selected.len(),
                            ))
                        })}</p>
                        <button
                            class="zxs-button"
                            data-zxs-directory-snapshot-commit=""
                            disabled=move ||!state.with(can_submit)
                            on:click=move |_|requests::submit(state)
                        >
                            {t("一次提交所选工作稿", "Commit selected drafts together")}
                        </button>
                    </div>
                }.into_any()
            }
        }
    })
    }
}

pub(super) fn original_text<T: data_serialization::Serialize>(value: &T, label: String) -> AnyView {
    match json_codec::to_string(value) {
        Ok(text) => view! {
            <label>
                {label}
                <textarea class="zxs-input" readonly prop:value=text></textarea>
            </label>
        }.into_any(),
        Err(error) => view! {
            <p role="alert">{format!("{}: {error}", t("无法显示原回执", "Could not display this original"))}</p>
        }.into_any(),
    }
}

fn mount_result(
    state: RwSignal<State>, repository: storage_contract::RepositoryId,
    receipt: storage_contract::snapshot_worktree::mount_change::MountChangeReceipt,
) -> AnyView {
    use storage_contract::snapshot_worktree::mount_change::MountOutcome;
    let in_scope = state.with_untracked(|s| {
        s.files.directory_snapshot.owns(s)
            && Target::capture(s).is_some_and(|t| t.repository == repository)
    });
    if !in_scope { return ().into_any(); }
    let summary = match &receipt.outcome {
        MountOutcome::Hidden {} => t(
            "原操作已隐藏当时的挂载，实例和内容仍保留。请保留原回执。",
            "The original operation hid that occurrence. The instance and content remain. Keep the receipt.",
        ),
        MountOutcome::Restored { name } => format!("{}: {name}",
            t("原操作恢复时的实际名称", "Actual name at the original restore")),
        MountOutcome::Pending {} => t("原挂载操作仍在处理。", "The original mount change is pending."),
    };
    let original = original_text(&receipt, t(
        "确切原请求与结果，可复制", "Exact original request and result, ready to copy",
    ));
    view! {
        <div class="zxs-stack" data-zxs-mount-result="">
            <p class="zxs-row" role="status">{summary}</p>
            {original}
        </div>
    }.into_any()
}

fn mount_recovery(s: &State) -> Option<AnyView> {
    let m = &s.files.directory_snapshot;
    let p = m.pending.as_ref().filter(|_| m.owns(s))?;
    let super::Command::MountChange { request, .. } = &p.record.command else { return None; };
    let description = if p.phase == Phase::Settled {
        t("结果已核对，只需清理本机原件。", "Outcome confirmed. Only local cleanup remains.")
    } else if m.mount_pending == Some(request.operation_id) {
        t("上次查询或提交返回同一原操作仍在处理。", "The last response reported this original operation as pending.")
    } else {
        t("结果未确定。只有重新读到同一原请求仍在处理才可重发。",
            "Outcome not confirmed. Resending requires a fresh read of this exact pending request.")
    };
    let original = original_text(request, t("原挂载请求", "Original mount request"));
    Some(view! {
        <div class="zxs-stack" data-zxs-mount-original="">
            <p role="status">{description}</p>
            {original}
        </div>
    }.into_any())
}

fn publication_view(state: RwSignal<State>) -> impl IntoView {
    move || {
        state
            .with(|state| state.files.directory_snapshot.publication.clone())
            .map(|publication| {
                let (text, identity) = match publication {
                    Publication::WorktreeCreated { target, .. } => (
                        t(
                            "worktree 已创建；可读取精确实例状态。",
                            "Worktree created. Its exact instance status can now be read.",
                        ),
                        target.worktree_id.to_string(),
                    ),
                    Publication::WorktreeCommitted { snapshot, .. } => (
                        t(
                            "此 worktree 已显式提交到新目录节点。",
                            "This worktree was explicitly committed to a new directory node.",
                        ),
                        snapshot.node_id.to_string(),
                    ),
                    Publication::MountChanged { repository, receipt } => {
                        return mount_result(state, repository, receipt);
                    }
                    Publication::Registered(root) => (
                        t(
                            "空盘根已登记；可继续初始化 .zxs。",
                            "Disk root registered. You can now initialize .zxs.",
                        ),
                        root.repository_id.to_string(),
                    ),
                    Publication::Initialized(snapshot) => (
                        t(".zxs 已初始化。", ".zxs initialized."),
                        snapshot.node_id.to_string(),
                    ),
                    Publication::Committed(observation) => {
                        use storage_contract::snapshot_worktree::draft_commit::DirectoryDraftOutcome;
                        let text = match (observation.result,observation.degraded) {
                            (DirectoryDraftOutcome::Changed { .. },false) => t(
                                "所选工作稿已共同提交。未选稿保持原样。",
                                "Selected drafts committed together. Unselected drafts are unchanged.",
                            ),
                            (DirectoryDraftOutcome::Changed { .. },true) => t(
                                "所选工作稿已发布，但发布时保护不足，请检查磁盘状态。",
                                "Selected drafts published with reduced protection. Check disk status.",
                            ),
                            (DirectoryDraftOutcome::Unchanged { .. },false) => t(
                                "已核对原选集：内容未改变，没有创建新目录节点。",
                                "The original selection was checked. Content is unchanged; no new directory node was created.",
                            ),
                            (DirectoryDraftOutcome::Unchanged { .. },true) => t(
                                "原选集未产生新目录节点；发布时保护不足，请检查磁盘状态。",
                                "The original selection produced no new directory node. Protection was reduced; check disk status.",
                            ),
                        };
                        (text,observation.snapshot().node_id.to_string())
                    }
                };
                view! {
                    <p class="zxs-row" role="status" data-zxs-directory-snapshot-result="">
                        {format!("{text} {identity}")}
                    </p>
                }.into_any()
            })
    }
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| {
        let current = target.get();
        state.update(|state| {
            let model=&mut state.files.directory_snapshot;
            if let (Some(owner),Some(target))=(state.principal.as_ref(),current.as_ref())
                && model.kept.as_ref().is_some_and(|set| !set.same_owner(owner,target)) {
                model.kept=None;
            }
            model.clear_review();
        });
        // A new child folder is ordinary browsing, not consent to read up to
        // one page of individual working statuses. Nested reviews are explicit.
        if current.is_some_and(|t| t.folder.is_none()) {
            requests::load(state);
        }
    });
    on_cleanup(move || {
        let _ = state.try_update(|state| state.files.directory_snapshot.clear_review());
    });
    view! {
        <section class="zxs-entry-section zxs-stack" data-zxs-directory-snapshot="">
            <h3 class="zxs-row">{t(".zxs 目录快照", ".zxs directory snapshots")}</h3>
            <Show when=move ||state.with(can_register)>
                <p class="zxs-row">{t(
                    "此空盘尚未登记文件管理根。先登记真实根身份，再独立初始化 .zxs；这一步不创建文件历史。",
                    concat!(
                        "This empty disk has no registered file-manager root. Register its real ",
                        "identity, then initialize .zxs separately. This creates no file history.",
                    ),
                )}</p>
                <button
                    class="zxs-button"
                    data-zxs-directory-snapshot-register=""
                    on:click=move |_|requests::register(state)
                >
                    {t("登记空盘根", "Register empty disk root")}
                </button>
            </Show>
            <Show
                when=move ||target.get().is_some()
                fallback=move ||view! {
                    <p class="zxs-row">{t(
                        "此入口要求已登记的根目录。全新空盘先使用“登记空盘根”；子目录和历史预览暂不支持正式 .zxs 初始化，不调用旧目录仓库写接口。",
                        concat!(
                            "This entry requires a registered root. On a new empty disk, use ",
                            "Register empty disk root first. Subfolders and historical previews ",
                            "do not ",
                            "support formal .zxs initialization here.",
                        ),
                    )}</p>
                }
            >
                <button
                    class="zxs-button"
                    data-zxs-directory-snapshot-review=""
                    disabled=move ||!state.with(can_review)
                    on:click=move |_|requests::load(state)
                >
                    {t("刷新真实分支与本页工作稿", "Refresh actual branch and this page's drafts")}
                </button>
                <Show when=move ||state.with(|state| {
                    state.files.directory_snapshot.reading.is_some()
                })>
                    <p class="zxs-row" role="status">
                        {t("正在读取真实分支和工作稿…", "Reading the actual branch and drafts…")}
                    </p>
                </Show>
                {review_view(state)}
                {publication_view(state)}
                <Show when=move ||state.with(super::worktree::available)>
                    {super::worktree::view(state)}
                </Show>
            </Show>
            {move ||state.with(|state| {
                let model = &state.files.directory_snapshot;
                if model.pending.is_none() { model.error.clone() } else { None }
            }).map(|error|view! {<p role="alert">{error.to_string()}</p>})}
        </section>
    }
}

pub(crate) fn recovery_view(state: RwSignal<State>) -> impl IntoView {
    let observed_original = Memo::new(move |_| state.with(requests::observe_restored));
    Effect::new(move |_| {
        if observed_original.get().is_some() { requests::lookup(state); }
    });
    view! {
        <Show when=move ||state.with(blocks_writes)>
            <aside class="zxs-surface zxs-stack" data-zxs-directory-snapshot-recovery="">
                <h2>{t("目录快照 / worktree：原请求恢复", "Snapshot / worktree: original-request recovery")}</h2>
                {move || state.with(mount_recovery)}
                <p>{t(
                    "保留原账号、目录、分支、选集及操作编号。刷新不自动重发；查询只读永久回执，重试只发送原请求，不改选最新工作稿。",
                    concat!(
                        "The account, directory, branch, selection and operation ID are retained. ",
                        "Refresh never resubmits. Look up the receipt or retry the exact ",
                        "original request without selecting newer drafts.",
                    ),
                )}</p>
                <p>{move ||state.with(|state| format!(
                    "{} {}", t("保留的原请求数：", "Retained original requests:"),
                    state.files.directory_snapshot.retained_count,
                ))}</p>
                <p>{move ||state.with(|state| {
                    state.files.directory_snapshot.pending.as_ref()
                        .map(|pending| pending.record.command.operation().to_string())
                })}</p>
                <button
                    class="zxs-button"
                    data-zxs-directory-snapshot-outcome=""
                    disabled=move ||state.with(|state| {
                        requests::retry_record(state)
                            .and_then(|record| record.command.outcome_endpoint()).is_none()
                    })
                    on:click=move |_|requests::lookup(state)
                >
                    {t("查询原操作结果", "Look up original operation result")}
                </button>
                <Show when=move ||state.with(|state| {
                    state.files.directory_snapshot.pending.as_ref()
                        .is_some_and(|pending| !pending.record.command.can_post())
                })>
                    <p>{t(
                        "此手动命名 worktree 原请求只可查询结果，不再重发创建；原请求保留，不改成新操作。",
                        concat!(
                            "This retained named-worktree request is read-only. Query its ",
                            "original result; creation cannot be retried. Its original bytes ",
                            "are retained, not replaced with a new operation.",
                        ),
                    )}</p>
                </Show>
                {move ||state.with(|s| {
                    let m=&s.files.directory_snapshot;
                    let p=m.pending.as_ref()?;
                    let (original,observation)=m.observed.as_ref()?;
                    (m.owns(s) && s.principal.as_ref()==Some(&original.principal)
                        && std::sync::Arc::ptr_eq(&p.record,original)).then_some(*observation)
                }).map(|observation|view! {
                    <p role="status" data-zxs-directory-scoped-receipt="">
                        {format!("{} {}. {}",
                            t("已查到此账号、原目录和操作编号的节点：","Found the node for this account, original directory and operation:"),
                            observation.snapshot().node_id,
                            t("查询不核对完整选集和备注，原请求仍保留。可写时显式重试原请求以完成核对；本次查询不重发。","The lookup does not compare every selected draft or the note. The original is retained. When writable, explicitly retry it to verify the full request; this lookup sends no write."))
                        }
                        {observation.degraded.then(||view!{<strong>{t(" 发布时保护不足，请检查磁盘状态。"," Protection was reduced at publication. Check disk status.")}</strong>})}
                    </p>
                })}
                <button
                    class="zxs-button"
                    data-zxs-directory-snapshot-retry=""
                    disabled=move ||state.with(|state| requests::retry_post_record(state).is_none())
                    on:click=move |_|requests::retry(state)
                >
                    {t("重试原请求", "Retry original request")}
                </button>
                <Show when=move ||state.with(|state| {
                    state.files.directory_snapshot.pending.as_ref()
                        .is_some_and(|pending| pending.phase == Phase::Settled)
                })>
                    <p>{t(
                        "原结果已确认，仅本机记录清理待完成。",
                        "The result is confirmed; local recovery cleanup remains.",
                    )}</p>
                    <button class="zxs-button" data-zxs-directory-snapshot-cleanup="" on:click=move |_|requests::cleanup(state)>
                        {t("重试本机记录清理", "Retry local cleanup")}
                    </button>
                </Show>
                <button
                    class="zxs-button"
                    disabled=move ||state.with(|state| state.files.mutation_pending)
                    on:click=move |_|requests::restore(state)
                >
                    {t("重新读取本机原请求", "Reload local original request")}
                </button>
                {move ||state.with(|state| state.files.directory_snapshot.error.clone())
                    .map(|error|view! {<p role="alert">{error.to_string()}</p>})}
            </aside>
        </Show>
    }
}
