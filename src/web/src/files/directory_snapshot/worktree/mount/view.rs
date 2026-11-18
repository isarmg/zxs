//! Nothing in this panel initiates a write when it renders or receives a paste.
use super::*;
use crate::files::directory_snapshot::view::original_text;
use web_interface::i18n::t;

fn input(s: &State, field: Field) -> String {
    form(s).map(|f| match field {
        Field::Operation => &f.operation,
        Field::Lookup => &f.lookup,
        Field::Hidden => &f.hidden,
        Field::Name => &f.name,
    }).cloned().unwrap_or_default()
}

fn hide_panel(s: RwSignal<State>) -> impl IntoView {
    let status = move || s.with(|s| {
        hide_facts(s).map(|(p, current)| format!(
            "{} · {}: {} · {}: {} · {}: {}",
            current.name.as_str(), t("提交操作", "Commit"), p.operation,
            t("目录节点", "Node"), p.committed.node_id, t("本次挂载", "Occurrence"), current.visible,
        ))
    });
    let cannot_read = move || s.with(|s| {
        s.files.directory_snapshot.worktree.mount.reading || Reader::capture(s).is_none()
    });
    view! {
        <p>{t(
            "隐藏只撤下挂载名，不删除正文或历史。服务器只接受已经整批提交、此后没有新工作或待定操作的原实例。",
            concat!("Hiding removes the mount name, not content or history. The server admits only ",
                "the original instance with a completed batch commit and no later work or pending operation."),
        )}</p>
        <label>
            {t("已打开实例的原目录提交操作号（若尚未确认）", "Original commit operation for the open instance (if not yet confirmed)")}
            <input
                class="zxs-input"
                data-zxs-mount-commit-operation=""
                maxlength="64"
                prop:value=move || s.with(|s| input(s, Field::Operation))
                on:input=move |e| s.update(|s| edit(s, Field::Operation, event_target_value(&e)))
            />
        </label>
        <button
            class="zxs-button"
            disabled=cannot_read
            on:click=move |_| Kind::Commit.start(s)
        >
            {t("核对原目录提交", "Check original commit")}
        </button>
        <p role="status">{status}</p>
        <button
            class="zxs-button"
            data-zxs-mount-hide=""
            disabled=move || s.with(|s| hide_facts(s).is_none())
            on:click=move |_| super::super::requests::submit(s, hide)
        >
            {t("隐藏此已提交实例的当前挂载", "Hide this committed instance’s current mount")}
        </button>
    }
}

fn lookup_panel(s: RwSignal<State>) -> impl IntoView {
    let cannot_read = move || s.with(|s| {
        s.files.directory_snapshot.worktree.mount.reading || Query::capture(s).is_none()
    });
    let result = move || s.with(|s| {
        original::observed(s).map(|r| original_text(r, t(
            "服务器返回的原请求与结果（可复制）",
            "Original request and outcome returned by the server (copyable)",
        )))
    });
    view! {
        <label>
            {t("我在此仓库的原挂载操作号", "My original mount-change operation in this repository")}
            <input
                class="zxs-input"
                data-zxs-mount-lookup-id=""
                maxlength="64"
                prop:value=move || s.with(|s| input(s, Field::Lookup))
                on:input=move |e| s.update(|s| edit(s, Field::Lookup, event_target_value(&e)))
            />
        </label>
        <button
            class="zxs-button"
            data-zxs-mount-lookup=""
            disabled=cannot_read
            on:click=move |_| Kind::Mount.start(s)
        >
            {t("只查询原挂载操作，不重发", "Read original mount outcome without resending")}
        </button>
        {result}
    }
}

fn restore_status(s: &State) -> Option<String> {
    if let Some((_, r, _)) = restore_facts(s) {
        Some(format!("{}: {} · {}: {}",
            t("实例", "Instance"), r.target.worktree_id, t("原隐藏操作", "Original hide"), r.deletion))
    } else if form(s).is_some_and(|f| !f.hidden.is_empty() || f.invalid_name) {
        Some(t("本根的准确隐藏回执或名称无效。",
            "The exact Hidden receipt for this root or the requested name is not valid."))
    } else { None }
}

fn restore_panel(s: RwSignal<State>) -> impl IntoView {
    view! {
        <p>{t(
            "管理员恢复：粘贴已知的完整“已隐藏”原回执。不提供隐藏实例列表。恢复不创建新实例；服务器重新验证当前管理员和原隐藏操作。",
            concat!("Administrator restore: paste the known, complete Hidden receipt. There is no hidden-instance ",
                "listing. Restore reuses the same instance; the server checks the current administrator and original hide."),
        )}</p>
        <label>
            {t("原隐藏回执（JSON）", "Original Hidden receipt (JSON)")}
            <textarea
                class="zxs-input"
                data-zxs-mount-restore-receipt=""
                maxlength="8192"
                prop:value=move || s.with(|s| input(s, Field::Hidden))
                on:input=move |e| s.update(|s| edit(s, Field::Hidden, event_target_value(&e)))
            ></textarea>
        </label>
        <label>
            {t("恢复名称：留空优先原名，冲突自动分配；填写则必须使用该名称。",
                "Restore name: leave blank to prefer the old name or allocate on collision; enter a name to require it.")}
            <input
                class="zxs-input"
                prop:value=move || s.with(|s| input(s, Field::Name))
                on:input=move |e| s.update(|s| edit(s, Field::Name, event_target_value(&e)))
            />
        </label>
        <p role="status">{move || s.with(restore_status)}</p>
        <button
            class="zxs-button"
            data-zxs-mount-restore=""
            disabled=move || s.with(|s| restore_facts(s).is_none())
            on:click=move |_| super::super::requests::submit(s, restore)
        >
            {t("以当前管理员身份恢复原实例", "Restore the original instance as the current administrator")}
        </button>
    }
}

pub(in crate::files::directory_snapshot::worktree) fn view(s: RwSignal<State>) -> impl IntoView {
    view! {
        <details class="zxs-stack zxs-content-panel" data-zxs-clean-mount="">
            <summary>{t("隐藏或恢复一个 worktree 挂载", "Hide or restore a worktree mount")}</summary>
            {hide_panel(s)}
            {lookup_panel(s)}
            {restore_panel(s)}
        </details>
    }
}
