use super::*;
use crate::files::model::FileOperation;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.tag_edit = Some(Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        root: crate::files::history::HistoryRoot::parse(crate::files::history::tests::ROOT)
            .unwrap(),
        branches: vec![],
        targets: (0..120)
            .map(|index| Target {
                source: format!("node-{index:03}"),
                branch: "main".into(),
                expected: "head".into(),
                tags: BTreeSet::new(),
                label: index.to_string(),
            })
            .collect(),
        add: BTreeSet::from(["draft-tag".into()]),
        remove: BTreeSet::new(),
        input: "unfinished".into(),
        offset: 0,
    });
    state.files.checked_nodes.insert(
        "node".into(),
        CheckedNode {
            root: crate::files::history::HistoryRoot::parse(crate::files::history::tests::ROOT)
                .unwrap(),
            node_id: "node".into(),
            branch: "main".into(),
            expected: "head".into(),
            tags: BTreeSet::new(),
            label: "node".into(),
        },
    );
    state
        .files
        .publications
        .insert(review::Kind::Tags, Default::default());
    state
        .files
        .publications
        .insert(review::Kind::Backup, Default::default());
    state
}

#[test]
fn tag_cancel_remains_available_when_only_storage_or_listing_blocks_writes() {
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
        assert!(state.files.tag_edit.is_none());
        assert!(!state.files.publications.contains_key(&review::Kind::Tags));
        assert!(state.files.publications.contains_key(&review::Kind::Backup));
        assert!(state.files.checked_nodes.contains_key("node"));
    }
}

#[test]
fn the_selected_list_renders_in_local_pages_of_fifty() {
    let state = editing();
    let draft = state.files.tag_edit.as_ref().unwrap();
    assert_eq!(draft.targets.len(), 120);
    let first = visible_targets(draft);
    assert_eq!(first.len(), SELECTED_PAGE);
    assert_eq!(first[0].source, "node-000");
    let mut later = draft.clone();
    later.offset = 100;
    let last = visible_targets(&later);
    assert_eq!(last.len(), 20);
    assert_eq!(last[0].source, "node-100");
}

#[test]
fn removal_options_replace_the_local_page_and_retain_only_explicit_removals() {
    let mut state = editing();
    let draft = state.files.tag_edit.as_mut().unwrap();
    for (index, target) in draft.targets.iter_mut().enumerate() {
        target.tags = (0..storage_contract::MAX_FILE_TAGS)
            .map(|tag| format!("tag-{index:03}-{tag:02}"))
            .collect();
    }
    let first = removal_options(draft);
    assert_eq!(first.len(), SELECTED_PAGE * storage_contract::MAX_FILE_TAGS);
    assert!(first.contains("tag-000-00"));
    assert!(!first.contains("tag-050-00"));
    draft.remove.insert("tag-000-00".into());
    draft.offset = 50;
    let next = removal_options(draft);
    assert_eq!(
        next.len(),
        SELECTED_PAGE * storage_contract::MAX_FILE_TAGS + 1
    );
    assert!(next.contains("tag-000-00"));
    assert!(!next.contains("tag-000-01"));
    assert!(next.contains("tag-050-00"));
    assert!(!next.contains("tag-100-00"));
}

#[test]
fn target_options_use_the_current_candidate_page_and_keep_the_explicit_target_visible() {
    let mut state = editing();
    state
        .files
        .history
        .install(crate::files::history::tests::chain(1));
    let draft = state.files.tag_edit.as_mut().unwrap();
    draft.branches.push(BranchChoice {
        id: "main".into(),
        name: "Selected main".into(),
        head: "head".into(),
    });
    state.files.history.branches = Some(crate::files::history::tests::branch_page(
        vec![crate::files::history::tests::branch_wire(
            json!({"branch_id":"last","name":"Last branch"}),
        )],
        None,
    ));
    let options = branch_options(&state.files, "node-000");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].value, "main");
    assert!(options[0].label.contains("Selected main"));
    assert_eq!(options[1].value, "last");
}

#[test]
fn tag_cancel_preserves_pending_and_unknown_operation_drafts() {
    let mut state = editing();
    state.files.mutation_pending = true;
    cancel(&mut state);
    assert!(state.files.tag_edit.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state);
    assert!(state.files.tag_edit.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Tags));
    state.files.operations.clear();
    cancel(&mut state);
    assert!(state.files.tag_edit.is_none());
}
