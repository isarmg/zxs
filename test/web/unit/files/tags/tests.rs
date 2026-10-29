use super::*;
use crate::files::history::tests::{ROOT, branch_page, branch_wire, chain, node, window};
use crate::files::test_entries::entry;

fn root() -> super::super::history::HistoryRoot {
    super::super::history::HistoryRoot::parse(ROOT).unwrap()
}
fn target(source: &str, tags: &[&str]) -> Target {
    Target {
        source: source.into(),
        branch: "main".into(),
        expected: "head".into(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        label: source.into(),
    }
}
fn branches(head: &str) -> Vec<BranchChoice> {
    vec![BranchChoice {
        id: "main".into(),
        name: "main".into(),
        head: head.into(),
    }]
}
fn draft(targets: Vec<Target>, add: &[&str], remove: &[&str]) -> Draft {
    Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        root: root(),
        branches: branches("head"),
        targets,
        add: add.iter().map(|tag| (*tag).to_owned()).collect(),
        remove: remove.iter().map(|tag| (*tag).to_owned()).collect(),
        input: String::new(),
        offset: 0,
    }
}

#[test]
fn tag_deltas_preserve_heterogeneous_inherited_labels_and_do_not_mutate_the_parent() {
    let tags = BTreeSet::from(["old".to_owned(), "keep".to_owned()]);
    let updated = result_tags(
        &tags,
        &BTreeSet::from(["new".into()]),
        &BTreeSet::from(["old".into()]),
    );
    assert_eq!(updated, BTreeSet::from(["keep".into(), "new".into()]));
    assert_eq!(tags, BTreeSet::from(["old".into(), "keep".into()]));
    assert!(valid_tag(&format!("{}ab", "文".repeat(42))));
    assert!(!valid_tag(&"文".repeat(43)));
    assert!(!valid_tag("bad\nlabel"));
}
#[test]
fn requests_distinguish_historical_source_nodes_from_frozen_branch_heads() {
    let mut draft = draft(vec![target("old-node", &["inherited"])], &["reviewed"], &[]);
    draft.targets[0].expected = "newer-head".into();
    draft.branches = branches("newer-head");
    let body = request(&draft, Some(&root())).unwrap();
    assert_eq!(body["targets"][0]["source_node_id"], "old-node");
    assert_eq!(body["targets"][0]["expected_node_id"], "newer-head");
    assert_eq!(body["add"], json!(["reviewed"]));
    assert!(body.get("note").is_none());
    assert!(request(&draft, None).is_err(), "no current root");
    let other = super::super::history::HistoryRoot::parse(crate::files::history::tests::OTHER_ROOT)
        .unwrap();
    assert!(
        request(&draft, Some(&other))
            .unwrap_err()
            .contains("changed"),
        "a new root keeps the draft but requires re-review"
    );
}

