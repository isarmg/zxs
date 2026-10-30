use super::*;

fn branch(id: &str, head: &str) -> SelectedBranch {
    SelectedBranch { id:id.into(), name:id.into(), expected:head.into() }
}

pub(crate) fn draft() -> Draft {
    Draft {
        file:"file".into(),repository:"repo".into(),path:"a.txt".into(),
        root:"root-one".into(),max_entries:5_001,
        selected:[branch("left","fnode-left"),branch("right","fnode-right")]
            .into_iter().map(|branch|(branch.id.clone(),branch)).collect(),
        chosen:Some(ChosenNode { id:"fnode-unrelated".into(),version:"body-old".into() }),
        survivor:String::new(),selected_page:0,anchor:String::new(),resolved:None,
    }
}

#[test]
fn navigation_discards_an_open_merge_review() {
    use crate::files::model::Navigation;
    for destination in [Navigation::Root,Navigation::Disk("B01".into()),Navigation::Directory("other".into())] {
        let mut model=Model { merge:Some(draft()),..Default::default() };
        model.navigate(destination);
        assert!(model.merge.is_none());
    }
}

#[test]
fn merge_keeps_an_arbitrary_off_page_node_and_every_selected_branch() {
    let mut draft=draft();
    assert!(request(&draft).is_err(),"the survivor must be explicit");
    draft.survivor="left".into();
    let body=request(&draft).unwrap();
    assert_eq!(body["chosen_node_id"],"fnode-unrelated");
    assert_eq!(body["inputs"][0]["expected_node_id"],"fnode-left");
    assert_eq!(body["inputs"][1]["expected_node_id"],"fnode-right");
    assert_eq!(body["surviving_branch_id"],"left");
    assert_eq!(body["confirmed"],true);
    assert!(body.get("content").is_none());
    // A draft contains no candidate page to replace or pin. Its compact result
    // and frozen heads remain sufficient after the browser evicts those pages.
    assert_eq!(draft.chosen.as_ref().unwrap().version,"body-old");
    let inventory=draft.review_inventory();
    assert_eq!(inventory.node_ids,["fnode-unrelated"]);
    assert_eq!(inventory.branch_ids,["left","right"]);
}

#[test]
fn entry_limit_rejects_the_new_selection_without_dropping_previous_inputs() {
    let mut draft=draft();
    draft.max_entries=2;
    let before=draft.selected.clone();
    let chosen=draft.chosen.clone();
    assert!(draft.insert_branch(branch("third","head-third")).is_err());
    assert_eq!(draft.selected,before);
    assert_eq!(draft.chosen,chosen);
    assert!(draft.survivor.is_empty());
}

#[test]
fn root_review_stages_new_heads_until_explicit_acceptance() {
    let mut draft=draft();
    draft.survivor="left".into();
    let updated=[branch("left","new-left"),branch("right","new-right")]
        .into_iter().map(|branch|(branch.id.clone(),branch)).collect::<BTreeMap<_,_>>();
    let chosen=draft.chosen.iter().cloned().collect::<Vec<_>>();
    draft.resolve_review("root-two".into(),&chosen,&updated.values().cloned().collect::<Vec<_>>(),9).unwrap();
    assert_eq!(request(&draft).unwrap()["inputs"][0]["expected_node_id"],"fnode-left");
    assert_eq!(draft.root(),"root-one");
    draft.accept_review("root-two").unwrap();
    assert_eq!(draft.root(),"root-two");
    assert_eq!(draft.selected,updated);
    assert_eq!(draft.survivor,"left");
    assert_eq!(draft.chosen.as_ref().unwrap().id,"fnode-unrelated");
}

#[test]
fn a_further_root_change_rejects_a_prepared_review_and_preserves_inputs() {
    let mut draft=draft();
    let before=draft.selected.clone();
    draft.resolved=Some(ResolvedReview {
        root:"root-two".into(),selected:BTreeMap::new(),chosen:None,max_entries:9,
    });
    assert!(draft.accept_review("root-three").is_err());
    assert_eq!(draft.root(),"root-one");
    assert_eq!(draft.selected,before);
    assert!(draft.chosen.is_some());
}

#[test]
fn whole_request_validation_keeps_the_same_core_and_json_limits() {
    let mut draft=draft();
    draft.survivor="left".into();
    draft.max_entries=1;
    assert!(request(&draft).is_err());
    draft.max_entries=5_001;
    draft.chosen=None;
    assert!(request(&draft).is_err());
    draft.chosen=Some(ChosenNode{id:"other".into(),version:"body".into()});
    draft.survivor="unselected".into();
    assert!(request(&draft).is_err());
}

#[test]
fn an_incomplete_resolve_discards_old_prepared_review_without_replacing_heads() {
    let mut draft = draft();
    let selected = draft.selected.clone();
    draft.resolved = Some(ResolvedReview {
        root: "root-two".into(), selected: BTreeMap::new(), chosen: None, max_entries: 10,
    });
    assert!(draft.resolve_review("root-two".into(), &[], &[], 10).is_err());
    assert!(draft.resolved.is_none());
    assert_eq!(draft.selected, selected);
    assert_eq!(draft.root(), "root-one");
}
