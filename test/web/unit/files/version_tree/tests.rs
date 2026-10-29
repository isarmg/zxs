use super::*;
use crate::files::history::tests::{node, node_wire, window, window_wire};
use json_codec::json;

#[test]
fn layout_input_uses_only_in_page_parent_edges_and_never_invents_nodes_for_stubs() {
    let page = ["new", "old"];
    let window = window(
        vec![
            node_wire(
                json!({"node_id":"new","version_id":"new","sequence":"3","created_at":"1970-01-01T00:00:03Z",
                "parents":{"status":"more","edges":[{"node_id":"old","in_window":true},{"node_id":"outside","in_window":false}],"cursor":"cc"}}),
            ),
            node("old", 1, &[], &page),
        ],
        json!({}),
    );
    let parsed = metadata(&window.nodes);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "new");
    assert_eq!(
        parsed[0].parents,
        ["old"],
        "outside-window parents are not edges"
    );
    assert_eq!(parsed[0].created, "1970-01-01T00:00:03Z");
    let tree = layout(&parsed);
    assert_eq!(ids(&tree), BTreeSet::from(["new".into(), "old".into()]));
    assert_eq!(tree.edges, vec![("old".into(), "new".into())]);
    assert_eq!(
        window.nodes[0].parents.outside_window().collect::<Vec<_>>(),
        ["outside"]
    );
    assert!(window.nodes[0].parents.more());
}

