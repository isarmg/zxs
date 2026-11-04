use super::*;

fn diamond<'a>(ids: &'a [String], layers: usize) -> FileAncestry<'a> {
    let mut parents = BTreeMap::from([(ids[0].as_str(), BTreeSet::new())]);
    for layer in 0..layers {
        let previous = ids[layer * 3].as_str();
        let left = ids[layer * 3 + 1].as_str();
        let right = ids[layer * 3 + 2].as_str();
        let tip = ids[layer * 3 + 3].as_str();
        parents.insert(left, BTreeSet::from([previous]));
        parents.insert(right, BTreeSet::from([previous]));
        parents.insert(tip, BTreeSet::from([left, right]));
    }
    FileAncestry { parents }
}
#[test]
fn diamond_charges_49_unique_nodes_and_64_edges_not_65537_paths() {
    let ids = (0..49).map(|n| format!("node_{n:03}")).collect::<Vec<_>>();
    let index = diamond(&ids, 16);
    let mut budget = SearchBudget {
        nodes: 49,
        edges: 64,
    };
    assert_eq!(
        index
            .source_branch(
                "absent",
                [(ids[48].as_str(), "branch")],
                &mut budget,
                || Ok(())
            )
            .unwrap(),
        None
    );
    assert_eq!((budget.nodes, budget.edges), (0, 0));
    let mut budget = SearchBudget {
        nodes: 49,
        edges: 64,
    };
    assert!(
        index
            .contains_all(
                &ids[48],
                &BTreeSet::from([ids[0].as_str(), ids[3].as_str(), ids[24].as_str()]),
                &mut budget,
                || Ok(())
            )
            .unwrap()
    );
}
#[test]
fn multi_head_search_finds_later_source_independent_of_candidate_order() {
    let ids = (0..49).map(|n| format!("node_{n:03}")).collect::<Vec<_>>();
    let mut index = diamond(&ids, 16);
    index.parents.insert("source", BTreeSet::new());
    index.parents.insert("short", BTreeSet::from(["source"]));
    for roots in [
        [(ids[48].as_str(), "diamond"), ("short", "proven")],
        [("short", "proven"), (ids[48].as_str(), "diamond")],
    ] {
        // Too little work to exhaust the first graph, ample for a fair shallow proof.
        let mut budget = SearchBudget { nodes: 6, edges: 4 };
        assert_eq!(
            index
                .source_branch("source", roots, &mut budget, || Ok(()))
                .unwrap(),
            Some("proven")
        );
    }
}
#[test]
fn incomplete_global_search_is_resource_error_and_never_absence() {
    let ids = (0..49).map(|n| format!("node_{n:03}")).collect::<Vec<_>>();
    let index = diamond(&ids, 16);
    let mut budget = SearchBudget { nodes: 5, edges: 5 };
    assert_eq!(
        index
            .source_branch(
                "absent",
                [(ids[48].as_str(), "branch")],
                &mut budget,
                || Ok(())
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let mut budget = SearchBudget { nodes: 0, edges: 0 };
    assert_eq!(
        index
            .contains_all(
                &ids[48],
                &BTreeSet::from([ids[0].as_str()]),
                &mut budget,
                || Ok(())
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}
#[test]
fn repeated_merge_records_do_not_duplicate_graph_edges() {
    // Builder is the same distinct-edge admission used by body/path/metadata
    // parents and every merge provenance record.
    let mut builder = Builder {
        parents: BTreeMap::new(),
        edges: 0,
        headers: 0,
        parent_records: 0,
    };
    builder.node("child").unwrap();
    builder.node("parent").unwrap();
    for _ in 0..100 {
        builder.edge("child", "parent").unwrap();
    }
    assert_eq!(builder.edges, 1);
    assert_eq!(builder.parents["child"].len(), 1);
    let index = FileAncestry {
        parents: builder.parents,
    };
    let mut budget = SearchBudget { nodes: 2, edges: 1 };
    assert_eq!(
        index
            .source_branch("absent", [("child", "branch")], &mut budget, || Ok(()))
            .unwrap(),
        None
    );
}
#[test]
fn index_admission_bounds_vertices_and_edges_without_partial_success() {
    let mut builder = Builder {
        parents: BTreeMap::new(),
        edges: MAX_ANCESTRY_EDGES,
        headers: 0,
        parent_records: 0,
    };
    builder.node("child").unwrap();
    builder.node("parent").unwrap();
    assert_eq!(
        builder.edge("child", "parent").unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert!(builder.parents["child"].is_empty());
}

#[test]
fn global_header_and_raw_parent_scans_have_separate_hard_limits() {
    let mut builder = Builder {
        parents: BTreeMap::new(),
        edges: 0,
        headers: MAX_ANCESTRY_HEADERS,
        parent_records: MAX_INDEX_PARENT_RECORDS,
    };
    assert_eq!(
        builder.header().unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert_eq!(
        builder.edge("outside", "outside").unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}

#[test]
fn chosen_tag_inheritance_cannot_continue_after_shared_work_is_exhausted() {
    let mut budget = SearchBudget { nodes: 0, edges: 0 };
    assert_eq!(
        chosen_tags(&State::default(), "unvisited", &mut budget)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}
