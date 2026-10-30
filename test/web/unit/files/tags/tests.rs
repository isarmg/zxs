use super::*;

pub(crate) fn selection(ids: &[&str]) -> Selection {
    let mut selected=Selection::default();
    for id in ids {
        selected.insert("root-one",SelectedNode {
            id:(*id).into(),version:format!("body-{id}"),tags:BTreeSet::new(),
        }).unwrap();
    }
    selected
}
fn selected_with_tags() -> Selection {
    let mut selected=Selection::default();
    selected.insert("root-one",SelectedNode {
        id:"old-node".into(),version:"old-body".into(),tags:BTreeSet::from(["old".into(),"inherited".into()]),
    }).unwrap();
    selected
}
fn branch(id: &str, expected: &str) -> SelectedBranch {
    SelectedBranch { id:id.into(),name:id.into(),expected:expected.into() }
}
pub(crate) fn draft_with_pending_input(input: &str) -> Draft {
    Draft {
        file:"file".into(),repository:"repo".into(),path:"a.txt".into(),
        root:"root-one".into(),max_entries:5_001,
        targets:BTreeMap::from([("old-node".into(),Some("main".into()))]),
        branches:BTreeMap::from([("main".into(),branch("main","newer-head"))]),
        add:BTreeSet::from(["reviewed".into()]),remove:BTreeSet::from(["old".into()]),
        input:input.into(),target_page:0,delta_page:0,resolved:None,
    }
}

#[test]
fn tag_deltas_preserve_heterogeneous_inherited_labels_and_do_not_mutate_the_parent() {
    let selected=selected_with_tags();
    let node=selected.get("old-node").unwrap();
    let updated=result_tags(node,&BTreeSet::from(["new".into()]),&BTreeSet::from(["old".into()]));
    assert_eq!(updated,BTreeSet::from(["inherited".into(),"new".into()]));
    assert!(node.tags.contains("old"));
    assert!(valid_tag(&format!("{}ab","文".repeat(42))));
    assert!(!valid_tag(&"文".repeat(43)));
    assert!(!valid_tag("bad\nlabel"));
}

#[test]
fn requests_preserve_off_page_sources_and_frozen_branch_heads() {
    let selected=selected_with_tags();
    let draft=draft_with_pending_input("");
    let body=request(&draft,&selected).unwrap();
    assert_eq!(body["targets"][0]["source_node_id"],"old-node");
    assert_eq!(body["targets"][0]["expected_node_id"],"newer-head");
    assert_eq!(body["add"],json!(["reviewed"]));
    assert!(body.get("note").is_none());
    assert_eq!(selected.get("old-node").unwrap().version,"old-body");
    let inventory=draft.review_inventory();
    assert_eq!(inventory.node_ids,["old-node"]);
    assert_eq!(inventory.branch_ids,["main"]);
}

#[test]
fn one_invalid_target_blocks_every_selected_node_instead_of_submitting_a_subset() {
    let mut selected=selected_with_tags();
    selected.insert("root-one",SelectedNode {
        id:"full".into(),version:"body-full".into(),
        tags:(0..storage_contract::MAX_FILE_TAGS).map(|index|format!("tag-{index}")).collect(),
    }).unwrap();
    let mut draft=draft_with_pending_input("");
    draft.targets.insert("full".into(),Some("main".into()));
    assert!(request(&draft,&selected).is_err());
    draft.remove.insert("tag-0".into());
    assert_eq!(request(&draft,&selected).unwrap()["targets"].as_array().unwrap().len(),2);
    selected.remove("full");
    assert!(request(&draft,&selected).is_err(),"an evicted or missing selected identity cannot become a partial action");
}

#[test]
fn unassigned_branches_and_a_lower_current_core_capability_block_the_whole_action() {
    let selected=selected_with_tags();
    let mut draft=draft_with_pending_input("");
    draft.targets.insert("old-node".into(),None);
    assert!(request(&draft,&selected).is_err());
    draft.targets.insert("old-node".into(),Some("main".into()));
    draft.max_entries=0;
    assert!(request(&draft,&selected).is_err());
}