#[test]
fn one_invalid_tag_target_blocks_the_entire_batch() {
    let labels = (0..storage_contract::MAX_FILE_TAGS)
        .map(|index| format!("tag-{index}"))
        .collect::<Vec<_>>();
    let full = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let mut draft = draft(
        vec![target("old", &[]), target("full", &full)],
        &["extra"],
        &[],
    );
    assert!(
        request(&draft, Some(&root())).is_err(),
        "a valid first target must not hide a full second target"
    );
    draft.remove.insert("tag-0".into());
    assert_eq!(
        request(&draft, Some(&root())).unwrap()["targets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    draft.branches.clear();
    assert!(
        request(&draft, Some(&root())).is_err(),
        "targets need a writable branch from the captured page"
    );
    draft.branches = branches("moved");
    assert!(request(&draft, Some(&root())).is_err(), "head moved");
}

fn draft_with_pending_input(input: &str) -> Draft {
    let mut draft = draft(vec![target("source", &["old"])], &["reviewed"], &["old"]);
    draft.input = input.into();
    draft
}

#[test]
fn pending_tag_input_cannot_be_silently_dropped_from_a_valid_delta() {
    for input in ["another", "  another  ", "bad\nlabel", &"a".repeat(129)] {
        let draft = draft_with_pending_input(input);
        let error =
            request(&draft, Some(&root())).expect_err("pending input must block the whole request");
        assert!(error.contains("Add tag"));
        assert_eq!(draft.input, input);
        assert_eq!(draft.add, BTreeSet::from(["reviewed".into()]));
        assert_eq!(draft.remove, BTreeSet::from(["old".into()]));
    }
    for input in ["", "   "] {
        let body = request(&draft_with_pending_input(input), Some(&root())).unwrap();
        assert_eq!(body["add"], json!(["reviewed"]));
        assert_eq!(body["remove"], json!(["old"]));
    }
}

fn checked(id: &str) -> CheckedNode {
    CheckedNode {
        root: root(),
        node_id: id.into(),
        branch: "main".into(),
        expected: "head".into(),
        tags: BTreeSet::new(),
        label: id.into(),
    }
}

#[test]
fn pending_tag_input_rejection_preserves_an_editable_draft_without_starting_an_operation() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.storage = crate::management::Storage::Ready;
            s.files.writable = true;
            s.files.history.install(chain(1));
            s.files.tag_edit = Some(draft_with_pending_input("  another  "));
            s.files
                .checked_nodes
                .insert("source".into(), checked("source"));
        });
        // Validation must stop before review::submit generates an operation ID
        // or schedules a browser request. This path also runs in native tests.
        submit(state);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.input, "  another  ");
            assert_eq!(draft.targets[0].source, "source");
            assert_eq!(draft.targets[0].branch, "main");
            assert_eq!(draft.targets[0].expected, "head");
            assert!(s.files.checked_nodes.contains_key("source"));
            assert!(s.files.operations.is_empty());
            assert!(!s.files.mutation_pending);
            assert!(
                s.files.publications[&review::Kind::Tags]
                    .operation_id
                    .is_none()
            );
            assert!(!review::error_text(s, review::Kind::Tags).is_empty());
            assert!(actions::write_allowed(s));
        });

        // The user can explicitly add the pending tag and keep the prior delta.
        add_input(state);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert!(draft.input.is_empty());
            let body = request(draft, s.files.history.root()).unwrap();
            assert_eq!(body["add"], json!(["another", "reviewed"]));
            assert_eq!(body["remove"], json!(["old"]));
            assert!(s.files.operations.is_empty());
        });
    });
}

fn checked_model() -> Model {
    let mut model = Model {
        selected: Some(entry(
            json!({"file_id":"file","selected_branch_id":"branch"}),
        )),
        ..Default::default()
    };
    model.history.install(chain(3));
    model.history.branches = Some(branch_page(
        vec![branch_wire(
            json!({"branch_id":"branch","file_node_id":"n3","node_id":"n3","version_id":"n3","own_version_id":"n3","current":true}),
        )],
        Some("branch"),
    ));
    model
}

#[test]
fn checked_nodes_record_their_target_from_the_resident_page_and_survive_paging() {
    let mut model = checked_model();
    assert_eq!(check(&mut model, "n2", true), Check::Changed);
    assert_eq!(check(&mut model, "n2", true), Check::Ignored);
    assert_eq!(check(&mut model, "missing", true), Check::NotResident);
    let recorded = &model.checked_nodes["n2"];
    assert_eq!(recorded.branch, "branch");
    assert_eq!(recorded.expected, "n3");
    assert!(recorded.label.starts_with("2 · "));
    assert!(model.checked_bytes > 0);
    let bytes = model.checked_bytes;
    // Replace the page with one that no longer contains n2.
    model.history.install(window(
        vec![node("n9", 9, &[], &["n9"])],
        json!({"previous_cursor":"aa"}),
    ));
    assert!(
        model.checked_nodes.contains_key("n2"),
        "paging never drops a selection"
    );
    assert_eq!(check(&mut model, "n9", true), Check::Changed);
    assert!(model.checked_bytes > bytes);
    assert_eq!(check(&mut model, "n9", false), Check::Changed);
    assert_eq!(
        model.checked_bytes, bytes,
        "removing restores the exact accounting"
    );
    assert_eq!(check(&mut model, "n2", false), Check::Changed);
    assert_eq!(model.checked_bytes, 0);
    assert_eq!(check(&mut model, "n2", false), Check::Ignored);
    model.tag_edit = Some(draft(vec![], &[], &[]));
    assert_eq!(
        check(&mut model, "n9", false),
        Check::Ignored,
        "an open editor freezes the set"
    );
}

