//! Geometry for a proven, ranked section. A guide is a distinct arrow-free
//! overlay to the newer event line; it never enters the content DAG.
use super::*;

pub(super) fn guides(tips: &[String], boxes: &BTreeMap<String, NodeBounds>) -> BTreeMap<String, String> {
    let bounds = RoutingBounds::new(boxes.values().copied());
    if !bounds.all_finite { return BTreeMap::new(); }
    tips.iter().filter_map(|id| {
        let card = boxes.get(id)?;
        let x = card.center();
        let y = card.top;
        if y <= 0. { return None; }
        let path = if card.depth == 0 {
            rounded_route(&[(x, y), (x, 0.)], EDGE_CORNER_RADIUS)?
        } else {
            let exit = (bounds.layers.get(&card.depth)?.top - 6.).clamp(0., y);
            let side = if x - bounds.left < bounds.right - x { bounds.left - 12. } else { bounds.right + 12. };
            rounded_route(&[(x, y), (x, exit), (side, exit), (side, 0.)], EDGE_CORNER_RADIUS)?
        };
        Some((id.clone(), path))
    }).collect()
}

pub(in crate::files) fn graph<N>(
    levels: Vec<Vec<String>>, edges: Vec<(String, String)>, tips: Vec<String>,
    marker: String, render: impl Fn(String) -> N + Copy + Send + Sync + 'static,
) -> impl IntoView where N: IntoView + 'static {
    let admitted = StoredValue::new(Layout { levels, edges });
    let tree = Memo::new(move |_| admitted.get_value());
    let tips = StoredValue::new(tips);
    let geometry = RwSignal::new(Geometry::default());
    let root = NodeRef::<html::Div>::new();
    let links = NodeRef::<svg::Svg>::new();
    observe(root, links, tree, geometry, Some(tips));
    let arrow = StoredValue::new(format!("url(#{marker})"));
    view! {
        <div class="zxs-version-tree-scroll zxs-interval-scroll" aria-label=t("经验证的操作区段", "Verified operation interval")>
            <div class="zxs-version-tree-canvas zxs-interval-canvas" node_ref=root>
                <svg node_ref=links class="zxs-version-tree-links" aria-hidden="true" focusable="false"
                    viewBox=move ||geometry.with(|g|format!("0 0 {} {}",g.width.max(1.),g.height.max(1.))) preserveAspectRatio="none">
                    <defs><marker id=marker class="zxs-version-tree-arrow" markerWidth="8" markerHeight="8" refX="8" refY="4" orient="auto" markerUnits="userSpaceOnUse"><path d="M0,0 L8,4 L0,8 Z"/></marker></defs>
                    <For each=move ||tips.get_value() key=|id|id.clone() children=move |id| {
                        let key=StoredValue::new(id.clone());
                        view!{<path class="zxs-interval-guide" data-endpoint-node=id data-edge-kind="event-guide"
                            d=move ||geometry.with(|g|key.with_value(|k|g.guides.get(k).cloned().unwrap_or_default()))
                            marker-end="none" vector-effect="non-scaling-stroke"/>}
                    }/>
                    <For each=move ||admitted.with_value(|l|l.edges.clone()) key=|edge|edge.clone() children=move |(parent,child)| {
                        let key=StoredValue::new((parent.clone(),child.clone()));
                        view!{<path class="zxs-version-tree-edge" data-parent-version=parent data-child-version=child data-edge-kind="ancestry"
                            d=move ||geometry.with(|g|key.with_value(|k|g.path(k))) marker-end=arrow.get_value()
                            stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>}
                    }/>
                </svg>
                <For each=move ||admitted.with_value(|l|l.levels.iter().enumerate().map(|(d,ids)|(d,ids.clone())).collect::<Vec<_>>())
                    key=|row|row.0 children=move |(depth, ids)|{
                    view!{<div class="zxs-version-tree-level" data-version-depth=depth.to_string()>
                        {ids.into_iter().map(render).collect_view()}
                    </div>}
                }/>
            </div>
        </div>
    }
}
