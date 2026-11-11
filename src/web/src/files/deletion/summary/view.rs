use crate::files::deletion::summary::{AuditPage, Model, Read, Target, read, restore};
use crate::files::deletion::{Inspection, restore as file_restore};
use crate::ui::State;
use web_interface::{i18n::t, prelude::*};

pub(in crate::files::deletion) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        {move ||state.with(|s|s.files.deletion.node.as_ref()
            .and_then(Inspection::paged).cloned()).map(|model| inspection(state,model))}
        {move ||state.with(|s|match &s.files.deletion.restore_review {
            Some(file_restore::Review::Summary(review)) => Some(review.clone()),
            Some(file_restore::Review::Single(_))
            | Some(file_restore::Review::Batch(_)) | None => None,
        }).map(|review| confirmation(state,review))}
    }
}

fn inspection(state: RwSignal<State>, model: Model) -> impl IntoView {
    let target = StoredValue::new(Target {
        epoch: state.with_untracked(|s|s.session_epoch),
        generation: state.with_untracked(|s|s.files.deletion.generation),
        summary: model.summary.clone(),
    });
    let summary = model.summary;
    view! {
        <section class="zxs-stack" data-deletion-summary=summary.node_id.to_string()>
            <h3>{t("大型删除节点审阅", "Large deletion audit")}</h3>
            <p><code>{summary.path.as_str().to_owned()}</code></p>
            <p>
                <code>{summary.file_id.to_string()}</code>" · "
                <code>{summary.node_id.to_string()}</code>
            </p>
            <p>
                <code>{summary.repository_id.to_string()}</code>" · "
                <code>{summary.history_tree_id.to_string()}</code>
            </p>
            <p>{format!("{} · {} · {} · {}",
                summary.operation_id, summary.author, summary.sequence, summary.created_at)}</p>
            <p>{format!("{}: {} / {}",
                t("分支 / 去重父边", "Branches / distinct parents"),
                summary.branch_count, summary.parent_count)}</p>
            <p>{t(
                "完整记录超过单次读取预算。分支和父边可分别逐页审阅；原 D 和全部审计仍完整保留。",
                concat!(
                    "The complete record exceeds one response budget. ",
                    "Review branches and parents in separate pages; ",
                    "the original D and complete audit remain retained.",
                ),
            )}</p>
            <div class="zxs-row">
                {read_button(state,target,Read::Branches { after: None, start: 0 },
                    t("分支首页", "First branch page"))}
                {read_button(state,target,Read::Parents { offset: 0 },
                    t("父边首页", "First parent page"))}
            </div>
            {audit_page(state,target,model.page)}
            <button
                class="zxs-button"
                data-zxs-review-summary-restore=""
                disabled=move ||!state.with(|s| {
                    target.with_value(|target|restore::can_review(s,target))
                })
                on:click=move |_|restore::begin(state,target.get_value())
            >
                {t("重新核对摘要并恢复此文件…", "Review the summary and restore this file…")}
            </button>
        </section>
    }
}

fn audit_page(
    state: RwSignal<State>,
    target: StoredValue<Target>,
    page: AuditPage,
) -> impl IntoView {
    let (label, entries, next) = match page {
        AuditPage::Empty => (
            t("选择审阅分支或父边。", "Choose branches or parents to review."),
            Vec::new(),
            None,
        ),
        AuditPage::Branches { start, page } => {
            let next = page.next_after.map(|after| Read::Branches {
                after: Some(after),
                start: start + page.entries.len() as u64,
            });
            let rows = page
                .entries
                .iter()
                .map(|entry| {
                    format!("{} → {} → {}", entry.branch_id, entry.node_id, page.deletion_node_id)
                })
                .collect();
            let label = format!(
                "{} · {}-{}",
                t("分支 → 保留末端 → D", "Branch → retained tip → D"),
                start + 1,
                start + page.entries.len() as u64,
            );
            (label, rows, next)
        }
        AuditPage::Parents(page) => {
            let rows = page
                .entries
                .iter()
                .map(|node| format!("{} → {}", node, page.deletion_node_id))
                .collect();
            let next = page.next_offset.map(|offset| Read::Parents { offset });
            let label = format!(
                "{} · {}-{}",
                t("完整去重父边", "Complete distinct parents"),
                page.offset + 1,
                page.offset + page.entries.len() as u64,
            );
            (label, rows, next)
        }
    };
    view! {
        <section class="zxs-stack" data-deletion-audit-page="">
            <h4>{label}</h4>
            <ul>{entries.into_iter().map(|entry|view! {
                <li><code>{entry}</code></li>
            }).collect_view()}</ul>
            {next.map(|read|read_button(state,target,read,t("下一页", "Next page")))}
        </section>
    }
}

fn read_button(
    state: RwSignal<State>,
    target: StoredValue<Target>,
    read: Read,
    label: String,
) -> impl IntoView {
    view! {
        <button
            class="zxs-button"
            disabled=move ||!state.with(|s|target.with_value(|target|target.current(s)))
            on:click=move |_|read::page(state,target.get_value(),read)
        >
            {label}
        </button>
    }
}

fn confirmation(state: RwSignal<State>, review: restore::Review) -> impl IntoView {
    let epoch = review.target.epoch;
    let generation = review.generation;
    let summary = review.target.summary;
    view! {
        <section class="zxs-surface zxs-stack" data-zxs-summary-restore-review="">
            <h3>{t("确认恢复准确文件 D", "Confirm restoring the exact file D")}</h3>
            <p><code>{summary.path.as_str().to_owned()}</code></p>
            <p>
                <code>{summary.file_id.to_string()}</code>" · "
                <code>{summary.node_id.to_string()}</code>
            </p>
            <p>{t(
                "已重新核对同一 D 的摘要。恢复涵盖完整 D 的全部分支，与当前审阅哪一页无关。服务端重新核对权限、原路径和目录删除约束。",
                concat!(
                    "The same D summary was checked again. Restoration covers every branch in the ",
                    "complete D, regardless of the displayed page. ",
                    "The server rechecks permission, original paths ",
                    "and directory-deletion constraints.",
                ),
            )}</p>
            <div class="zxs-row">
                <button
                    class="zxs-button"
                    data-zxs-confirm-summary-restore=""
                    disabled=move ||state.with(|s|restore::reviewed(s,epoch,generation).is_none())
                    on:click=move |_|restore::confirm(state,epoch,generation)
                >
                    {t("确认恢复这个文件", "Confirm restoring this file")}
                </button>
                <button
                    class="zxs-button"
                    on:click=move |_|restore::cancel(state,epoch,generation)
                >
                    {t("取消，不发送请求", "Cancel without sending")}
                </button>
            </div>
            <p>{t(
                "发送前保留小型原请求。未知结果只查询同一操作，关闭或刷新仍保留恢复依据。",
                concat!(
                    "A compact original request is retained before sending. ",
                    "Unknown outcomes only query that operation; ",
                    "closing or refreshing preserves recovery evidence.",
                ),
            )}</p>
        </section>
    }
}