#[test]
fn starting_a_draft_uses_the_recorded_targets_including_non_resident_ones() {
    Owner::new().with(|| {
        let mut initial = State {
            authenticated: true,
            ..Default::default()
        };
        initial.management.storage = crate::management::Storage::Ready;
        initial.files = checked_model();
        initial.files.writable = true;
        initial.files.selected = Some(entry(json!({
            "file_id":"file","selected_branch_id":"branch","repository_id":"repo","repository_path":"docs/a.txt",
            "path":"docs/a.txt","name":"a.txt"
        })));
        let state = RwSignal::new(initial);
        assert_eq!(
            state.try_update(|s| check(&mut s.files, "n2", true)),
            Some(Check::Changed)
        );
        state.update(|s| {
            s.files.history.install(window(vec![node("n9", 9, &[], &["n9"])], json!({"previous_cursor":"aa"})));
        });
        start(state);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.targets.len(), 1);
            assert_eq!(draft.targets[0].source, "n2");
            assert_eq!(draft.targets[0].expected, "n3");
            assert_eq!(draft.branches.len(), 1, "only the checked target's compact branch/head survives");
            assert_eq!(draft.branches[0].head, "n3");
        });
        // A stale root refuses to open a draft and says why.
        state.update(|s| {
            s.files.tag_edit = None;
            s.files.history.mark_stale(crate::files::history::state::StaleReason::RootChanged);
        });
        start(state);
        state.with_untracked(|s| {
            assert!(s.files.tag_edit.is_none());
            assert!(s.notice.contains("changed"));
            assert!(s.files.checked_nodes.contains_key("n2"), "selection retained for re-review");
        });
    });
}

#[test]
fn the_request_budget_refuses_an_addition_that_would_exceed_one_mebibyte() {
    let mut model = checked_model();
    let reserve = request_reserve();
    assert!(reserve < 16 * 1024, "the fixed reserve is small: {reserve}");
    let per_target = target_bytes(&target("n2", &[]));
    // Pretend earlier pages already filled the budget up to the last slot.
    model.checked_bytes = MAX_REQUEST_BYTES - reserve - per_target;
    assert_eq!(check(&mut model, "n2", true), Check::Changed);
    assert_eq!(
        check(&mut model, "n1", true),
        Check::BudgetExceeded,
        "the next addition is refused, nothing is dropped or split"
    );
    assert!(model.checked_nodes.contains_key("n2"));
    assert!(!model.checked_nodes.contains_key("n1"));
    assert_eq!(check(&mut model, "n2", false), Check::Changed);
    assert_eq!(check(&mut model, "n1", true), Check::Changed);
    // The final request check measures the complete encoded body.
    let many = (0..20_000)
        .map(|i| target(&format!("node-{i:05}-{}", "x".repeat(40)), &[]))
        .collect::<Vec<_>>();
    let error = request(&draft(many, &["t"], &[]), Some(&root())).unwrap_err();
    assert!(error.contains("1 MiB"), "{error}");
}

#[test]
fn edit_node_replaces_the_selection_with_one_resident_node() {
    Owner::new().with(|| {
        let mut initial = State {
            authenticated: true,
            ..Default::default()
        };
        initial.files = checked_model();
        let state = RwSignal::new(initial);
        assert_eq!(
            state.try_update(|s| check(&mut s.files, "n1", true)),
            Some(Check::Changed)
        );
        edit_node(state, "n3".into());
        state.with_untracked(|s| {
            assert_eq!(s.files.checked_nodes.keys().collect::<Vec<_>>(), ["n3"]);
            // write_allowed is false (storage unknown), so no draft opens.
            assert!(s.files.tag_edit.is_none());
        });
    });
}

#[test]
fn archived_migrated_and_merged_branch_targets_cannot_be_submitted() {
    // Only writable branches are captured into the draft at start(); a target
    // whose branch is absent from the captured set is rejected.
    let mut draft = draft_with_pending_input("");
    draft.branches.clear();
    assert!(request(&draft, Some(&root())).is_err());
}

