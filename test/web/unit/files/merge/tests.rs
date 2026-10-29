use super::*;
use crate::files::history::tests::{ROOT, branch_page, branch_wire, chain, node, window};

fn heads() -> BTreeMap<String, InputBranch> {
    [
        ("left", Some("fnode-left")),
        ("right", Some("fnode-right")),
        ("empty", None),
    ]
    .into_iter()
    .map(|(id, head)| {
        (
            id.to_owned(),
            InputBranch {
                id: id.into(),
                name: id.into(),
                head: head.map(str::to_owned),
            },
        )
    })
    .collect()
}
fn draft() -> Draft {
    let mut selected = heads();
    selected.remove("empty");
    let input_bytes = selected.values().map(InputBranch::encoded_bytes).sum();
    Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        root: HistoryRoot::parse(ROOT).unwrap(),
        branch_heads: selected,
        input_bytes,
        offset: 0,
        chosen: "fnode-unrelated".into(),
        chosen_summary: Some(NodeLabel {
            node_id: "fnode-unrelated".into(),
            version_id: "body-old".into(),
            path: "a.txt".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }),
        survivor: String::new(),
    }
}
fn root() -> HistoryRoot {
    HistoryRoot::parse(ROOT).unwrap()
}

#[test]
fn navigation_discards_an_open_merge_review() {
    use crate::files::model::{Model, Navigation};
    for destination in [
        Navigation::Root,
        Navigation::Disk("B01".into()),
        Navigation::Directory("other".into()),
    ] {
        let mut model = Model {
            merge: Some(draft()),
            ..Default::default()
        };
        assert!(model.merge.is_some());
        model.navigate(destination);
        assert!(model.merge.is_none());
    }
}

#[test]
fn merging_requires_an_explicit_survivor_and_accepts_any_file_node_from_its_summary() {
    let mut draft = draft();
    assert!(request(&draft, Some(&root())).is_err());
    draft.survivor = "left".into();
    let body = request(&draft, Some(&root())).unwrap();
    assert_eq!(body["chosen_node_id"], "fnode-unrelated");
    assert_eq!(body["inputs"][0]["expected_node_id"], "fnode-left");
    assert_eq!(body["surviving_branch_id"], "left");
    assert_eq!(body["confirmed"], true);
    assert!(body.get("content").is_none());
    // The chosen node does not need to be on the resident page.
    assert!(request(&draft, Some(&root())).is_ok());
}
#[test]
fn unknown_branches_missing_baselines_and_stale_roots_cannot_be_submitted() {
    let mut draft = draft();
    draft.survivor = "left".into();
    draft
        .branch_heads
        .insert("empty".into(), heads().remove("empty").unwrap());
    assert!(request(&draft, Some(&root())).is_err());
    draft.branch_heads.remove("empty");
    assert!(request(&draft, Some(&root())).is_ok());
    assert!(request(&draft, None).is_err(), "no current root");
    let other = HistoryRoot::parse(crate::files::history::tests::OTHER_ROOT).unwrap();
    let error = request(&draft, Some(&other)).unwrap_err();
    assert!(error.contains("changed"), "a new root requires re-review");
    draft.chosen_summary = None;
    assert!(request(&draft, Some(&root())).is_err());
}

#[test]
fn availability_counts_only_writable_branches_of_a_current_root() {
    for (state, alias, read_only) in [
        ("active", None, false),
        ("active", Some("left"), true),
        ("archived", None, true),
        ("migrated", None, true),
        ("merged", Some("left"), true),
    ] {
        let mut model = Model::default();
        assert!(!available(&model), "no window");
        model.history.install(chain(1));
        assert!(!available(&model), "no branch page");
        model.history.branches = Some(branch_page(
            vec![
                branch_wire(json!({"branch_id":"left"})),
                branch_wire(
                    json!({"branch_id":"right","state":state,"alias_of":alias,"read_only":read_only}),
                ),
            ],
            None,
        ));
        assert_eq!(available(&model), !read_only);
        model
            .history
            .mark_stale(crate::files::history::state::StaleReason::RootChanged);
        assert!(!available(&model), "a stale root offers no merge entry");
    }
}

