use super::{Model, Target, load};
use crate::ui::State;
use storage_contract::automatic_observation::Reply;
use web_interface::{i18n::t, prelude::*};

fn results(reply: &Reply, files: u64, nodes: u64, carried: u64) -> AnyView {
    let page = &reply.page;
    let complete = page.next.is_none();
    let status = if complete {
        t("已完整读取此时间窗", "Complete for this window")
    } else {
        t(
            "仅统计已读页面，不是整个时间窗的总数",
            "Pages read so far, not the full-window total",
        )
    };
    let observation = page.observed;
    let mut carry = carried;
    let groups = page
        .rows
        .chunk_by(|a, b| a.key.file == b.key.file)
        .map(|group| {
            let file = group[0].key.file;
            let count =
                u64::try_from(group.len()).expect("invariant: a page has at most 64 rows") + carry;
            carry = 0;
            let label = format!(
                "{}: {file} · {}: {count}",
                t("文件 ID", "File ID"),
                t("已读独立节点", "Distinct nodes read")
            );
            let members = group
                .iter()
                .map(|row| {
                    let text = format!(
                        "{} · {} {} · {} {}",
                        row.key.node,
                        t("原操作", "Original operation"),
                        row.operation,
                        t("发表时间（UTC Unix 秒）", "Published at (UTC Unix seconds)"),
                        row.published_at
                    );
                    view! {<li><code>{text}</code></li>}
                })
                .collect_view();
            view! {<li><details><summary>{label}</summary><ul>{members}</ul></details></li>}
        })
        .collect_view();
    view!{
        <div class="zxs-stack zxs-stack--flush" data-zxs-automatic-publication-page="">
            <p role="status" aria-live="polite">{format!("{status} · {}: {files} · {}: {nodes} · {}: {observation}",
                t("稳定文件","Stable files"),t("独立自动发表节点","Distinct automatically published nodes"),t("观察序号","Observation sequence"))}</p>
            <ul aria-label=t("本页文件与节点","Files and nodes on this page")>{groups}</ul>
        </div>
    }.into_any()
}
pub(in crate::files) fn panel(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| model.update(|m| m.target(target.get())));
    on_cleanup(move || {
        let _ = model.try_update(Model::clear);
    });
    let active = move || target.get().is_some() && model.with(|m| m.target == target.get());
    let busy = move || model.with(|m| m.read.is_some());
    view! {
        <Show when=active>
            <details class="zxs-entry-section zxs-stack" data-zxs-automatic-observation="">
                <summary>{t("此目录的自动发表记录","Automatic publications in this directory")}</summary>
                <p>{t("按发表时的目录归属查询。只数已发表的自动文件节点，不数普通保存、手动节点或备份。时间为 UTC，包含开始、不包含结束。",
                    "Uses the directory at publication. Counts only published automatic file nodes, not draft saves, manual nodes or backups. Times are UTC, start inclusive and end exclusive.")}</p>
                <div class="zxs-flow">
                    <label>{t("开始（UTC）","Start (UTC)")}<input type="datetime-local" step="1"
                        prop:value=move ||model.with(|m|m.start.clone()) on:input=move |ev|model.update(|m|{m.clear();m.start=event_target_value(&ev);})/></label>
                    <label>{t("结束（UTC）","End (UTC)")}<input type="datetime-local" step="1"
                        prop:value=move ||model.with(|m|m.end.clone()) on:input=move |ev|model.update(|m|{m.clear();m.end=event_target_value(&ev);})/></label>
                    <button class="zxs-button" disabled=busy on:click=move |_|load(state,model,false)>{t("查询时间窗","Read window")}</button>
                    <button class="zxs-button" disabled=move ||busy() || model.with(|m|m.page.as_ref().is_none_or(|r|r.page.next.is_none()))
                        on:click=move |_|load(state,model,true)>{t("读取下一页","Read next page")}</button>
                    <button class="zxs-button" disabled=move ||model.with(|m|m.read.is_none()&&m.page.is_none()&&m.error.is_none())
                        on:click=move |_|model.update(Model::clear)>{t("清除查询结果","Clear observation")}</button>
                </div>
                <Show when=busy><p role="status">{t("正在读取原发表记录…","Reading original publication decisions…")}</p></Show>
                {move ||model.with(|m|if m.target==target.get(){m.page.as_ref().map(|r|results(r,m.totals.files,m.totals.nodes,m.carried))}else{None})}
                {move ||model.with(|m|m.error.clone()).map(|e|view!{<p role="alert">{e.to_string()}</p>})}
            </details>
        </Show>
    }
}