#[test]
fn tag_branch_choices_reach_the_last_of_201_candidates_and_keep_only_selected_heads() {
    let mut model = checked_model();
    model.tag_edit = Some(draft(vec![target("n2", &[])], &["reviewed"], &[]));
    for (start, end) in [(0, 100), (100, 200), (200, 201)] {
        model.history.branches = Some(branch_page((start..end).map(|index| branch_wire(json!({
            "branch_id":format!("branch-{index:03}"),"name":format!("Branch {index}"),"node_id":"n3"
        }))).collect(), None));
        assert!(model.history.branch_page().unwrap().branches().len() <= 100);
    }
    choose_branch(&mut model, "n2", "branch-200").unwrap();
    assert_eq!(model.tag_edit.as_ref().unwrap().branches.len(), 1);
    let body = request(model.tag_edit.as_ref().unwrap(), model.history.root()).unwrap();
    assert_eq!(body["targets"][0]["branch_id"], "branch-200");
    assert_eq!(body["targets"][0]["expected_node_id"], "n3");
    model.history.branches = Some(branch_page(
        vec![branch_wire(
            json!({"branch_id":"branch-000","node_id":"n1"}),
        )],
        None,
    ));
    assert_eq!(
        request(model.tag_edit.as_ref().unwrap(), model.history.root()).unwrap(),
        body
    );
    choose_branch(&mut model, "n2", "branch-000").unwrap();
    let draft = model.tag_edit.as_ref().unwrap();
    assert_eq!(
        draft.branches.len(),
        1,
        "the replaced explicit branch is released"
    );
    assert_eq!(draft.branches[0].id, "branch-000");
    assert!(
        choose_branch(&mut model, "n2", "branch-200").is_err(),
        "a queued old option is no longer a current candidate"
    );
}

#[test]
fn a_new_draft_cannot_rebase_checked_nodes_captured_under_an_old_root() {
    Owner::new().with(|| {
        let mut initial = State { authenticated:true, ..Default::default() };
        initial.management.storage = crate::management::Storage::Ready;
        initial.files = checked_model();
        initial.files.writable = true;
        initial.files.selected = Some(entry(json!({"file_id":"file","repository_id":"repo","repository_path":"docs/a.txt","path":"docs/a.txt","name":"a.txt"})));
        assert_eq!(check(&mut initial.files, "n2", true), Check::Changed);
        let mut wire = crate::files::history::tests::window_wire(vec![node("n3",3,&[],&["n3"])], json!({}));
        wire["history_root"] = crate::files::history::tests::OTHER_ROOT.into();
        initial.files.history.install(super::super::history::HistoryWindow::parse(wire,"file").unwrap());
        let state = RwSignal::new(initial);
        start(state);
        state.with_untracked(|s| {
            assert!(s.files.tag_edit.is_none());
            assert!(s.files.checked_nodes.contains_key("n2"));
            assert!(s.notice.contains("Clear the old selection"));
        });
    });
}

#[test]
fn tag_delta_edits_refuse_the_seventeenth_member_without_changing_either_set() {
    let mut draft = draft(vec![target("node", &[])], &[], &[]);
    for index in 0..storage_contract::MAX_FILE_TAGS {
        add_tag(&mut draft, format!("tag-{index:02}")).unwrap();
    }
    draft.remove.insert("seventeenth".into());
    let before = (draft.add.clone(), draft.remove.clone());
    assert!(
        add_tag(&mut draft, "seventeenth".into())
            .unwrap_err()
            .contains("16")
    );
    assert_eq!((draft.add.clone(), draft.remove.clone()), before);
    draft.remove.clear();
    for index in 0..storage_contract::MAX_FILE_TAGS {
        toggle_removal(&mut draft, &format!("remove-{index:02}")).unwrap();
    }
    let before = (draft.add.clone(), draft.remove.clone());
    assert!(
        toggle_removal(&mut draft, "tag-00")
            .unwrap_err()
            .contains("16")
    );
    assert_eq!((draft.add.clone(), draft.remove.clone()), before);
    toggle_removal(&mut draft, "remove-00").unwrap();
    toggle_removal(&mut draft, "tag-00").unwrap();
    assert!(!draft.add.contains("tag-00"));
    assert!(draft.remove.contains("tag-00"));
    draft.remove.insert("extra".into());
    assert!(request(&draft, Some(&root())).unwrap_err().contains("16"));
}