#[test]
fn pending_tag_input_cannot_be_silently_dropped_from_a_valid_delta() {
    let selected=selected_with_tags();
    for input in ["another","  another  ","bad\nlabel",&"a".repeat(129)] {
        let draft=draft_with_pending_input(input);
        assert!(request(&draft,&selected).unwrap_err().contains("Add tag"));
        assert_eq!(draft.input,input);
        assert_eq!(draft.add,BTreeSet::from(["reviewed".into()]));
        assert_eq!(draft.remove,BTreeSet::from(["old".into()]));
    }
    for input in ["","   "] {
        let body=request(&draft_with_pending_input(input),&selected).unwrap();
        assert_eq!(body["add"],json!(["reviewed"]));
        assert_eq!(body["remove"],json!(["old"]));
    }
}

#[test]
fn rejected_submission_preserves_the_draft_and_never_starts_an_operation() {
    Owner::new().with(|| {
        let state=RwSignal::new(State { authenticated:true,..Default::default() });
        state.update(|s| {
            s.management.storage=crate::management::Storage::Ready;
            s.files.writable=true;
            s.files.tag_edit=Some(draft_with_pending_input("  another  "));
            s.files.checked_nodes=selected_with_tags();
        });
        submit(state);
        state.with_untracked(|s| {
            let draft=s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.input,"  another  ");
            assert_eq!(draft.targets["old-node"].as_deref(),Some("main"));
            assert_eq!(draft.branches["main"].expected,"newer-head");
            assert!(s.files.checked_nodes.contains("old-node"));
            assert!(s.files.operations.is_empty());
            assert!(!s.files.mutation_pending);
            assert!(s.files.publications[&review::Kind::Tags].operation_id.is_none());
            assert!(!review::error_text(s,review::Kind::Tags).is_empty());
        });
        add_input(state);
        state.with_untracked(|s| {
            let draft=s.files.tag_edit.as_ref().unwrap();
            assert!(draft.input.is_empty());
            let body=request(draft,&s.files.checked_nodes).unwrap();
            assert_eq!(body["add"],json!(["another","reviewed"]));
            assert_eq!(body["remove"],json!(["old"]));
            assert!(s.files.operations.is_empty());
        });
    });
}

#[test]
fn root_review_keeps_old_labels_and_heads_until_explicit_complete_acceptance() {
    let mut selected=selected_with_tags();
    let mut draft=draft_with_pending_input("");
    let mut new=selected.get("old-node").unwrap().clone();
    new.tags=BTreeSet::from(["rechecked".into()]);
    draft.resolve_review("root-two".into(),&[new],&[branch("main","current-head")],5_001).unwrap();
    assert_eq!(draft.root(),"root-one");
    assert_eq!(draft.branches["main"].expected,"newer-head");
    assert!(selected.get("old-node").unwrap().tags.contains("old"));
    draft.accept_review("root-two",&mut selected).unwrap();
    assert_eq!(draft.root(),"root-two");
    assert_eq!(selected.root(),Some("root-two"));
    assert_eq!(draft.branches["main"].expected,"current-head");
    assert_eq!(selected.get("old-node").unwrap().tags,BTreeSet::from(["rechecked".into()]));
    assert_eq!(draft.add,BTreeSet::from(["reviewed".into()]));
    assert_eq!(draft.remove,BTreeSet::from(["old".into()]));
}

#[test]
fn incomplete_or_superseded_review_preserves_all_existing_inputs() {
    for (root,nodes) in [("root-two",vec![]),("root-three",vec![])] {
        let mut selected=selected_with_tags();
        let before=selected.clone();
        let mut draft=draft_with_pending_input("unfinished");
        draft.resolved=Some(ResolvedReview {
            root:"root-two".into(),nodes,branches:BTreeMap::new(),max_entries:5_001,
        });
        assert!(draft.accept_review(root,&mut selected).is_err());
        assert_eq!(selected,before);
        assert_eq!(draft.root(),"root-one");
        assert_eq!(draft.branches["main"].expected,"newer-head");
        assert_eq!(draft.input,"unfinished");
    }
}

#[test]
fn failed_complete_resolution_cannot_leave_an_old_review_ready_for_acceptance() {
    let mut draft = draft_with_pending_input("unfinished");
    draft.resolved = Some(ResolvedReview {
        root: "root-two".into(), nodes: vec![], branches: BTreeMap::new(), max_entries: 10,
    });
    assert!(draft.resolve_review("root-two".into(), &[], &[], 10).is_err());
    assert!(draft.resolved.is_none());
    assert_eq!(draft.root(), "root-one");
    assert_eq!(draft.branches["main"].expected, "newer-head");
    assert_eq!(draft.input, "unfinished");
}