#[test]
fn outside_window_parents_collapse_to_one_stub_per_node_whose_jump_anchors_the_first_parent() {
    let page = ["tip", "mid"];
    let window = window(
        vec![
            node_wire(
                json!({"node_id":"tip","version_id":"tip","sequence":"3","created_at":"1970-01-01T00:00:03Z",
                "parents":{"status":"more","edges":[
                    {"node_id":"a-old","in_window":false},{"node_id":"mid","in_window":true},{"node_id":"z-old","in_window":false}
                ],"cursor":"cc"}}),
            ),
            node("mid", 1, &[], &page),
        ],
        json!({}),
    );
    let stub = ParentStub::of(&window.nodes[0]).unwrap();
    assert_eq!(stub.jump.as_deref(), Some("a-old"));
    assert_eq!(stub.outside, 2, "two missing parents, one stub");
    assert!(stub.more);
    assert!(
        ParentStub::of(&window.nodes[1]).is_none(),
        "complete in-page parents need no stub"
    );
    let only_more = window.nodes[0].clone();
    let mut only_more = only_more;
    only_more.parents.edges.retain(|edge| edge.in_window);
    let stub = ParentStub::of(&only_more).unwrap();
    assert!(stub.jump.is_none() && stub.more && stub.outside == 0);
    // The jump issues an anchor request bound to the resident root.
    Owner::new().with(|| {
        let mut initial = State {
            authenticated: true,
            ..Default::default()
        };
        initial.files.selected = Some(crate::files::test_entries::entry(json!({
            "file_id":"file","repository_id":"repo","repository_path":"docs/a.txt","path":"docs/a.txt","name":"a.txt"
        })));
        initial.files.history.install(window);
        let state = RwSignal::new(initial);
        let (_, endpoint) = actions::history::begin_main(
            state,
            &actions::history::MainRequest::Anchor("a-old".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            endpoint,
            format!(
                "/api/file-manager/files/file/history?repository_id=repo&path=docs%2Fa.txt&limit=50&anchor_node_id=a-old&history_root={}",
                crate::files::history::tests::ROOT
            )
        );
    });
}

#[test]
fn rename_nodes_reuse_body_versions_but_keep_their_own_ancestry() {
    let page = ["ver-two", "fnode-rename", "ver-one"];
    let window = window(
        vec![
            node("ver-two", 3, &["fnode-rename"], &page),
            node_wire(
                json!({"node_id":"fnode-rename","version_id":"ver-one","kind":"rename","sequence":"2","created_at":"1970-01-01T00:00:02Z","previous_path":"old/a.txt",
                "parents":{"status":"complete","edges":[{"node_id":"ver-one","in_window":true}],"cursor":null}}),
            ),
            node("ver-one", 1, &[], &page),
        ],
        json!({}),
    );
    let tree = layout(&metadata(&window.nodes));
    assert_eq!(
        tree.levels,
        vec![
            vec!["ver-two".to_owned()],
            vec!["fnode-rename".to_owned()],
            vec!["ver-one".to_owned()]
        ]
    );
    assert_eq!(
        tree.edges,
        vec![
            ("fnode-rename".into(), "ver-two".into()),
            ("ver-one".into(), "fnode-rename".into())
        ]
    );
    let mut wire = window_wire(
        vec![node("fnode-rename", 2, &[], &["fnode-rename"])],
        json!({"context":{"path":"docs/a.txt","note":"","current_node_id":"fnode-rename","selected_branch_id":null,
            "version_id":"ver-one","materialized":false,"inherited":true,"own_version_id":null,
            "source_version_id":"ver-one","content_state":"PENDING","visible_content_state":"LIVE"}}),
    );
    wire["nodes"][0]["current"] = true.into();
    let mut model = Model::default();
    model
        .history
        .install(crate::files::history::HistoryWindow::parse(wire, "file").unwrap());
    assert!(inherited_current(&model, "fnode-rename"));
    assert!(!inherited_current(&model, "ver-one"));
}
#[test]
fn current_utc_history_nodes_sort_sibling_tips_by_time_then_identity() {
    let page = ["a-newer-tags", "b-newer-body", "older-rename", "root"];
    let nodes = vec![
        node_wire(
            json!({"node_id":"b-newer-body","version_id":"b-newer-body","sequence":"4","created_at":"2026-01-01T01:00:00Z",
            "parents":{"status":"complete","edges":[{"node_id":"root","in_window":true}],"cursor":null}}),
        ),
        node_wire(
            json!({"node_id":"a-newer-tags","version_id":"b-newer-body","kind":"tags","sequence":"3","created_at":"2026-01-01T01:00:00Z",
            "parents":{"status":"complete","edges":[{"node_id":"root","in_window":true}],"cursor":null}}),
        ),
        node_wire(
            json!({"node_id":"older-rename","version_id":"root","kind":"rename","previous_path":"x","sequence":"2","created_at":"2026-01-01T00:00:00Z",
            "parents":{"status":"complete","edges":[{"node_id":"root","in_window":true}],"cursor":null}}),
        ),
        node_wire(
            json!({"node_id":"root","version_id":"root","sequence":"1","created_at":"2025-12-31T23:59:59Z"}),
        ),
    ];
    let _ = page;
    let window = window(nodes, json!({}));
    let tree = layout(&metadata(&window.nodes));
    assert_eq!(
        tree.levels,
        vec![
            vec!["a-newer-tags", "b-newer-body", "older-rename"],
            vec!["root"]
        ]
    );
    let mut reversed = metadata(&window.nodes);
    reversed.reverse();
    assert_eq!(layout(&reversed), tree);
}

fn v(id: &str, created: u64, parents: &[&str]) -> Version {
    Version {
        id: id.into(),
        created: format!("1970-01-01T00:{:02}:{:02}Z", created / 60, created % 60),
        parents: parents.iter().map(|p| (*p).into()).collect(),
    }
}
fn ids(tree: &Layout) -> BTreeSet<String> {
    tree.levels.iter().flatten().cloned().collect()
}

#[test]
fn a_real_fork_has_two_siblings_and_a_merge_keeps_both_parent_edges() {
    let tree = layout(&[
        v("root", 1, &[]),
        v("left", 2, &["root"]),
        v("right", 3, &["root"]),
        v("merge", 4, &["left", "right"]),
    ]);
    assert_eq!(
        tree.levels,
        vec![vec!["merge"], vec!["right", "left"], vec!["root"]]
    );
    assert_eq!(tree.edges.len(), 4);
    assert!(tree.edges.contains(&("left".into(), "merge".into())));
    assert!(tree.edges.contains(&("right".into(), "merge".into())));
}
#[test]
fn timestamps_without_parent_evidence_never_invent_a_chain() {
    let tree = layout(&[
        v("new", 30, &[]),
        v("old", 1, &[]),
        v("missing", 2, &["invisible"]),
    ]);
    assert_eq!(tree.levels, vec![vec!["new", "missing", "old"]]);
    assert!(tree.edges.is_empty());
}
#[test]
fn topology_wins_over_misordered_timestamps_and_input_order_is_irrelevant() {
    let records = vec![
        v("late-parent", 99, &[]),
        v("early-child", 1, &["late-parent"]),
        v("sibling", 2, &["late-parent"]),
    ];
    let tree = layout(&records);
    let mut reversed = records;
    reversed.reverse();
    assert_eq!(layout(&reversed), tree);
    assert_eq!(
        tree.levels,
        vec![vec!["sibling", "early-child"], vec!["late-parent"]]
    );
}
#[test]
fn cycles_and_duplicates_are_safe_without_losing_unrelated_edges_or_nodes() {
    let tree = layout(&[
        v("a", 1, &["b"]),
        v("b", 2, &["a"]),
        v("c", 3, &["a", "a", "c"]),
        v("c", 3, &["a", "a", "c"]),
        v("d", 4, &[]),
    ]);
    assert_eq!(
        ids(&tree),
        BTreeSet::from(["a".into(), "b".into(), "c".into(), "d".into()])
    );
    assert_eq!(tree.edges, vec![("a".into(), "c".into())]);
    assert_eq!(tree.levels, vec![vec!["d", "c", "b"], vec!["a"]]);
}
#[test]
fn old_ancestors_group_by_their_newer_children_before_timestamps_and_ids() {
    let records = vec![
        v("root", 1, &[]),
        v("left", 2, &["root"]),
        v("right", 3, &["root"]),
        v("z-left-one", 30, &["left"]),
        v("z-left-two", 50, &["left"]),
        v("a-right-one", 4, &["right"]),
        v("a-right-two", 20, &["right"]),
        v("merge", 60, &["z-left-two", "a-right-one"]),
    ];
    let tree = layout(&records);
    assert_eq!(tree.levels[2], vec!["left", "right"]);
    assert_eq!(tree.edges.len(), 8);
    assert_eq!(ids(&tree).len(), records.len());
    let mut reversed = records;
    reversed.reverse();
    assert_eq!(layout(&reversed), tree);
}
#[test]
fn utc_node_timestamps_keep_a_total_stable_order_and_unknown_dates_are_last() {
    let versions = [
        v("two", 2, &[]),
        v("ten", 10, &[]),
        Version {
            id: "recent".into(),
            created: "2020-01-01T00:00:00Z".into(),
            parents: vec![],
        },
        Version {
            id: "unknown".into(),
            created: String::new(),
            parents: vec![],
        },
    ];
    for a in &versions {
        for b in &versions {
            for c in &versions {
                if newer_first(a, b).is_le() && newer_first(b, c).is_le() {
                    assert!(newer_first(a, c).is_le());
                }
            }
        }
    }
    let tree = layout(&versions);
    assert_eq!(tree.levels, vec![vec!["recent", "ten", "two", "unknown"]]);
}
#[test]
fn every_branch_tip_is_above_its_ancestors_even_when_branches_have_unequal_depths() {
    let tree = layout(&[
        v("root", 1, &[]),
        v("long-mid", 2, &["root"]),
        v("long-tip", 3, &["long-mid"]),
        v("short-tip", 100, &["root"]),
    ]);
    assert_eq!(
        tree.levels,
        vec![
            vec!["short-tip", "long-tip"],
            vec!["long-mid"],
            vec!["root"]
        ]
    );
    for (parent, child) in &tree.edges {
        let depth = |id: &String| {
            tree.levels
                .iter()
                .position(|level| level.contains(id))
                .unwrap()
        };
        assert!(
            depth(parent) > depth(child),
            "every real ancestry edge points upwards"
        );
    }
    assert!(tree.edges.contains(&("root".into(), "short-tip".into())));
}
#[test]
fn upward_cross_layer_routes_avoid_nodes_and_enter_below_the_tallest_target_sibling() {
    let child = NodeBounds {
        left: 100.,
        right: 394.,
        top: 0.,
        bottom: 110.,
        depth: 0,
    };
    let tall_sibling = NodeBounds {
        left: 430.,
        right: 724.,
        top: 0.,
        bottom: 182.,
        depth: 0,
    };
    let middle = NodeBounds {
        left: 100.,
        right: 394.,
        top: 200.,
        bottom: 310.,
        depth: 1,
    };
    let parent = NodeBounds {
        left: 100.,
        right: 394.,
        top: 328.,
        bottom: 438.,
        depth: 2,
    };
    let boxes = [child, tall_sibling, middle, parent];
    let routing = RoutingBounds::new(boxes);
    let route = long_route(parent, child, &routing);
    assert_eq!(middle.top - tall_sibling.bottom, 18.);
    assert_eq!(parent.top - middle.bottom, 18.);
    assert_eq!(route.first(), Some(&(parent.center(), parent.top)));
    assert_eq!(route[1].1, parent.top - 6.);
    assert_eq!(route[3].1, tall_sibling.bottom + 6.);
    assert!(route.windows(2).all(|segment| segment[1].1 <= segment[0].1));
    assert_eq!(route.last(), Some(&(child.center(), child.bottom + 3.)));
    assert!(route[2].0 < child.left);
    for segment in route.windows(2) {
        let [(x1, y1), (x2, y2)] = [segment[0], segment[1]];
        assert!(x1 == x2 || y1 == y2);
        for node in boxes {
            let enters = if x1 == x2 {
                x1 > node.left
                    && x1 < node.right
                    && y1.min(y2) < node.bottom
                    && y1.max(y2) > node.top
            } else {
                y1 > node.top
                    && y1 < node.bottom
                    && x1.min(x2) < node.right
                    && x1.max(x2) > node.left
            };
            assert!(
                !enters,
                "an upward route must never enter a node's interior: {segment:?} / {node:?}"
            );
        }
    }
    let across = edge_path(parent, child, &routing).unwrap();
    assert!(across.contains('H') && across.contains('V') && across.contains('A'));
    assert!(!across.contains(['C', 'L', 'Q']));
    let adjacent = edge_path(middle, child, &routing).unwrap();
    assert_eq!(adjacent, "M247.000,200.000 V113.000");
    assert!(edge_path(child, parent, &routing).is_none());
}
#[test]
fn an_eighteen_pixel_layer_gap_places_rounded_right_angles_between_nodes() {
    let child = NodeBounds {
        left: 18.,
        right: 312.,
        top: 0.,
        bottom: 110.,
        depth: 0,
    };
    let sibling = NodeBounds {
        left: 330.,
        right: 624.,
        top: 0.,
        bottom: 182.,
        depth: 0,
    };
    let parent = NodeBounds {
        left: 330.,
        right: 624.,
        top: 200.,
        bottom: 310.,
        depth: 1,
    };
    let boxes = [child, sibling, parent];
    assert_eq!(parent.top - sibling.bottom, 18.);
    let routing = RoutingBounds::new(boxes);
    let (exit, entry) = gap_heights(parent, child, &routing);
    assert_eq!((exit, entry), (194., 188.));
    assert!(parent.top > exit && exit > entry && entry > child.bottom + 3.);
    let path = edge_path(parent, child, &routing).unwrap();
    assert_eq!(
        path,
        "M477.000,200.000 V195.000 A4.000,4.000 0 0 0 473.000,191.000 H169.000 A4.000,4.000 0 0 1 165.000,187.000 V113.000"
    );
    assert!(191. - EDGE_CORNER_RADIUS > sibling.bottom);
    assert!(191. + EDGE_CORNER_RADIUS < parent.top);
}
#[test]
fn orthogonal_turns_have_circular_arcs_and_short_runs_limit_both_corner_radii() {
    assert_eq!(
        rounded_route(&[(10., 40.), (10., 20.), (50., 20.), (50., 0.)], 4.).unwrap(),
        "M10.000,40.000 V24.000 A4.000,4.000 0 0 1 14.000,20.000 H46.000 A4.000,4.000 0 0 0 50.000,16.000 V0.000"
    );
    assert_eq!(
        rounded_route(&[(0., 10.), (0., 5.), (2., 5.), (2., 0.)], 4.).unwrap(),
        "M0.000,10.000 V6.000 A1.000,1.000 0 0 1 1.000,5.000 A1.000,1.000 0 0 0 2.000,4.000 V0.000"
    );
    assert_eq!(
        rounded_route(&[(3., 10.), (3., 10.), (3., 5.), (3., 0.)], 4.).unwrap(),
        "M3.000,10.000 V0.000"
    );
    for points in [
        &[(0., 0.)][..],
        &[(0., 0.), (1., 1.)][..],
        &[(0., 0.), (0., 1.), (0., 0.)][..],
        &[(0., 0.), (f64::NAN, 1.)][..],
    ] {
        assert!(rounded_route(points, 4.).is_none());
    }
    assert!(rounded_route(&[(0., 0.), (0., 1.)], f64::INFINITY).is_none());
}
#[test]
fn metadata_ignores_notes_current_markers_and_self_or_duplicate_parents() {
    let page = ["b", "a"];
    let mut wire = window_wire(
        vec![
            node_wire(
                json!({"node_id":"b","version_id":"b","sequence":"2","created_at":"1970-01-01T00:00:02Z",
                "parents":{"status":"complete","edges":[{"node_id":"a","in_window":true},{"node_id":"foreign","in_window":false}],"cursor":null}}),
            ),
            node("a", 1, &[], &page),
        ],
        json!({}),
    );
    let before = metadata(&window(wire["nodes"].as_array().unwrap().clone(), json!({})).nodes);
    wire["nodes"][1]["note"] = "typing stays mounted".into();
    wire["context"]["current_node_id"] = "a".into();
    wire["nodes"][1]["current"] = true.into();
    let refreshed = crate::files::history::HistoryWindow::parse(wire, "file").unwrap();
    assert_eq!(metadata(&refreshed.nodes), before);
    let tree = layout(&before);
    assert_eq!(tree.edges, vec![("a".into(), "b".into())]);
    assert_eq!(ids(&tree), BTreeSet::from(["a".into(), "b".into()]));
}

#[test]
fn inherited_current_markers_do_not_change_real_version_identity_or_ancestry() {
    let mut wire = window_wire(
        vec![node("parent", 1, &[], &["parent"])],
        json!({"context":{"path":"docs/a.txt","note":"","current_node_id":"parent","selected_branch_id":null,
            "version_id":"parent","materialized":false,"inherited":true,"own_version_id":null,
            "source_version_id":"parent","content_state":"PENDING","visible_content_state":"LIVE"}}),
    );
    wire["nodes"][0]["current"] = true.into();
    let mut model = Model::default();
    model
        .history
        .install(crate::files::history::HistoryWindow::parse(wire.clone(), "file").unwrap());
    let before = metadata(&model.history.window().unwrap().nodes);
    assert!(inherited_current(&model, "parent"));
    assert!(!inherited_current(&model, "other"));
    wire["context"]["materialized"] = true.into();
    wire["context"]["inherited"] = false.into();
    wire["context"]["own_version_id"] = "parent".into();
    wire["context"]["source_version_id"] = json_codec::Value::Null;
    wire["context"]["content_state"] = "LIVE".into();
    model
        .history
        .install(crate::files::history::HistoryWindow::parse(wire, "file").unwrap());
    assert!(!inherited_current(&model, "parent"));
    assert_eq!(metadata(&model.history.window().unwrap().nodes), before);
    assert_eq!(layout(&before).levels, vec![vec!["parent"]]);
}

#[test]
fn a_replaced_window_changes_the_tree_identity_and_keeps_it_for_note_refreshes() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let identity = Memo::new(move |_| {
            state.with(|s| s.files.history.window().map(|window| window.identity()))
        });
        assert!(identity.get().is_none());
        let page = ["n2", "n1"];
        let first = window(
            vec![node("n2", 2, &["n1"], &page), node("n1", 1, &[], &page)],
            json!({}),
        );
        state.update(|s| s.files.history.install(first.clone()));
        let before = identity.get().unwrap();
        // A refresh on the same root with the same first node (note edits,
        // tag changes elsewhere) keeps the mounted tree.
        let mut refreshed = first.clone();
        refreshed.nodes[1].note = "edited".into();
        state.update(|s| s.files.history.install(refreshed));
        assert_eq!(identity.get().unwrap(), before);
        // Paging replaces the tree.
        state.update(|s| {
            s.files.history.install(window(
                vec![node("n1", 1, &[], &["n1"])],
                json!({"previous_cursor":"aa"}),
            ))
        });
        assert_ne!(identity.get().unwrap(), before);
        state.update(|s| s.files.history.clear());
        assert!(identity.get().is_none());
    });
}

