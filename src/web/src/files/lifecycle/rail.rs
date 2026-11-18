//! Event identities are separate from their related content IDs, even if several
//! members came from one operation. Keep the rail and event anchors mounted on refresh.
use super::{navigation::{Controls, Origin, Proof}, *};
use storage_contract::file_lifecycle::{LinkRole, References};

fn role_label(role: &LinkRole) -> String {
    match role {
        LinkRole::Before => t("操作前", "Before"),
        LinkRole::After => t("操作后", "After"),
        LinkRole::BranchHead { branch } => format!("{} {branch}", t("冻结的分支头", "Frozen branch head")),
        LinkRole::MergeInput { branch } => format!("{} {branch}", t("合并输入分支", "Merge input branch")),
        LinkRole::Chosen => t("选用的正文", "Chosen content"),
        LinkRole::Result => t("结果正文", "Result content"),
    }
}

fn references(c: Controls, origin: &Origin, event: &Event) -> AnyView {
    match &event.references {
        References::Open {} => view! { <p class="zxs-row">{t(
            "缺少完整的原始对应，不提供内容跳转。", "Original relations are incomplete; no content link is available."
        )}</p> }.into_any(),
        References::Complete { links } if links.is_empty() => view! { <p class="zxs-row">{t(
            "原记录未列出关联内容。", "The original record lists no related content."
        )}</p> }.into_any(),
        References::Complete { links } => {
            let items = links.iter().map(|link| {
                let role = role_label(&link.role);
                let body = match link.node {
                    Some(node) => c.button(origin.clone(), Proof::Event {
                        event: event.event_key(), role: link.role.clone(), node,
                    }, format!("{role}: {node}")).into_any(),
                    None => view! { <span>{format!("{role}: {}", t(
                        "本文件内没有可定位的原节点", "No verified node to locate in this file"
                    ))}</span> }.into_any(),
                };
                view! { <li>{body}</li> }
            }).collect_view();
            view! { <ul class="zxs-lifecycle-relations" aria-label=t("原内容关系", "Original content relations")>{items}</ul> }.into_any()
        }
    }
}

fn record(c: Controls, origin: &Origin, event: &Event) -> AnyView {
    let detail = detail_view(&event.detail);
    let refs = references(c, origin, event);
    view! {
        <p class="zxs-row" role="heading" {web_interface::attr::custom::custom_attribute("aria-level", "4")}>
            {action_label(&event.detail)}
        </p>
        <p class="zxs-row">{event_time(event)}</p>
        <p class="zxs-row">{format!("{}: {}", t("操作主体", "Recorded actor"), event.principal)}</p>
        <p class="zxs-row">{format!("{}: {}", t("操作", "Operation"), event.operation_id)}</p>
        {detail}
        <p class="zxs-row">{format!("{}: {}", t("保留记录序号", "Retained record sequence"), event.sequence)}</p>
        {refs}
    }.into_any()
}

type Row = (OperationId, SourceId);

fn event_view(c: Controls, origin: Memo<Option<Origin>>, key: Row) -> impl IntoView {
    let source = key.1.to_string();
    let operation = key.0.to_string();
    let lookup = move |origin: &Origin|
        origin.page.events.iter().position(|e| event_identity(e) == key);
    let anchor = Memo::new(move |_| origin.with(|origin| {
        let origin = origin.as_ref()?;
        Some(event_anchor(&origin.page.file_id, &origin.page.events[lookup(origin)?]))
    }));
    let marker = Memo::new(move |_| origin.with(|origin| {
        let origin = origin.as_ref()?;
        let e = &origin.page.events[lookup(origin)?];
        Some(format!("{} · {}", action_label(&e.detail), e.sequence))
    }));
    let record = move || origin.with(|origin| {
        let origin = origin.as_ref()?;
        Some(record(c, origin, &origin.page.events[lookup(origin)?]))
    });
    let segment = super::interval::below(c, origin, key);
    view! {
        <div class="zxs-lifecycle-event" role="listitem">
            <a class="zxs-lifecycle-marker" href=move ||anchor.get().as_deref().map(event_link)>
                {move || marker.get()}
            </a>
            <article class="zxs-lifecycle-record zxs-content-panel zxs-frame zxs-stack zxs-stack--flush"
                id=move || anchor.get() tabindex="-1" data-zxs-lifecycle-operation=operation data-zxs-lifecycle-source=source>
                {record}
            </article>
            {segment}
        </div>
    }
}

pub(super) fn page_view(c: Controls, origin: Memo<Option<Origin>>) -> impl IntoView {
    let rows = Memo::new(move |_| origin.with(|origin| origin.as_ref().map(|o|
        o.page.events.iter().rev().map(event_identity).collect::<Vec<_>>()).unwrap_or_default()));
    view! {
        <Show when=move ||origin.with(Option::is_some)>
            <div class="zxs-stack">
                <p class="zxs-row" role="status">{move ||origin.with(|o|o.as_ref().map(|o|page_status(&o.page)))}</p>
                <p class="zxs-row">{move ||origin.with(|o|o.as_ref().map(|o|format!("{}: {}",
                    t("观察状态序号", "Observed state sequence"), o.page.observation_sequence)))}</p>
                <p class="zxs-row">{t("只对本页两个原始事件之间经证实的完整区段排布内容；页外边界保持开放。",
                    "Only a verified interval between two original events on this page is aligned. Page boundaries stay open.")}</p>
                <div class="zxs-lifecycle-rail" role="list">
                    <For each=move ||rows.get() key=|key|*key children=move |key|event_view(c, origin, key)/>
                </div>
            </div>
        </Show>
    }
}
