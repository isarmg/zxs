//! File history ancestry, including body and path nodes, laid out once per identity/edge change. Notes never
//! participate in layout keys, so refreshing metadata preserves active editors.
use super::{
    actions, branch,
    history::{FileNode, NodeKind, ParentEndpoint, ParentSet},
    history_index::HistoryIndex,
    inline,
    model::Model,
};
use crate::ui::State;
use storage_contract::history_window::HistoryField;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure};
use web_interface::{html, i18n::t, prelude::*, svg};

mod observers;
mod refresh;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    id: String,
    sequence: u64,
    parents: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Layout {
    levels: Vec<Vec<String>>,
    edges: Vec<(String, String)>,
}

fn metadata(records: &[FileNode]) -> Vec<Version> {
    let mut versions = records
        .iter()
        .map(|record| {
            let mut parents = record
                .parents()
                .iter()
                .filter(|parent| parent.endpoint() == ParentEndpoint::InWindow)
                .map(|parent| parent.id().to_owned())
                .collect::<Vec<_>>();
            parents.sort();
            parents.dedup();
            Version {
                id: record.id().to_owned(),
                sequence: record.sequence(),
                parents,
            }
        })
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    versions
}

fn newer_first(a: &Version, b: &Version) -> std::cmp::Ordering {
    b.sequence
        .cmp(&a.sequence)
        .then_with(|| a.id.cmp(&b.id))
}

/// This marker belongs to the selected branch projection, not the immutable
/// parent version. Its tree node remains one real historical node.
fn inherited_current(model: &Model, version: &str) -> bool {
    (model.file_history.current_inherited()
        && model.file_history.current_node_id() == Some(version))
        || model
            .selected
            .as_ref()
            .is_some_and(|entry| entry.inherited_content() && entry.node_id() == Some(version))
}

/// Iterative Kosaraju traversal: each adjacency is visited once in each
/// direction, without using the browser call stack for long histories.
fn components(children: &[Vec<usize>], parents: &[Vec<usize>]) -> Vec<usize> {
    let mut seen = vec![false; children.len()];
    let mut finished = Vec::with_capacity(children.len());
    let mut pending = Vec::new();
    for start in 0..children.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        pending.push((start, 0));
        while let Some((node, next)) = pending.last_mut() {
            if let Some(&child) = children[*node].get(*next) {
                *next += 1;
                #[cfg(test)]
                LAYOUT_WORK.with(|work| work.borrow_mut().component_edges += 1);
                if !seen[child] {
                    seen[child] = true;
                    pending.push((child, 0));
                }
            } else {
                finished.push(*node);
                pending.pop();
                #[cfg(test)]
                LAYOUT_WORK.with(|work| work.borrow_mut().component_nodes += 1);
            }
        }
    }
    let mut component = vec![usize::MAX; children.len()];
    let mut pending = Vec::new();
    for start in finished.into_iter().rev() {
        if component[start] != usize::MAX {
            continue;
        }
        component[start] = start;
        pending.push(start);
        while let Some(node) = pending.pop() {
            #[cfg(test)]
            LAYOUT_WORK.with(|work| work.borrow_mut().component_nodes += 1);
            for &parent in &parents[node] {
                #[cfg(test)]
                LAYOUT_WORK.with(|work| work.borrow_mut().component_edges += 1);
                if component[parent] == usize::MAX {
                    component[parent] = start;
                    pending.push(parent);
                }
            }
        }
    }
    component
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
struct LayoutWork {
    component_nodes: usize,
    component_edges: usize,
    candidate_edges: usize,
    topology_edges: usize,
    center_edges: usize,
    ready_pushes: usize,
    ready_pops: usize,
}

#[cfg(test)]
thread_local! {
    static LAYOUT_WORK: std::cell::RefCell<LayoutWork> = const {
        std::cell::RefCell::new(LayoutWork {
            component_nodes: 0, component_edges: 0, candidate_edges: 0,
            topology_edges: 0, center_edges: 0, ready_pushes: 0, ready_pops: 0,
        })
    };
}

