//! Only a complete, adjacent pair on the same admitted page can supply a DAG.
//! The rank of a shared child follows its longest real parent chain, not its
//! sequence, current branch or apparent distance from a visible tip.
use super::{navigation::{Controls, Origin, Proof}, *};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    BranchId, FileNodeId,
    file_lifecycle::{BranchTip, EventKey, IntervalBody, IntervalNode},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Graph {
    // Newer rows first for display; rank one is adjacent to the older boundary.
    rows: Vec<Vec<FileNodeId>>,
    edges: Vec<(FileNodeId, FileNodeId)>,
    outside: BTreeMap<FileNodeId, Vec<FileNodeId>>,
    tips: BTreeMap<FileNodeId, Vec<BranchId>>,
}

fn ranked(nodes: &[IntervalNode], branches: &[BranchTip]) -> Option<Graph> {
    let ids: BTreeSet<_> = nodes.iter().map(|r| r.node).collect();
    if ids.len() != nodes.len() { return None; }
    let mut remaining = BTreeMap::new();
    let mut children = BTreeMap::<_, Vec<_>>::new();
    let mut outside = BTreeMap::<_, Vec<_>>::new();
    let mut edges = Vec::new();
    for row in nodes {
        let mut seen = BTreeSet::new();
        let mut n = 0usize;
        for &p in &row.parents {
            if p == row.node || !seen.insert(p) { return None; }
            if ids.contains(&p) {
                children.entry(p).or_default().push(row.node);
                edges.push((p, row.node));
                n += 1;
            } else {
                outside.entry(row.node).or_default().push(p);
            }
        }
        remaining.insert(row.node, n);
    }
    let mut ready: BTreeSet<_> = remaining.iter().filter_map(|(&id,&n)| (n == 0).then_some(id)).collect();
    let mut ranks: BTreeMap<FileNodeId, usize> = BTreeMap::new();
    let mut rows = Vec::<Vec<FileNodeId>>::new();
    let mut count = 0usize;
    while let Some(id) = ready.pop_first() {
        let rank = ranks.get(&id).copied().unwrap_or(0);
        if rows.len() <= rank { rows.resize_with(rank + 1, Vec::new); }
        rows[rank].push(id);
        count += 1;
        for &child in children.get(&id).into_iter().flatten() {
            let r = ranks.entry(child).or_default();
            *r = (*r).max(rank + 1);
            let n = remaining.get_mut(&child)?;
            *n -= 1;
            if *n == 0 { ready.insert(child); }
        }
    }
    if count != ids.len() { return None; }
    let mut assigned = BTreeSet::new();
    let mut tips: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for b in branches {
        if !ids.contains(&b.node) || !assigned.insert(b.branch) { return None; }
        tips.entry(b.node).or_default().push(b.branch);
    }
    for row in &mut rows { row.sort_unstable(); }
    for values in outside.values_mut() { values.sort_unstable(); }
    for values in tips.values_mut() { values.sort_unstable(); }
    edges.sort_unstable();
    rows.reverse();
    Some(Graph { rows, edges, outside, tips })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Section {
    OutsidePage,
    Open,
    Complete { older: EventKey, newer: EventKey, anchor: String, graph: Graph },
}
fn below_in(page: &Page, index: usize) -> Section {
    let Some((older, newer)) = index.checked_sub(1).and_then(|prev|
        Some((page.events.get(prev)?.event_key(), page.events.get(index)?.event_key()))) else {
        return Section::OutsidePage;
    };
    let mut matches = page.intervals.iter().filter(|i| i.older == older && i.newer == newer);
    let Some(interval) = matches.next() else { return Section::Open };
    if matches.next().is_some() { return Section::Open; }
    let IntervalBody::Complete { nodes, branches } = &interval.body else { return Section::Open };
    let Some(graph) = ranked(nodes, branches) else { return Section::Open };
    Section::Complete { older, newer, anchor: event_anchor(&page.file_id, &page.events[index]), graph }
}

pub(super) fn below(c: Controls, origin: Memo<Option<Origin>>, key: (OperationId, SourceId)) -> impl IntoView {
    // No scroll/focus reset on a refresh that changes an unrelated operation or
    // principal. Rebuild a graph only when its own admitted boundaries/DAG change.
    let section = Memo::new(move |_| origin.with(|o| {
        let page = &o.as_ref()?.page;
        Some(below_in(page, page.events.iter().position(|e| event_identity(e) == key)?))
    }));
    move || section.get().map(|s| match s {
        Section::OutsidePage => view! { <p class="zxs-row zxs-interval-open" data-interval="open">
            {t("开放区段：本页没有较旧事件边界。", "Open interval: its older event boundary is not on this page.")}
        </p> }.into_any(),
        Section::Open => view! { <p class="zxs-row zxs-interval-open" data-interval="open">
            {t("开放区段：无法证明全部内容、父关系或边界，不绘制完整区段。",
               "Open interval: complete membership, ancestry or boundaries are not proven. No aligned graph is drawn.")}
        </p> }.into_any(),
        Section::Complete { older, newer, anchor, graph } => complete(c, origin, older, newer, anchor, graph),
    })
}

fn complete(c: Controls, origin: Memo<Option<Origin>>, older: EventKey, newer: EventKey, anchor: String, graph: Graph) -> AnyView {
    let ids: BTreeSet<_> = graph.rows.iter().flatten().copied().collect();
    if graph.rows.is_empty() {
        return view! { <div class="zxs-stack" data-interval="complete-empty">
            <p class="zxs-row">{t("已验证：两个事件之间没有新内容节点。", "Verified: there are no new content nodes between these events.")}</p>
        </div> }.into_any();
    }
    let levels = graph.rows.iter().map(|r| r.iter().map(ToString::to_string).collect()).collect();
    let edges = graph.edges.iter().map(|(p,n)|(p.to_string(),n.to_string())).collect();
    let tips = graph.tips.keys().filter(|id|ids.contains(*id)).map(ToString::to_string).collect();
    let lookup = StoredValue::new(ids.into_iter().map(|id|(id.to_string(),id)).collect::<BTreeMap<_,_>>());
    let graph = StoredValue::new(graph);
    let boundaries = StoredValue::new((older, newer));
    let draw = crate::files::version_tree::interval::graph(levels, edges, tips, format!("{anchor}-ancestry"), move |id| {
        let node = lookup.with_value(|ids|ids[&id]);
        let (older,newer) = boundaries.get_value();
        let button = c.section_button(origin, Proof::Section { older, newer, node });
        let external = graph.with_value(|g|g.outside.get(&node).cloned().unwrap_or_default());
        let endpoints = graph.with_value(|g|g.tips.get(&node).cloned().unwrap_or_default());
        view! { <article class="zxs-version-node zxs-interval-node" data-version-id=id>
            {button}
            {external.into_iter().map(|p|view! { <p>{format!("{}: {p}", t("经验证的区段外父节点", "Verified parent outside this interval"))}</p> }).collect_view()}
            {endpoints.into_iter().map(|b|view! { <p>{format!("{}: {b}", t("已验证的分支端点", "Verified branch endpoint"))}</p> }).collect_view()}
        </article>}
    });
    view! { <section class="zxs-stack zxs-interval" data-interval="complete">
        <p class="zxs-row">{t("实线箭头表示内容父子关系。虚线无箭头，仅把已验证的分支端点连到较新的操作边界。",
            "Solid arrows show content ancestry. Dashed, arrow-free guides connect verified branch endpoints to the newer event boundary.")}</p>
        <a class="zxs-interval-boundary" href=event_link(&anchor)>{t("较新的操作边界", "Newer event boundary")}</a>
        {draw}
    </section> }.into_any()
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/lifecycle_interval.rs"]
mod tests;
