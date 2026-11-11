use super::*;
use data_serialization::Deserialize;
use json_codec::json;
use storage_contract::directory_history::DirectoryHistoryPage;

fn id(prefix: &str, value: u8) -> String {
    format!("{prefix}_{value:032x}")
}

fn reviewed() -> (State, Target, Model, DirectoryBranchCreateSource) {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.loaded = true;
    state.files.writable = true;
    state.files.directory_branches_supported = true;
    state.files.slot = "P01".into();
    state.files.directory = "Repository/folder".into();
    state.files.repository_id = id("repo", 1);
    state.files.repository_path = "folder".into();
    state.files.namespace_head = id("commit", 9);
    state.files.generation = 3;
    let target = Target {
        epoch: 7,
        repository: id("repo", 1),
        directory: id("entry", 1),
        slot: "P01".into(),
        path: "Repository/folder".into(),
        preview: false,
        scope: Scope::CurrentDirectory {
            initialized: true,
            generation: 3,
            repository_path: "folder".into(),
            namespace_head: id("commit", 9),
        },
    };
    let nodes = DirectoryHistoryPage::deserialize(json!({
        "identity": {"repository_id": id("repo",1), "directory_id": id("entry",1),
            "history_tree_id": id("history",1), "history_root": "a".repeat(64),
            "collection": "nodes", "limit": 64},
        "current": {"committed_tree_id": id("dtree",9), "staged": false,
            "directory_branch_id": id("dbr",9), "tree_id": id("dtree",10)},
        "nodes": [{"message": "Old commit", "tree_id": id("dtree",1),
            "directory_branch_id": id("dbr",1), "parent_tree_id": null,
            "parent_endpoint": null, "commit_id": id("commit",1),
            "path": "folder", "sequence": "1"}],
        "branches": [], "page": {"previous_cursor": null, "next_cursor": null}
    })).unwrap();
    let model = Model { nodes: Some(nodes), ..Default::default() };
    let source = DirectoryBranchCreateSource {
        repository_id: id("repo", 1),
        directory_id: id("entry", 1),
        directory_branch_id: id("dbr", 9),
        working_tree_id: id("dtree", 10),
        head_tree_id: id("dtree", 9),
        index_generation: 11,
        namespace_head: id("commit", 9),
    };
    (state, target, model, source)
}

#[test]
fn historical_branch_admits_loaded_commit_independently_of_current_head() {
    let (state, target, model, source) = reviewed();
    let tree = id("dtree", 1).parse().unwrap();
    assert!(available(&state, &target, &model, &source, tree));
    assert_ne!(tree.to_string(), source.head_tree_id);
    assert_ne!(tree.to_string(), source.working_tree_id);
    assert!(!available(&state, &target, &model, &source, id("dtree", 9).parse().unwrap()));
    for change in 0..6 {
        let mut model = model.clone();
        match change {
            0 => model.loading = true,
            1 => model.stale = true,
            2 => model.error = Some(crate::api::error(
                storage_contract::ErrorCode::HistoryChanged, "Changed history")),
            3 => model.nodes = None,
            4 => model.nodes.as_mut().unwrap().nodes.clear(),
            5 => model.nodes.as_mut().unwrap().nodes[0].tree_id = id("dtree", 2),
            _ => unreachable!(),
        }
        assert!(!available(&state, &target, &model, &source, tree), "{change}");
    }
    for change in 0..2 {
        let mut source = source.clone();
        if change == 0 { source.repository_id = id("repo", 2); }
        else { source.directory_id = id("entry", 2); }
        assert!(!available(&state, &target, &model, &source, tree), "{change}");
    }
}

#[test]
fn historical_branch_never_enables_from_readonly_stale_or_replaced_folder() {
    let (state, target, model, source) = reviewed();
    let tree = id("dtree", 1).parse().unwrap();
    for change in 0..13 {
        let mut next = state.clone();
        match change {
            0 => next.authenticated = false,
            1 => next.session_epoch += 1,
            2 => next.files.generation += 1,
            3 => next.files.slot = "P02".into(),
            4 => next.files.directory = "Repository/other".into(),
            5 => next.files.repository_id = id("repo", 2),
            6 => next.files.repository_path = "other".into(),
            7 => next.files.namespace_head = id("commit", 10),
            8 => next.files.writable = false,
            9 => next.files.directory_branches_supported = false,
            10 => next.files.loading = true,
            11 => next.files.loaded = false,
            12 => next.files.mutation_pending = true,
            _ => unreachable!(),
        }
        assert!(!available(&next, &target, &model, &source, tree), "{change}");
    }
    let mut selected = target.clone();
    selected.scope = Scope::Selected;
    assert!(!available(&state, &selected, &model, &source, tree));
    let mut uninitialized = target;
    if let Scope::CurrentDirectory { initialized, .. } = &mut uninitialized.scope {
        *initialized = false;
    }
    assert!(!available(&state, &uninitialized, &model, &source, tree));
}