/// Only declared, visible parents produce edges. Cyclic edges are isolated
/// rather than inventing chronology or making the traversal recurse forever.
fn layout(versions: &[Version]) -> Layout {
    let mut nodes = BTreeMap::<String, Version>::new();
    // Stable conflict handling also makes duplicate identities harmless.
    let mut sorted = versions.to_vec();
    sorted.sort();
    for version in sorted {
        nodes.entry(version.id.clone()).or_insert(version);
    }
    let nodes = nodes.into_values().collect::<Vec<_>>();
    let indexes = nodes
        .iter()
        .enumerate()
        .map(|(index, version)| (version.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut children = vec![Vec::new(); nodes.len()];
    for (child, version) in nodes.iter().enumerate() {
        for parent in &version.parents {
            if let Some(&parent) = indexes.get(parent.as_str())
                && parent != child
                && children[parent].last() != Some(&child)
            {
                // Node iteration is in identity order, preserving the original
                // BTreeSet child order while deduplicating declared parents.
                children[parent].push(child);
            }
        }
    }
    let mut parents = vec![Vec::new(); nodes.len()];
    for (parent, descendants) in children.iter().enumerate() {
        for &child in descendants {
            parents[child].push(parent);
        }
    }
    let component = components(&children, &parents);
    let mut edges = Vec::new();
    let mut accepted_children = vec![Vec::new(); nodes.len()];
    let mut accepted_parents = vec![Vec::new(); nodes.len()];
    for (parent, descendants) in children.iter().enumerate() {
        for &child in descendants {
            #[cfg(test)]
            LAYOUT_WORK.with(|work| work.borrow_mut().candidate_edges += 1);
            // With parent -> child already declared, child reaches parent iff
            // both are in the same SCC. Remove every cyclic edge, not merely
            // DFS back edges. Redundant acyclic ancestry edges remain visible.
            if component[parent] != component[child] {
                edges.push((nodes[parent].id.clone(), nodes[child].id.clone()));
                accepted_children[parent].push(child);
                accepted_parents[child].push(parent);
            }
        }
    }
    // Walk from loaded-window tips towards loaded ancestors. This is a reverse
    // topological layout, not a visual reversal of the old root-first layers:
    // both short and long visible paths end on the first layer. Window bounds
    // do not establish the tips or roots of the complete history.
    let mut remaining_children = accepted_children.iter().map(Vec::len).collect::<Vec<_>>();
    let mut by_priority = (0..nodes.len()).collect::<Vec<_>>();
    by_priority.sort_by(|&a, &b| newer_first(&nodes[a], &nodes[b]));
    let mut priority = vec![0; nodes.len()];
    for (rank, &node) in by_priority.iter().enumerate() {
        priority[node] = rank;
    }
    let mut ready = remaining_children
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(node, _)| priority[node])
        .collect::<BTreeSet<_>>();
    #[cfg(test)]
    LAYOUT_WORK.with(|work| work.borrow_mut().ready_pushes += ready.len());
    let mut depths = vec![0; nodes.len()];
    let mut levels = Vec::<Vec<usize>>::new();
    while let Some(rank) = ready.pop_first() {
        let node = by_priority[rank];
        #[cfg(test)]
        LAYOUT_WORK.with(|work| work.borrow_mut().ready_pops += 1);
        let depth = depths[node];
        if levels.len() <= depth {
            levels.resize(depth + 1, Vec::new());
        }
        levels[depth].push(node);
        for &parent in &accepted_parents[node] {
            #[cfg(test)]
            LAYOUT_WORK.with(|work| work.borrow_mut().topology_edges += 1);
            depths[parent] = depths[parent].max(depth + 1);
            remaining_children[parent] -= 1;
            if remaining_children[parent] == 0 {
                ready.insert(priority[parent]);
                #[cfg(test)]
                LAYOUT_WORK.with(|work| work.borrow_mut().ready_pushes += 1);
            }
        }
    }
    if let Some(tips) = levels.first_mut() {
        tips.sort_unstable_by_key(|&node| priority[node]);
    }
    let mut positions = vec![0; nodes.len()];
    let mut centers = vec![(usize::MAX as u128, 1u128, usize::MAX); nodes.len()];
    for depth in 1..levels.len() {
        for (index, &node) in levels[depth - 1].iter().enumerate() {
            positions[node] = index;
        }
        // Group ancestors by the positions of their newer children on the
        // preceding display layer; all cross-layer source edges remain intact.
        for &node in &levels[depth] {
            let (mut sum, mut count, mut minimum) = (0u128, 0u128, usize::MAX);
            for &child in &accepted_children[node] {
                #[cfg(test)]
                LAYOUT_WORK.with(|work| work.borrow_mut().center_edges += 1);
                if depths[child] == depth - 1 {
                    let position = positions[child];
                    sum += position as u128;
                    count += 1;
                    minimum = minimum.min(position);
                }
            }
            if count != 0 {
                centers[node] = (sum, count, minimum);
            }
        }
        levels[depth].sort_by(|&a, &b| {
            let (a_sum, a_count, a_min) = centers[a];
            let (b_sum, b_count, b_min) = centers[b];
            (a_sum * b_count)
                .cmp(&(b_sum * a_count))
                .then_with(|| a_min.cmp(&b_min))
                .then_with(|| priority[a].cmp(&priority[b]))
        });
    }
    let levels = levels
        .into_iter()
        .map(|level| {
            level
                .into_iter()
                .map(|node| nodes[node].id.clone())
                .collect()
        })
        .collect();
    Layout { levels, edges }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
    paths: BTreeMap<(String, String), String>,
}
impl Geometry {
    fn path(&self, edge: &(String, String)) -> String {
        #[cfg(test)]
        GEOMETRY_WORK.with(|work| work.borrow_mut().path_lookups += 1);
        self.paths.get(edge).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug)]
struct NodeBounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    depth: usize,
}
impl NodeBounds {
    fn center(self) -> f64 {
        (self.left + self.right) / 2.
    }
}