// Frozen pre-indexed layout oracle. This implementation is compiled only for tests.
fn reference_reachable(
    start: &str,
    goal: &str,
    children: &BTreeMap<String, BTreeSet<String>>,
) -> bool {
    let mut pending = vec![start.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if id == goal {
            return true;
        }
        if seen.insert(id.clone()) {
            pending.extend(children.get(&id).into_iter().flatten().cloned());
        }
    }
    false
}

/// Only declared, visible parents produce edges. Cyclic edges are isolated
/// rather than inventing chronology or making the traversal recurse forever.
fn reference_layout(versions: &[Version]) -> Layout {
    let mut nodes = BTreeMap::<String, Version>::new();
    // Stable conflict handling also makes duplicate identities harmless.
    let mut sorted = versions.to_vec();
    sorted.sort();
    for version in sorted {
        nodes.entry(version.id.clone()).or_insert(version);
    }
    let mut children = BTreeMap::<String, BTreeSet<String>>::new();
    for version in nodes.values() {
        for parent in &version.parents {
            if parent != &version.id && nodes.contains_key(parent) {
                children
                    .entry(parent.clone())
                    .or_default()
                    .insert(version.id.clone());
            }
        }
    }
    let mut edges = Vec::new();
    for (parent, descendants) in &children {
        for child in descendants {
            if !reference_reachable(child, parent, &children) {
                edges.push((parent.clone(), child.clone()));
            }
        }
    }
    // Walk from all branch tips towards old ancestors. This is a reverse
    // topological layout, not a visual reversal of the old root-first layers:
    // tips of both short and long branches belong on the first layer.
    let mut remaining_children = nodes
        .keys()
        .map(|id| (id.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut accepted_parents = BTreeMap::<String, Vec<String>>::new();
    for (parent, child) in &edges {
        *remaining_children.get_mut(parent).expect("visible parent") += 1;
        accepted_parents
            .entry(child.clone())
            .or_default()
            .push(parent.clone());
    }
    let mut ready = nodes
        .values()
        .filter(|v| remaining_children[&v.id] == 0)
        .collect::<Vec<_>>();
    ready.sort_by(|a, b| newer_first(a, b));
    let mut depths = BTreeMap::<String, usize>::new();
    let mut levels = Vec::<Vec<String>>::new();
    while !ready.is_empty() {
        let version = ready.remove(0);
        let depth = depths.get(&version.id).copied().unwrap_or(0);
        if levels.len() <= depth {
            levels.resize(depth + 1, Vec::new());
        }
        levels[depth].push(version.id.clone());
        for parent in accepted_parents.get(&version.id).into_iter().flatten() {
            depths
                .entry(parent.clone())
                .and_modify(|d| *d = (*d).max(depth + 1))
                .or_insert(depth + 1);
            let count = remaining_children.get_mut(parent).expect("visible parent");
            *count -= 1;
            if *count == 0 {
                ready.push(&nodes[parent]);
            }
        }
        ready.sort_by(|a, b| newer_first(a, b));
    }
    if let Some(tips) = levels.first_mut() {
        tips.sort_by(|a, b| newer_first(&nodes[a], &nodes[b]));
    }
    for depth in 1..levels.len() {
        let positions = levels[depth - 1]
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        // Group ancestors by the positions of their newer children on the
        // preceding display layer; all cross-layer source edges remain intact.
        let centers = levels[depth]
            .iter()
            .map(|id| {
                let children = edges
                    .iter()
                    .filter(|(parent, _)| parent == id)
                    .filter_map(|(_, child)| positions.get(child.as_str()).copied())
                    .collect::<Vec<_>>();
                let key = if children.is_empty() {
                    (usize::MAX as u128, 1u128, usize::MAX)
                } else {
                    (
                        children.iter().map(|index| *index as u128).sum(),
                        children.len() as u128,
                        *children.iter().min().expect("nonempty children"),
                    )
                };
                (id.clone(), key)
            })
            .collect::<BTreeMap<_, _>>();
        levels[depth].sort_by(|a, b| {
            let (a_sum, a_count, a_min) = centers[a];
            let (b_sum, b_count, b_min) = centers[b];
            (a_sum * b_count)
                .cmp(&(b_sum * a_count))
                .then_with(|| a_min.cmp(&b_min))
                .then_with(|| newer_first(&nodes[a], &nodes[b]))
        });
    }
    Layout { levels, edges }
}

#[test]
fn indexed_layout_matches_reference_for_every_four_node_directed_graph() {
    let names = ["a", "b", "c", "d"];
    let candidates = (0..names.len())
        .flat_map(|parent| {
            (0..names.len())
                .filter(move |&child| child != parent)
                .map(move |child| (parent, child))
        })
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 12);
    for mask in 0..1usize << candidates.len() {
        let mut records = names
            .iter()
            .enumerate()
            .map(|(index, name)| v(name, index as u64, &[]))
            .collect::<Vec<_>>();
        for (bit, &(parent, child)) in candidates.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                records[child].parents.push(names[parent].into());
            }
        }
        for dates in [
            ["same", "same", "same", "same"],
            ["", "later", "earlier", ""],
            ["z", "c", "b", "a"],
        ] {
            for (record, created) in records.iter_mut().zip(dates) {
                record.created = created.into();
            }
            let expected = reference_layout(&records);
            assert_eq!(layout(&records), expected, "mask {mask}, dates {dates:?}");
            records.reverse();
            assert_eq!(layout(&records), expected, "reversed mask {mask}");
            records.reverse();
        }
    }
}

