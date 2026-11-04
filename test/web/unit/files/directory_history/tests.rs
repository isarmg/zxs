use super::*;
use crate::files::test_entries::{entry, update};
use json_codec::json;

fn id(prefix: &str, value: u8) -> String {
    format!("{prefix}_{value:032x}")
}
fn state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.files.slot = "P01".into();
    state.files.directory = "Repository".into();
    state.files.selected = Some(entry(
        json!({"kind":"directory","entry_id":id("entry",1),"repository_id":id("repo",1),"path":"Repository/folder","repository_path":"folder"}),
    ));
    state
}
fn page(collection: Collection, root: char) -> DirectoryHistoryPage {
    let nodes = if collection == Collection::Nodes {
        json!([{"tree_id":id("dtree",1),"directory_branch_id":id("dbr",1),"parent_tree_id":null,"parent_endpoint":null,"commit_id":id("commit",1),"path":"folder","sequence":"1"}])
    } else {
        json!([])
    };
    let branches = if collection == Collection::Branches {
        json!([{"directory_branch_id":id("dbr",1),"name":"Main","parent_branch_id":null,"head_tree_id":id("dtree",1),"materialized":true,"current":true,"sequence":"1"}])
    } else {
        json!([])
    };
    DirectoryHistoryPage::deserialize(json!({"identity":{"repository_id":id("repo",1),"directory_id":id("entry",1),"history_tree_id":id("history",1),"history_root":root.to_string().repeat(64),"collection":collection,"limit":64},"current":{"directory_branch_id":id("dbr",1),"tree_id":id("dtree",1)},"nodes":nodes,"branches":branches,"page":{"previous_cursor":null,"next_cursor":null}})).unwrap()
}
#[test]
fn readonly_folder_retains_real_stable_scope_and_never_requires_write_admission() {
    let state = state();
    assert!(!state.files.writable);
    let target = Target::capture(&state).unwrap();
    let request = request(&target, &Model::default(), &Read::Refresh).unwrap();
    assert_eq!(request.repository_id(), id("repo", 1));
    assert_eq!(request.directory_id(), id("entry", 1));
    assert_eq!(request.limit(), 64);
    assert!(!endpoint(&request).contains("directory-scopes"));
    page(Collection::Nodes, 'a').validate(&request).unwrap();
}
#[test]
fn repository_root_uses_the_actual_repository_identity_but_backup_aggregates_do_not() {
    let mut state = state();
    state.files.directory.clear();
    state.files.selected = Some(entry(
        json!({"kind":"directory","entry_id":id("repo",2),"path":"Repository"}),
    ));
    let target = Target::capture(&state).unwrap();
    let request = request(&target, &Model::default(), &Read::Refresh).unwrap();
    assert_eq!(request.repository_id(), request.directory_id());
    state.files.selected = Some(entry(
        json!({"kind":"directory","entry_id":"backupdir_hash","path":"Repository","backup":true}),
    ));
    assert!(Target::capture(&state).is_none());
}
#[test]
fn stale_completions_cannot_follow_rename_replacement_or_new_login() {
    let mut state = state();
    let original = state.clone();
    let target = Target::capture(&state).unwrap();
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"path":"Repository/renamed","name":"renamed","repository_path":"renamed"}),
    );
    assert!(!target.current(&state));
    assert_eq!(
        Target::capture(&state).unwrap().directory,
        target.directory,
        "renaming keeps history identity"
    );
    state = original.clone();
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"entry_id":id("entry",2)}),
    );
    assert!(!target.current(&state));
    state = original;
    state.session_epoch += 1;
    assert!(!target.current(&state));
}
#[test]
fn failed_refresh_preserves_graph_and_known_root_conflict_blocks_paging() {
    let target = Target::capture(&state()).unwrap();
    let mut model = Model::default();
    model.install(page(Collection::Nodes, 'a'), Collection::Nodes);
    let before = model.nodes.clone();
    model.failed(api::error(
        storage_contract::ErrorCode::HistoryChanged,
        "root changed",
    ));
    assert_eq!(model.nodes, before);
    assert!(model.stale);
    assert!(
        request(
            &target,
            &model,
            &Read::Nodes {
                cursor: None,
                anchor: Some(id("dtree", 1))
            }
        )
        .is_err()
    );
    assert!(request(&target, &model, &Read::Refresh).is_ok());
}
#[test]
fn new_root_replaces_nodes_and_cannot_mix_previous_branch_names() {
    let mut model = Model::default();
    model.install(page(Collection::Nodes, 'a'), Collection::Nodes);
    model.install(page(Collection::Branches, 'a'), Collection::Branches);
    model.install(page(Collection::Nodes, 'b'), Collection::Nodes);
    assert!(model.branches.is_none());
    assert!(!model.stale);
    model.install(page(Collection::Branches, 'b'), Collection::Branches);
    assert!(model.branches.is_some());
}
#[test]
fn node_links_carry_the_admitted_root_and_true_tree_identity() {
    let target = Target::capture(&state()).unwrap();
    let mut model = Model::default();
    model.install(page(Collection::Nodes, 'a'), Collection::Nodes);
    let request = request(
        &target,
        &model,
        &Read::Nodes {
            cursor: None,
            anchor: Some(id("dtree", 1)),
        },
    )
    .unwrap();
    assert_eq!(request.anchor_tree_id(), Some(id("dtree", 1).as_str()));
    assert_eq!(request.expected_root().unwrap().to_string(), "a".repeat(64));
    assert!(endpoint(&request).contains("/directories/entry_"));
    let source = include_str!("../../../../../src/web/src/files/directory_history/view.rs");
    assert!(!source.contains("FileNode"));
    assert!(!source.contains("正在加载"));
    assert!(source.contains("data-directory-tree-id"));
}