#[derive(Clone, Copy, Debug)]
struct LayerBounds {
    top: f64,
    bottom: f64,
}

/// Bounds belong to one DOM measurement, not a cache across resizes or edits.
/// Preserve the old per-edge extrema, but inspect each node only once.
struct RoutingBounds {
    layers: BTreeMap<usize, LayerBounds>,
    left: f64,
    right: f64,
    all_finite: bool,
}
impl RoutingBounds {
    fn new(boxes: impl IntoIterator<Item = NodeBounds>) -> Self {
        let mut bounds = Self {
            layers: BTreeMap::new(),
            left: f64::INFINITY,
            right: f64::NEG_INFINITY,
            all_finite: true,
        };
        for node in boxes {
            #[cfg(test)]
            GEOMETRY_WORK.with(|work| work.borrow_mut().bounds_visits += 1);
            bounds.all_finite &= [node.left, node.top, node.right, node.bottom]
                .iter()
                .all(|value| value.is_finite());
            bounds.left = bounds.left.min(node.left);
            bounds.right = bounds.right.max(node.right);
            bounds
                .layers
                .entry(node.depth)
                .and_modify(|layer| {
                    layer.top = layer.top.min(node.top);
                    layer.bottom = layer.bottom.max(node.bottom);
                })
                .or_insert(LayerBounds {
                    top: node.top,
                    bottom: node.bottom,
                });
        }
        bounds
    }
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
struct GeometryWork {
    bounds_visits: usize,
    edge_visits: usize,
    path_lookups: usize,
}

#[cfg(test)]
thread_local! {
    static GEOMETRY_WORK: std::cell::RefCell<GeometryWork> = const {
        std::cell::RefCell::new(GeometryWork { bounds_visits: 0, edge_visits: 0, path_lookups: 0 })
    };
}

/// Arrows retain their old-parent → new-child meaning while travelling up.
/// The target gap follows the entire newer layer so a short target's connector
/// cannot pass through a taller sibling before arriving at its bottom edge.
fn gap_heights(parent: NodeBounds, child: NodeBounds, bounds: &RoutingBounds) -> (f64, f64) {
    let exit = bounds
        .layers
        .get(&parent.depth)
        .map(|layer| parent.top.min(layer.top))
        .unwrap_or(parent.top)
        - 6.;
    let entry = bounds
        .layers
        .get(&child.depth)
        .map(|layer| child.bottom.max(layer.bottom))
        .unwrap_or(child.bottom)
        + 6.;
    (exit, entry)
}

fn long_route(parent: NodeBounds, child: NodeBounds, bounds: &RoutingBounds) -> Vec<(f64, f64)> {
    let (exit, entry) = gap_heights(parent, child, bounds);
    let sx = parent.center();
    let tx = child.center();
    let left = parent.left.min(bounds.left) - 9.;
    let right = parent.right.max(bounds.right) + 9.;
    let lane = if (sx - left).abs() + (tx - left).abs() <= (sx - right).abs() + (tx - right).abs() {
        left
    } else {
        right
    };
    vec![
        (sx, parent.top),
        (sx, exit),
        (lane, exit),
        (lane, entry),
        (tx, entry),
        (tx, child.bottom + 3.),
    ]
}

const EDGE_CORNER_RADIUS: f64 = 4.;

/// Draw only horizontal/vertical runs and circular quarter turns. Adjacent
/// corners each consume at most half their shared run, so even short routes
/// cannot double back or overlap their own rounded turns.
fn rounded_route(points: &[(f64, f64)], radius: f64) -> Option<String> {
    if !radius.is_finite() || radius <= 0. || points.is_empty() {
        return None;
    }
    let mut route = Vec::<(f64, f64)>::new();
    for &(x, y) in points {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if let Some(&(px, py)) = route.last() {
            if x == px && y == py {
                continue;
            }
            if x != px && y != py {
                return None;
            }
        }
        // Remove straight-through waypoints; keep every real corner and reject
        // reversals instead of inventing a circular U-turn through a node.
        while route.len() >= 2 {
            let a = route[route.len() - 2];
            let b = route[route.len() - 1];
            let incoming = (b.0 - a.0, b.1 - a.1);
            let outgoing = (x - b.0, y - b.1);
            if incoming.0 * outgoing.1 != incoming.1 * outgoing.0 {
                break;
            }
            if incoming.0 * outgoing.0 + incoming.1 * outgoing.1 <= 0. {
                return None;
            }
            route.pop();
        }
        route.push((x, y));
    }
    let &(x, y) = route.first()?;
    if route.len() < 2 {
        return None;
    }
    let mut path = format!("M{x:.3},{y:.3}");
    let mut cursor = (x, y);
    let line = |path: &mut String, cursor: &mut (f64, f64), point: (f64, f64)| {
        if point == *cursor {
            return;
        }
        if point.0 == cursor.0 {
            path.push_str(&format!(" V{:.3}", point.1));
        } else {
            path.push_str(&format!(" H{:.3}", point.0));
        }
        *cursor = point;
    };
    for turn in route.windows(3) {
        let [previous, corner, next] = [turn[0], turn[1], turn[2]];
        let incoming_length = (corner.0 - previous.0).abs() + (corner.1 - previous.1).abs();
        let outgoing_length = (next.0 - corner.0).abs() + (next.1 - corner.1).abs();
        let incoming = (
            (corner.0 - previous.0) / incoming_length,
            (corner.1 - previous.1) / incoming_length,
        );
        let outgoing = (
            (next.0 - corner.0) / outgoing_length,
            (next.1 - corner.1) / outgoing_length,
        );
        let radius = radius.min(incoming_length / 2.).min(outgoing_length / 2.);
        let start = (
            corner.0 - incoming.0 * radius,
            corner.1 - incoming.1 * radius,
        );
        let end = (
            corner.0 + outgoing.0 * radius,
            corner.1 + outgoing.1 * radius,
        );
        line(&mut path, &mut cursor, start);
        let clockwise = u8::from(incoming.0 * outgoing.1 - incoming.1 * outgoing.0 > 0.);
        path.push_str(&format!(
            " A{radius:.3},{radius:.3} 0 0 {clockwise} {:.3},{:.3}",
            end.0, end.1
        ));
        cursor = end;
    }
    line(&mut path, &mut cursor, *route.last()?);
    Some(path)
}

fn edge_path(parent: NodeBounds, child: NodeBounds, bounds: &RoutingBounds) -> Option<String> {
    if parent.depth <= child.depth || parent.top <= child.bottom || !bounds.all_finite {
        return None;
    }
    if parent.depth > child.depth + 1 {
        return rounded_route(&long_route(parent, child, bounds), EDGE_CORNER_RADIUS);
    }
    let (exit, entry) = gap_heights(parent, child, bounds);
    let middle = (exit + entry) / 2.;
    let sx = parent.center();
    let tx = child.center();
    let sy = parent.top;
    let ty = child.bottom + 3.;
    rounded_route(
        &[(sx, sy), (sx, middle), (tx, middle), (tx, ty)],
        EDGE_CORNER_RADIUS,
    )
}

fn edge_paths(
    layout: &Layout,
    boxes: &BTreeMap<String, NodeBounds>,
) -> BTreeMap<(String, String), String> {
    let bounds = RoutingBounds::new(boxes.values().copied());
    layout
        .edges
        .iter()
        .filter_map(|(parent, child)| {
            #[cfg(test)]
            GEOMETRY_WORK.with(|work| work.borrow_mut().edge_visits += 1);
            Some((
                (parent.clone(), child.clone()),
                edge_path(*boxes.get(parent)?, *boxes.get(child)?, &bounds)?,
            ))
        })
        .collect()
}

fn measure(root: &web_sys::Element, layout: &Layout) -> Geometry {
    let bounds = root.get_bounding_client_rect();
    let depths = layout
        .levels
        .iter()
        .enumerate()
        .flat_map(|(depth, ids)| ids.iter().map(move |id| (id.as_str(), depth)))
        .collect::<BTreeMap<_, _>>();
    let mut boxes = BTreeMap::new();
    {
        let nodes = root
            .query_selector_all(".zxs-version-node[data-version-id]")
            .expect("invariant: version-node selector is valid");
        for index in 0..nodes.length() {
            if let Some(element) = nodes
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                && let Some(id) = element.get_attribute("data-version-id")
                && let Some(&depth) = depths.get(id.as_str())
            {
                let rect = element.get_bounding_client_rect();
                boxes.insert(
                    id,
                    NodeBounds {
                        left: rect.left() - bounds.left(),
                        right: rect.right() - bounds.left(),
                        top: rect.top() - bounds.top(),
                        bottom: rect.bottom() - bounds.top(),
                        depth,
                    },
                );
            }
        }
    }
    Geometry {
        width: bounds.width().max(1.),
        height: bounds.height().max(1.),
        paths: edge_paths(layout, &boxes),
    }
}

fn observe(
    root: NodeRef<html::Div>,
    links: NodeRef<svg::Svg>,
    layout: Memo<Layout>,
    geometry: RwSignal<Geometry>,
) {
    root.on_load(move |element| {
        let element: web_sys::Element = element.unchecked_into();
        let update_root = element.clone();
        let initially_centered = std::rc::Rc::new(std::cell::Cell::new(false));
        let update =
            move || {
                let next = measure(&update_root, &layout.get_untracked());
                if geometry.with_untracked(|old| old != &next) {
                    geometry.set(next);
                }
                // A wide tree initially reveals its newest tip rather than an empty
                // left corner. This belongs to this mounted tree only: refreshes, note
                // edits and later resizes never take the user's scroll position.
                if !initially_centered.get()
                && let (Some(scroll), Some(first_tip)) = (
                    update_root.parent_element(),
                    update_root.query_selector(
                        ".zxs-version-tree-level[data-version-depth=\"0\"] .zxs-version-node",
                    ).expect("invariant: first-tip selector is valid"),
                )
            {
                let rect = first_tip.get_bounding_client_rect();
                let viewport = scroll.client_width();
                if rect.width() > 0. && rect.height() > 0. && viewport > 0 {
                    let frame = scroll.get_bounding_client_rect();
                    let center = rect.left() - frame.left()
                        + f64::from(scroll.scroll_left())
                        + rect.width() / 2.;
                    let maximum = (scroll.scroll_width() - viewport).max(0);
                    scroll.set_scroll_left(
                        (center - f64::from(viewport) / 2.)
                            .clamp(0., f64::from(maximum))
                            .round() as i32,
                    );
                    initially_centered.set(true);
                }
            }
            };
        update();
        let window = element
            .owner_document()
            .and_then(|document| document.default_view())
            .expect("invariant: mounted version tree belongs to a browser window");
        let refresh = refresh::FrameRefresh::new(window, update);
        let resize_refresh = Rc::clone(&refresh);
        let callback = Closure::wrap(Box::new(
            move |_: js_sys::Array, _: web_sys::ResizeObserver| resize_refresh.request(),
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>);
        let resize = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref())
            .expect("invariant: admitted browser constructs a ResizeObserver");
        resize.observe(&element);
        let nodes = Rc::new(observers::Nodes::new(element.clone(), resize.clone()));
        let mutation_nodes = Rc::clone(&nodes);
        let mutation_refresh = Rc::clone(&refresh);
        // A NodeRef captures our actual SVG, never an author's lookalike class.
        let links: web_sys::Element = links
            .get_untracked()
            .expect("invariant: mounted tree owns its connector SVG")
            .unchecked_into();
        let mutation_callback = Closure::wrap(Box::new(
            move |records: js_sys::Array, _: web_sys::MutationObserver| {
                if mutation_nodes.reconcile(records, &links) {
                    mutation_refresh.request();
                }
            },
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>);
        let mutation = web_sys::MutationObserver::new(mutation_callback.as_ref().unchecked_ref())
            .expect("invariant: admitted browser constructs a MutationObserver");
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        mutation
            .observe_with_options(&element, &options)
            .expect("invariant: mounted tree and child-list observation options are valid");
        let cleanup = send_wrapper::SendWrapper::new((
            resize,
            callback,
            mutation,
            mutation_callback,
            refresh,
            nodes,
        ));
        on_cleanup(move || {
            cleanup.4.close();
            cleanup.5.close();
            cleanup.0.disconnect();
            cleanup.2.disconnect();
            drop(cleanup);
        });
    });
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    // A replaced window owns a fresh graph and observer lifetime. Equal
    // structural keys keep metadata refreshes on the existing controls.
    let key = Memo::new(move |_| state.with(|s| s.files.file_history.window_key()));
    view! {
        <For each=move ||[key.get()] key=|key|*key children=move |_|window(state)/>
    }
}

fn window(state: RwSignal<State>) -> impl IntoView {
    let records = super::history_index::shared();
    let versions = Memo::new(move |_| records.with(|index| metadata(index.snapshot.nodes())));
    let tree = Memo::new(move |_| layout(&versions.get()));
    let geometry = RwSignal::new(Geometry::default());
    let root = NodeRef::<html::Div>::new();
    let links = NodeRef::<svg::Svg>::new();
    observe(root, links, tree, geometry);
    view! {
        <p class="zxs-row zxs-history-window-boundary">{t(
            "此窗口只展示已加载节点之间的关系，窗口边界不代表完整历史的起点或分支末端。",
            "This window shows relationships between loaded nodes. Its boundaries do not mark the start or tips of the complete history.",
        )}</p>
        <div class="zxs-version-tree-scroll" aria-label=t("文件版本树","File version tree")>
            <div class="zxs-version-tree-canvas" node_ref=root>
                <svg node_ref=links class="zxs-version-tree-links" aria-hidden="true" focusable="false"
                    viewBox=move ||geometry.with(|g|format!("0 0 {} {}",g.width.max(1.),g.height.max(1.))) preserveAspectRatio="none">
                    <defs><marker id="zxs-version-arrow" class="zxs-version-tree-arrow" markerWidth="8" markerHeight="8" refX="8" refY="4" orient="auto" markerUnits="userSpaceOnUse"><path d="M0,0 L8,4 L0,8 Z"/></marker></defs>
                    <For each=move ||geometry.with(|g|g.paths.keys().cloned().collect::<Vec<_>>()) key=|edge|edge.clone() children=move |(parent,child)| {
                        let key=StoredValue::new((parent.clone(),child.clone()));
                        view!{<path class="zxs-version-tree-edge" data-parent-version=parent data-child-version=child d=move ||geometry.with(|g|key.with_value(|key|g.path(key))) marker-end="url(#zxs-version-arrow)" stroke-dasharray="none" stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>}
                    }/>
                </svg>
                <For each=move ||tree.with(|tree|tree.levels.iter().enumerate().map(|(depth,ids)|(depth,ids.clone())).collect::<Vec<_>>()) key=|level|level.0 children=move |(depth,_)| {
                    view!{<div class="zxs-version-tree-level" data-version-depth=depth.to_string()>
                        <For each=move ||tree.with(|tree|tree.levels.get(depth).cloned().unwrap_or_default()) key=|id|id.clone() children=move |id|node(state,records,id)/>
                    </div>}
                }/>
            </div>
        </div>
    }
}

fn node(
    state: RwSignal<State>,
    records: Memo<HistoryIndex>,
    version_id: String,
) -> impl IntoView {
    let id = StoredValue::new(version_id);
    let version = Memo::new(move |_| {
        records.with(|records| id.with_value(|id| records.node(id).map(|(_, node)| node.clone())))
    });
    let can_modify = Signal::derive(move || {
        state.with(|s| s.files.writable && !super::model::selected_entry_read_only(&s.files))
    });
    let disabled = Signal::derive(move || {
        !can_modify.get() || !state.with(actions::history_write_allowed)
    });
    let navigation_disabled = Signal::derive(move || !state.with(actions::history_navigation_ready));
    let current = Signal::derive(move || {
        state.with(|s| s.files.file_history.current_node_id() == Some(id.get_value().as_str()))
    });
    let inherited =
        Signal::derive(move || state.with(|s| inherited_current(&s.files, &id.get_value())));
    let note_target = version.with_untracked(|node| {
        let node = node
            .as_ref()
            .expect("invariant: rendered node belongs to the installed window");
        match node.kind() {
            NodeKind::Rename | NodeKind::Tags | NodeKind::Merge => {
                inline::Target::NodeNote(id.get_value())
            }
            NodeKind::Content => inline::Target::VersionNote(
                node.version_id()
                    .expect("invariant: visible content node references a body")
                    .to_owned(),
            ),
        }
    });
    let boundary = Memo::new(move |_| {
        version.with(|node| node.as_ref().map(parent_boundary).unwrap_or_default())
    });
    let (note, edit) = inline::version_note(state, note_target, id.get_value(), disabled);
    view! {
        <article class="zxs-content-panel zxs-version-node zxs-frame zxs-stack zxs-stack--flush" data-version-id=id.get_value() data-node-id=id.get_value() data-body-version-id=move ||version.with(|node|node.as_ref().and_then(FileNode::version_id).map(str::to_owned).unwrap_or_default()) data-node-kind=move ||version.with(|node|node.as_ref().map(|node|kind_name(node.kind())).unwrap_or_default())
            data-parent-set=move ||boundary.with(|boundary|if boundary.more.is_some(){"more"}else{"complete"})
            aria-label=move ||if current.get(){if inherited.get(){t("继承的父版本","Inherited parent version")}else{t("当前版本","Current version")}}else{t("历史版本","Historical version")}>
            <div class="zxs-version-date zxs-row" title=id.get_value()>
                <Show when=move ||can_modify.get()><input class="zxs-node-checkbox" type="checkbox" aria-label=t("选择此节点","Select this node") disabled=move ||disabled.get()||state.with(|s|s.files.tag_edit.is_some()) prop:checked=move ||state.with(|s|s.files.checked_nodes.contains(&id.get_value())) on:change=move |event|super::tags::toggle(state,id.get_value(),event_target_checked(&event))/></Show>
                <span class="zxs-row">{t("日期未加载","Date not loaded")}</span>
                <button class="zxs-button" data-history-field="created_at" disabled=move ||navigation_disabled.get() on:click=move |_|actions::load_history_field(state,id.get_value(),HistoryField::CreatedAt,None)>{t("读取日期","Load date")}</button>
                <Show when=move ||current.get()><span class="zxs-version-current">{move ||if inherited.get(){t("继承的父版本","Inherited parent version")}else{t("当前","Current")}}</span></Show>
            </div>
            {note}
            <Show when=move ||version.with(|node|node.as_ref().and_then(FileNode::content_state).is_some())>
                <span class="zxs-row" data-content-disposition="recorded">{move ||version.with(|node|match node.as_ref().and_then(FileNode::content_state){
                    Some(storage_contract::ContentState::Available)=>t("正文记录可用","Content recorded as available"),
                    Some(storage_contract::ContentState::Purged)=>t("正文已清除","Content purged"),
                    Some(storage_contract::ContentState::Migrated)=>t("正文已迁移","Content migrated"),
                    None=>String::new(),
                })}</span>
            </Show>
            <div class="zxs-version-actions zxs-flow zxs-flow--start">
                {edit}
                <Show when=move ||can_modify.get() fallback=move ||view!{<span class="zxs-row">{t("只读","Read only")}</span>}>
                <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|super::tags::edit_node(state,id.get_value())>{t("编辑标签","Edit tags")}</button>
                {branch::control(state,t("创建分支","Create branch"),branch::Target::HistoryNode(id.get_value()),disabled)}
                </Show>
            </div>
            <div class="zxs-version-parents zxs-stack">
                <For each=move ||boundary.with(|boundary|boundary.outside.clone()) key=|parent|parent.clone() children=move |parent|{
                    let parent=StoredValue::new(parent);
                    view!{<div class="zxs-flow" data-parent-endpoint="outside_window" data-parent-node=parent.get_value()>
                        <span class="zxs-row">{t("父节点未加载","Parent not loaded")}</span>
                        <span class="zxs-row">{parent.get_value()}</span>
                        <button class="zxs-button" title=parent.get_value() disabled=move ||navigation_disabled.get() on:click=move |_|actions::anchor_history_node(state,parent.get_value())>{t("定位父节点","Locate parent")}</button>
                    </div>}
                }/>
                <Show when=move ||boundary.with(|boundary|boundary.empty)><span class="zxs-row" data-parent-state="empty">{t("无父关系","No parent relationships")}</span></Show>
                <Show when=move ||boundary.with(|boundary|boundary.more.is_some())>
                    <button class="zxs-button" data-parent-state="more" disabled=move ||navigation_disabled.get() on:click=move |_|if let Some(cursor)=boundary.with_untracked(|boundary|boundary.more.clone()){actions::open_history_parents(state,id.get_value(),Some(cursor));}>{t("继续读取父关系","More parents")}</button>
                </Show>
            </div>
        </article>
    }
}

fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Content => "content",
        NodeKind::Rename => "rename",
        NodeKind::Tags => "tags",
        NodeKind::Merge => "merge",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParentBoundary {
    outside: Vec<String>,
    more: Option<String>,
    empty: bool,
}

fn parent_boundary(node: &FileNode) -> ParentBoundary {
    let outside = node
        .parents()
        .iter()
        .filter(|parent| parent.endpoint() == ParentEndpoint::OutsideWindow)
        .map(|parent| parent.id().to_owned())
        .collect();
    match node.parent_set() {
        ParentSet::Complete => ParentBoundary {
            outside,
            more: None,
            empty: node.parents().is_empty(),
        },
        ParentSet::More { cursor } => ParentBoundary {
            outside,
            more: Some(cursor.clone()),
            empty: false,
        },
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/version_tree/tests.rs"]
mod tests;