#[test]
fn indexed_layout_preserves_conflicting_identity_and_parent_normalization() {
    let mut records = vec![
        v("a", 1, &["missing", "a"]),
        v("b", 2, &["a", "a", "missing", "b"]),
        v("c", 3, &["b", "a"]),
        // Whole-Version ordering chooses this duplicate before normalization.
        v("c", 3, &["a", "b"]),
        v("c", 4, &["d"]),
        v("d", 4, &["c"]),
        v("e", 5, &[]),
    ];
    let expected = reference_layout(&records);
    for _ in 0..records.len() {
        assert_eq!(layout(&records), expected);
        records.rotate_left(1);
    }
    records.reverse();
    assert_eq!(layout(&records), expected);
    assert_eq!(layout(&[]), reference_layout(&[]));
}

#[test]
fn indexed_layout_rejects_all_cycle_edges_but_keeps_external_and_transitive_edges() {
    let records = [
        v("source", 1, &[]),
        v("a", 2, &["source", "c"]),
        v("b", 3, &["a"]),
        v("c", 4, &["a", "b"]),
        v("tail", 5, &["b", "c"]),
        v("tip", 6, &["tail", "source"]),
    ];
    let tree = layout(&records);
    assert_eq!(tree, reference_layout(&records));
    assert_eq!(
        tree.edges,
        vec![
            ("b".into(), "tail".into()),
            ("c".into(), "tail".into()),
            ("source".into(), "a".into()),
            ("source".into(), "tip".into()),
            ("tail".into(), "tip".into()),
        ]
    );
    let triangle = [v("a", 1, &[]), v("b", 2, &["a"]), v("c", 3, &["a", "b"])];
    assert_eq!(layout(&triangle).edges.len(), 3, "not transitive reduction");
}