#[test]
fn the_chosen_summary_survives_page_replacement_and_choosing_needs_a_resident_node() {
    let mut model = Model::default();
    model.history.install(chain(3));
    model.merge = Some(draft());
    assert!(!choose(&mut model, "missing"));
    assert!(choose(&mut model, "n2"));
    let summary = model
        .merge
        .as_ref()
        .unwrap()
        .chosen_summary()
        .unwrap()
        .clone();
    assert_eq!(summary.node_id, "n2");
    assert_eq!(summary.version_id, "n2");
    // Page to a window that does not contain n2.
    model.history.install(window(
        vec![node("n9", 9, &[], &["n9"])],
        json!({"previous_cursor":"aa"}),
    ));
    let draft = model.merge.as_ref().unwrap();
    assert_eq!(draft.chosen, "n2");
    assert_eq!(draft.chosen_summary(), Some(&summary));
    let error = request(draft, Some(&root())).unwrap_err();
    assert!(error.contains("keep after merging"), "{error}");
    assert_eq!(
        summary_label(&summary),
        format!("{} · docs/a.txt", summary.created_at)
    );
}

fn candidate_page(start: usize, end: usize) -> crate::files::history::BranchPage {
    let mut page = branch_page((start..end).map(|index| branch_wire(json!({
        "branch_id":format!("branch-{index:03}"), "name":format!("Branch {index}"), "node_id":"n1"
    }))).collect(), None);
    page.previous_cursor = (start > 0).then(|| format!("previous-{start}"));
    page.next_cursor = (end < 201).then(|| format!("next-{end}"));
    page
}

#[test]
fn merge_selection_spans_201_candidates_without_retaining_candidate_pages() {
    let mut model = Model::default();
    model.history.install(chain(1));
    let mut selection = draft();
    selection.branch_heads.clear();
    selection.input_bytes = 0;
    model.merge = Some(selection);
    model.history.branches = Some(candidate_page(0, 100));
    select_input(&mut model, "branch-000", true).unwrap();
    model.history.branches = Some(candidate_page(100, 200));
    assert_eq!(model.merge.as_ref().unwrap().branch_heads.len(), 1);
    model.history.branches = Some(candidate_page(200, 201));
    select_input(&mut model, "branch-200", true).unwrap();
    assert_eq!(model.history.branch_page().unwrap().branches().len(), 1);
    assert!(choose(&mut model, "n1"));
    model.merge.as_mut().unwrap().survivor = "branch-000".into();
    let body = request(model.merge.as_ref().unwrap(), model.history.root()).unwrap();
    assert_eq!(body["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(body["inputs"][0]["branch_id"], "branch-000");
    assert_eq!(body["inputs"][1]["branch_id"], "branch-200");
    assert_eq!(body["inputs"][1]["expected_node_id"], "n1");
    model.history.branches = Some(candidate_page(0, 100));
    assert_eq!(model.merge.as_ref().unwrap().branch_heads.len(), 2);
    select_input(&mut model, "branch-000", false).unwrap();
    assert!(model.merge.as_ref().unwrap().survivor.is_empty());
    assert_eq!(model.merge.as_ref().unwrap().branch_heads.len(), 1);
}

#[test]
fn merge_addition_is_refused_atomically_at_the_complete_encoded_request_budget() {
    let mut model = Model::default();
    model.history.install(chain(1));
    model.history.branches = Some(candidate_page(0, 100));
    let mut selection = draft();
    selection.input_bytes = MAX_REQUEST_BYTES - input_reserve(&selection);
    model.merge = Some(selection);
    let before = model.merge.as_ref().unwrap().branch_heads.clone();
    let error = select_input(&mut model, "branch-001", true).unwrap_err();
    assert!(error.contains("1 MiB"));
    assert_eq!(model.merge.as_ref().unwrap().branch_heads, before);
    model
        .history
        .mark_stale(crate::files::history::state::StaleReason::RootChanged);
    assert!(
        select_input(&mut model, "branch-002", true)
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(model.merge.as_ref().unwrap().branch_heads, before);
}
