use super::*;
use crate::files::history::MergePage;
use crate::files::history::tests::{
    ROOT, binding, branch_page, branch_wire, chain, merge_page_wire, merge_wire,
};
use crate::files::model::FileOperation;
use web_interface::components::SelectOption;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.merge = Some(Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        root: HistoryRoot::parse(ROOT).unwrap(),
        branch_heads: BTreeMap::new(),
        input_bytes: 0,
        offset: 0,
        chosen: "node".into(),
        chosen_summary: None,
        survivor: "branch".into(),
    });
    state
        .files
        .publications
        .insert(review::Kind::Merge, Default::default());
    state
        .files
        .publications
        .insert(review::Kind::Backup, Default::default());
    state
}

#[test]
fn merge_cancel_remains_available_when_only_storage_or_listing_blocks_writes() {
    for storage in [
        crate::management::Storage::ReadOnly,
        crate::management::Storage::Unavailable,
        crate::management::Storage::Ready,
    ] {
        let mut state = editing();
        state.management.storage = storage;
        state.files.loading = storage == crate::management::Storage::Ready;
        assert!(!actions::write_allowed(&state));
        assert!(!actions::pending(&state));
        cancel(&mut state);
        assert!(state.files.merge.is_none());
        assert!(!state.files.publications.contains_key(&review::Kind::Merge));
        assert!(state.files.publications.contains_key(&review::Kind::Backup));
    }
}

#[test]
fn merge_cancel_preserves_pending_and_unknown_operation_drafts() {
    let mut state = editing();
    state.files.mutation_pending = true;
    cancel(&mut state);
    assert!(state.files.merge.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state);
    assert!(state.files.merge.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Merge));
    state.files.operations.clear();
    cancel(&mut state);
    assert!(state.files.merge.is_none());
}

#[test]
fn result_options_list_the_resident_page_and_keep_a_non_resident_choice_visible() {
    let mut state = editing();
    assert!(result_options(&state.files).is_empty());
    state.files.history.install(chain(2));
    let values = |options: Vec<SelectOption>| {
        options
            .into_iter()
            .map(|option| option.value)
            .collect::<Vec<_>>()
    };
    assert_eq!(values(result_options(&state.files)), ["n2", "n1"]);
    assert!(choose(&mut state.files, "n1"));
    state.files.history.install(chain(1));
    assert_eq!(values(result_options(&state.files)), ["n1"]);
    state
        .files
        .history
        .install(crate::files::history::tests::window(
            vec![crate::files::history::tests::node("n7", 7, &[], &["n7"])],
            json!({"previous_cursor":"aa"}),
        ));
    let options = result_options(&state.files);
    assert_eq!(values(options.clone()), ["n1", "n7"]);
    assert!(options[0].label.contains("Chosen"));
}

#[test]
fn provenance_rows_read_the_merge_page_and_fall_back_for_non_resident_labels() {
    let mut state = editing();
    assert!(rows(&state).is_empty());
    state.files.history.install(chain(2));
    state.files.history.branches = Some(branch_page(
        vec![branch_wire(json!({"branch_id":"branch","name":"kept"}))],
        None,
    ));
    state.files.history.merges = Some(
        MergePage::parse(
            merge_page_wire(vec![
                merge_wire(json!({"merge_id":"m2","sequence":"9","chosen_node_id":"n2","result_node_id":"n2"})),
                merge_wire(json!({"merge_id":"m1","sequence":"8","chosen_node_id":"gone","result_node_id":"n1","reused_node":false,"surviving_branch_id":"other","input_count":3})),
            ]),
            &binding(),
        )
        .unwrap(),
    );
    let rows = rows(&state);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].survivor, "kept");
    assert!(rows[0].chosen.starts_with("1 · "));
    assert_eq!(rows[1].survivor, "Historical branch");
    assert_eq!(rows[1].chosen, "Historical node");
    assert_eq!(rows[1].input_count, 3);
    assert!(!rows[1].reused);
    state
        .files
        .history
        .mark_stale(crate::files::history::state::StaleReason::RootChanged);
    assert!(
        super::rows(&state).is_empty(),
        "a stale root drops the provenance page"
    );
}

#[test]
fn candidate_and_selected_windows_bound_checkbox_and_survivor_options() {
    let mut state = editing();
    state.files.history.install(chain(1));
    state.files.history.branches = Some(branch_page(
        (0..100)
            .map(|index| {
                branch_wire(json!({
                    "branch_id":format!("candidate-{index:03}"),"name":format!("Candidate {index}")
                }))
            })
            .collect(),
        None,
    ));
    let draft = state.files.merge.as_mut().unwrap();
    draft.branch_heads = (0..201)
        .map(|index| {
            let id = format!("selected-{index:03}");
            (
                id.clone(),
                InputBranch {
                    id,
                    name: format!("Selected {index}"),
                    head: Some("head".into()),
                },
            )
        })
        .collect();
    draft.survivor = "selected-000".into();
    draft.offset = 200;
    assert_eq!(branch_choices(&state.files).len(), 101);
    let survivors = survivor_options(&state.files);
    assert_eq!(survivors.len(), 2);
    assert_eq!(survivors[0].value, "selected-000");
    assert_eq!(survivors[1].value, "selected-200");
    state.files.merge.as_mut().unwrap().offset = 0;
    assert_eq!(branch_choices(&state.files).len(), 150);
    assert_eq!(survivor_options(&state.files).len(), 50);
}