fn measured_layout(records: &[Version]) -> (Layout, LayoutWork) {
    LAYOUT_WORK.with(|work| *work.borrow_mut() = LayoutWork::default());
    let tree = layout(records);
    let work = LAYOUT_WORK.with(|work| std::mem::take(&mut *work.borrow_mut()));
    (tree, work)
}

fn assert_linear_graph_visits(work: &LayoutWork, nodes: usize, candidates: usize, accepted: usize) {
    assert_eq!(work.component_nodes, 2 * nodes);
    assert_eq!(work.component_edges, 2 * candidates);
    assert_eq!(work.candidate_edges, candidates);
    assert_eq!(work.topology_edges, accepted);
    assert_eq!(work.center_edges, accepted);
    assert_eq!(work.ready_pushes, nodes);
    assert_eq!(work.ready_pops, nodes);
}

#[test]
fn a_long_chain_uses_linear_graph_visits_and_no_recursive_traversal() {
    for count in [2_048, 4_096, 20_000] {
        let records = (0..count)
            .map(|index| Version {
                id: format!("node-{index:05}"),
                created: String::new(),
                parents: if index == 0 {
                    vec![]
                } else {
                    vec![format!("node-{:05}", index - 1)]
                },
            })
            .collect::<Vec<_>>();
        let (tree, work) = measured_layout(&records);
        assert_linear_graph_visits(&work, count, count - 1, count - 1);
        assert_eq!(tree.levels.len(), count);
        assert_eq!(tree.edges.len(), count - 1);
        for (depth, level) in tree.levels.iter().enumerate() {
            assert_eq!(level, &[format!("node-{:05}", count - depth - 1)]);
        }
        // The old chain did N(N-1)/2 reachability visits plus (N-1)^2
        // center predicates. This asserts operation counts, not elapsed time.
        let old_visits = count * (count - 1) / 2 + (count - 1) * (count - 1);
        let visits = work.component_nodes
            + work.component_edges
            + work.candidate_edges
            + work.topology_edges
            + work.center_edges
            + work.ready_pushes
            + work.ready_pops;
        assert!(visits < 10 * count);
        assert!(old_visits > visits * 100);
    }
}

#[test]
fn wide_layered_dag_visits_each_edge_once_per_layout_stage() {
    let width = 2_048;
    let layers = 4;
    let mut records = Vec::new();
    for layer in 0..layers {
        for column in 0..width {
            records.push(Version {
                id: format!("node-{layer}-{column:04}"),
                created: if column % 3 == 0 {
                    String::new()
                } else {
                    format!("date-{:04}", column % 11)
                },
                parents: if layer == 0 {
                    vec![]
                } else {
                    vec![
                        format!("node-{}-{column:04}", layer - 1),
                        format!("node-{}-{:04}", layer - 1, (column + 1) % width),
                    ]
                },
            });
        }
    }
    let edges = (layers - 1) * width * 2;
    let (tree, work) = measured_layout(&records);
    assert_linear_graph_visits(&work, layers * width, edges, edges);
    assert_eq!(tree.edges.len(), edges);
    assert_eq!(tree.levels.len(), layers);
    assert!(tree.levels.iter().all(|level| level.len() == width));
    assert_eq!(ids(&tree).len(), layers * width);
    records.reverse();
    assert_eq!(layout(&records), tree);
}

#[test]
fn a_large_cycle_keeps_nodes_without_recursive_or_per_edge_reachability() {
    let count = 20_000;
    let records = (0..count)
        .map(|index| Version {
            id: format!("node-{index:05}"),
            created: String::new(),
            parents: vec![format!("node-{:05}", (index + count - 1) % count)],
        })
        .collect::<Vec<_>>();
    let (tree, work) = measured_layout(&records);
    assert_linear_graph_visits(&work, count, count, 0);
    assert!(tree.edges.is_empty());
    assert_eq!(tree.levels.len(), 1);
    assert_eq!(
        tree.levels[0],
        records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn equal_rational_centers_use_minimum_child_then_original_priority() {
    let different_minimum = [
        v("outer", 1, &[]),
        v("middle", 99, &[]),
        v("tip-0", 30, &["outer"]),
        v("tip-1", 20, &["middle"]),
        v("tip-2", 10, &["outer"]),
    ];
    let tree = layout(&different_minimum);
    assert_eq!(tree, reference_layout(&different_minimum));
    // Both means are one; {0, 2} precedes {1} even with an older timestamp.
    assert_eq!(
        tree.levels,
        vec![vec!["tip-0", "tip-1", "tip-2"], vec!["outer", "middle"]]
    );

    let different_count = [
        v("outer", 1, &[]),
        v("all", 99, &[]),
        v("tip-0", 30, &["outer", "all"]),
        v("tip-1", 20, &["all"]),
        v("tip-2", 10, &["outer", "all"]),
    ];
    let tree = layout(&different_count);
    assert_eq!(tree, reference_layout(&different_count));
    // 2/2 == 3/3 and both minima are zero, so newer_first breaks the tie.
    assert_eq!(
        tree.levels,
        vec![vec!["tip-0", "tip-1", "tip-2"], vec!["all", "outer"]]
    );
}

// Frozen pre-aggregation connector routing, compiled only for tests.
fn reference_gap_heights(
    parent: NodeBounds,
    child: NodeBounds,
    boxes: &[NodeBounds],
) -> (f64, f64) {
    let exit = boxes
        .iter()
        .filter(|node| node.depth == parent.depth)
        .map(|node| node.top)
        .fold(parent.top, f64::min)
        - 6.;
    let entry = boxes
        .iter()
        .filter(|node| node.depth == child.depth)
        .map(|node| node.bottom)
        .fold(child.bottom, f64::max)
        + 6.;
    (exit, entry)
}

fn reference_long_route(
    parent: NodeBounds,
    child: NodeBounds,
    boxes: &[NodeBounds],
) -> Vec<(f64, f64)> {
    let (exit, entry) = reference_gap_heights(parent, child, boxes);
    let sx = parent.center();
    let tx = child.center();
    let left = boxes
        .iter()
        .map(|node| node.left)
        .fold(parent.left, f64::min)
        - 9.;
    let right = boxes
        .iter()
        .map(|node| node.right)
        .fold(parent.right, f64::max)
        + 9.;
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

fn reference_edge_path(
    parent: NodeBounds,
    child: NodeBounds,
    boxes: &[NodeBounds],
) -> Option<String> {
    if parent.depth <= child.depth
        || parent.top <= child.bottom
        || !boxes.iter().all(|node| {
            [node.left, node.top, node.right, node.bottom]
                .iter()
                .all(|value| value.is_finite())
        })
    {
        return None;
    }
    if parent.depth > child.depth + 1 {
        return rounded_route(
            &reference_long_route(parent, child, boxes),
            EDGE_CORNER_RADIUS,
        );
    }
    let (exit, entry) = reference_gap_heights(parent, child, boxes);
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

#[test]
fn aggregated_routing_matches_exact_reference_paths_for_varied_bounds() {
    let mut seed = 0x2b64_f91d_a732_c508u64;
    for scene in 0..256 {
        let mut boxes = Vec::new();
        for depth in 0..5 {
            for column in 0..4 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let left = column as f64 * 130. - 260. + (seed % 19) as f64 / 8.;
                let top = depth as f64 * 210. + (seed % 13) as f64 / 4.;
                boxes.push(NodeBounds {
                    left,
                    right: left + 60. + (seed % 41) as f64,
                    top,
                    bottom: top + 40. + (seed % 100) as f64,
                    depth,
                });
            }
        }
        let bounds = RoutingBounds::new(boxes.iter().copied());
        for &parent in &boxes {
            for &child in &boxes {
                assert_eq!(
                    edge_path(parent, child, &bounds),
                    reference_edge_path(parent, child, &boxes),
                    "scene {scene}, parent {parent:?}, child {child:?}"
                );
            }
        }
    }
}

#[test]
fn routing_extrema_preserve_empty_partial_nonfinite_and_lane_tie_behavior() {
    let child = NodeBounds {
        left: -0.,
        right: 100.,
        top: -0.,
        bottom: 50.,
        depth: 0,
    };
    let parent = NodeBounds {
        left: 0.,
        right: 100.,
        top: 400.,
        bottom: 450.,
        depth: 2,
    };
    let middle = NodeBounds {
        left: 0.,
        right: 100.,
        top: 200.,
        bottom: 250.,
        depth: 1,
    };
    let outer = NodeBounds {
        left: -500.,
        right: 700.,
        top: 200.,
        bottom: 330.,
        depth: 1,
    };
    for boxes in [
        vec![],
        vec![middle],
        vec![child],
        vec![parent],
        vec![parent, child],
        vec![outer, child, middle, parent],
    ] {
        let bounds = RoutingBounds::new(boxes.iter().copied());
        assert_eq!(
            gap_heights(parent, child, &bounds),
            reference_gap_heights(parent, child, &boxes)
        );
        assert_eq!(
            long_route(parent, child, &bounds),
            reference_long_route(parent, child, &boxes)
        );
        assert_eq!(
            edge_path(parent, child, &bounds),
            reference_edge_path(parent, child, &boxes)
        );
    }
    let bounds = RoutingBounds::new([parent, middle, child]);
    assert_eq!(
        long_route(parent, child, &bounds)[2].0,
        -9.,
        "equal lane distances still choose the left lane"
    );
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for field in 0..4 {
            let mut unrelated = outer;
            unrelated.depth = 99;
            match field {
                0 => unrelated.left = invalid,
                1 => unrelated.right = invalid,
                2 => unrelated.top = invalid,
                _ => unrelated.bottom = invalid,
            }
            let boxes = [parent, child, unrelated];
            assert_eq!(
                edge_path(parent, child, &RoutingBounds::new(boxes)),
                reference_edge_path(parent, child, &boxes)
            );
            assert!(edge_path(parent, child, &RoutingBounds::new(boxes)).is_none());
        }
    }
}

fn reference_edge_paths(
    layout: &Layout,
    boxes: &BTreeMap<String, NodeBounds>,
) -> BTreeMap<(String, String), String> {
    let all = boxes.values().copied().collect::<Vec<_>>();
    layout
        .edges
        .iter()
        .filter_map(|(parent, child)| {
            Some((
                (parent.clone(), child.clone()),
                reference_edge_path(*boxes.get(parent)?, *boxes.get(child)?, &all)?,
            ))
        })
        .collect()
}

#[test]
fn indexed_paths_preserve_order_and_missing_endpoint_behavior() {
    let tree = layout(&[
        v("root", 1, &[]),
        v("left", 2, &["root"]),
        v("right", 3, &["root"]),
        v("merge", 4, &["root", "left", "right"]),
        v("orphan", 5, &[]),
    ]);
    let mut boxes = tree
        .levels
        .iter()
        .enumerate()
        .flat_map(|(depth, ids)| {
            ids.iter().enumerate().map(move |(column, id)| {
                (
                    id.clone(),
                    NodeBounds {
                        left: column as f64 * 200.,
                        right: column as f64 * 200. + 100.,
                        top: depth as f64 * 200.,
                        bottom: depth as f64 * 200. + 100.,
                        depth,
                    },
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let paths = edge_paths(&tree, &boxes);
    assert_eq!(paths, reference_edge_paths(&tree, &boxes));
    assert_eq!(paths.keys().cloned().collect::<Vec<_>>(), tree.edges);
    boxes.remove("left");
    let paths = edge_paths(&tree, &boxes);
    assert_eq!(paths, reference_edge_paths(&tree, &boxes));
    assert_eq!(paths.len(), tree.edges.len() - 2);
    assert!(
        paths
            .keys()
            .all(|(parent, child)| parent != "left" && child != "left")
    );
    boxes.get_mut("orphan").unwrap().bottom = f64::NAN;
    assert!(
        edge_paths(&tree, &boxes).is_empty(),
        "an unrelated nonfinite box invalidates all paths"
    );
    boxes.clear();
    assert!(edge_paths(&tree, &boxes).is_empty());
}

#[test]
fn geometry_aggregates_each_box_once_and_indexes_each_path_lookup() {
    for count in [2_048, 4_096, 20_000] {
        let boxes = (0..count)
            .map(|depth| {
                (
                    format!("node-{depth:05}"),
                    NodeBounds {
                        left: 0.,
                        right: 100.,
                        top: depth as f64 * 120.,
                        bottom: depth as f64 * 120. + 100.,
                        depth,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let tree = Layout {
            levels: (0..count)
                .map(|depth| vec![format!("node-{depth:05}")])
                .collect(),
            edges: (1..count)
                .flat_map(|depth| {
                    (1..=2.min(depth)).map(move |offset| {
                        (
                            format!("node-{depth:05}"),
                            format!("node-{:05}", depth - offset),
                        )
                    })
                })
                .collect(),
        };
        GEOMETRY_WORK.with(|work| *work.borrow_mut() = GeometryWork::default());
        let geometry = Geometry {
            paths: edge_paths(&tree, &boxes),
            ..Default::default()
        };
        assert_eq!(geometry.paths.len(), tree.edges.len());
        for edge in &tree.edges {
            assert!(geometry.path(edge).starts_with('M'));
        }
        GEOMETRY_WORK.with(|work| {
            assert_eq!(
                *work.borrow(),
                GeometryWork {
                    bounds_visits: count,
                    edge_visits: tree.edges.len(),
                    path_lookups: tree.edges.len(),
                }
            )
        });
    }
}

#[test]
fn indexed_path_getters_observe_new_geometry_without_changing_edge_identity() {
    Owner::new().with(|| {
        let edge = ("parent".to_owned(), "child".to_owned());
        let geometry = RwSignal::new(Geometry {
            paths: BTreeMap::from([(edge.clone(), "first".into())]),
            ..Default::default()
        });
        let key = edge.clone();
        let path = Memo::new(move |_| geometry.with(|g| g.path(&key)));
        assert_eq!(path.get_untracked(), "first");
        let keys = geometry.with_untracked(|g| g.paths.keys().cloned().collect::<Vec<_>>());
        geometry.update(|g| {
            g.paths.insert(edge.clone(), "resized".into());
        });
        assert_eq!(path.get_untracked(), "resized");
        assert_eq!(
            geometry.with_untracked(|g| g.paths.keys().cloned().collect::<Vec<_>>()),
            keys
        );
        geometry.update(|g| {
            g.paths.remove(&edge);
        });
        assert_eq!(path.get_untracked(), "");
    });
}
