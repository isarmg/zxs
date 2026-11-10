use super::*;
use crate::files::test_entries::{branch, entry};
use json_codec::json;

fn id(prefix: &str, n: u32) -> String {
    format!("{prefix}_{n:032x}")
}
fn ready() -> State {
    let folder = entry(json!({
        "kind":"directory", "entry_id":id("entry",2),
        "repository_id":id("repo",1), "path":"empty"
    }));
    let mut state = State {
        authenticated: true,
        principal: "alice".into(),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.slot = "P01".into();
    state.files.writable = true;
    state.files.loaded = true;
    state.files.repository_id = id("repo", 1);
    state.files.namespace_head = id("commit", 1);
    state.files.directory_branches_supported = true;
    state.files.directory_branches = vec![branch(json!({
        "directory_branch_id":id("dbr",1), "repository_id":id("repo",1),
        "directory_id":id("repo",1), "head_tree_id":id("dtree",1),
        "commit_id":id("commit",1), "path":"", "current":true
    }))];
    state.files.entries = vec![folder.clone()].into();
    state.files.selected = Some(folder);
    state
}
fn ordinary_status() -> Value {
    json!({
        "repository_id":id("repo",1), "directory_id":id("entry",2), "path":"empty",
        "initialized":false, "owner":{"directory_id":id("repo",1),"path":""},
        "directory_branch_id":null, "working_tree_id":null, "head_tree_id":null,
        "index_generation":"0", "staged":false, "unstaged":false, "needs_restage":false,
        "added":"0", "modified":"0", "deleted":"0", "children":[], "legacy_history":false
    })
}
fn directory_request() -> Request {
    Request::new(
        "P01".into(),
        HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
        id("repo", 1),
        id("dbr", 1),
        id("dtree", 1),
        id("entry", 2),
        name_policy::PortablePath::parse("empty").unwrap(),
    )
    .unwrap()
}
fn deletion_node() -> DirectoryDeletionNode {
    let operation: storage_contract::OperationId = id("op", 7).parse().unwrap();
    let hash = storage_contract::object_hash(
        "directory-deletion-identity",
        &json!({"operation_id":operation,"prefix":"ddel","owner":id("entry",2)}),
    )
    .unwrap();
    json_codec::from_value(json!({
        "node_id":format!("ddel_{}",&hash[..32]), "repository_id":id("repo",1),
        "history_tree_id":id("history",1), "owner_directory_id":id("repo",1),
        "directory_id":id("entry",2), "path":"empty", "namespace_head":id("commit",1),
        "directory_branch_id":id("dbr",1), "source_tree_id":id("dtree",9),
        "operation_id":operation, "author":"alice", "sequence":"1", "created_at":"123",
        "files":{}
    }))
    .unwrap()
}
fn summary(node: &DirectoryDeletionNode) -> DirectoryDeletionSummary {
    DirectoryDeletionSummary {
        directory_id: node.directory_id.clone(),
        deletion_node_id: node.node_id.clone(),
        path: node.path.clone(),
        operation_id: node.operation_id,
    }
}
#[test]
fn directory_capture_is_one_current_child_and_preserves_listing_cas_tree() {
    let state = ready();
    let selected = capture(&state, None).unwrap().unwrap();
    assert_eq!(selected.entry, id("entry", 2));
    assert_eq!(selected.tree, id("dtree", 1));
    assert!(selected.current(&state));
    assert!(available(&state));
    let request = directory_request();
    let (endpoint, body) = request.request(&id("op", 7));
    assert_eq!(endpoint, "/api/file-manager/disks/P01/namespace-batch");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["items"][0]["source"]["expected_tree_id"],
        id("dtree", 1)
    );
    assert_eq!(body["items"][0]["source"]["entry_id"], id("entry", 2));
    assert_eq!(body["kind"], "delete");
    assert!(body["items"][0]["target"].is_null());
}
#[test]
fn directory_capture_rejects_multiple_uninitialized_stale_and_read_only_choices() {
    let mut state = ready();
    state.files.checked_entries = ["one".into(), "two".into()].into();
    assert!(!available(&state));
    state.files.checked_entries.clear();
    state.files.directory_branches_supported = false;
    assert!(!available(&state));
    state.files.directory_branches_supported = true;
    state.files.writable = false;
    assert!(!available(&state));
    state.files.writable = true;
    let stale = state.files.selected.clone().unwrap();
    state.files.selected = None;
    state.files.entries = Vec::new().into();
    assert!(!entry_available(&state, &stale));
    state = ready();
    state
        .files
        .directory_branches
        .push(state.files.directory_branches[0].clone());
    assert!(!available(&state));
}
#[test]
fn directory_review_stales_on_refresh_selection_head_and_same_name_replacement() {
    let mut state = ready();
    let selected = capture(&state, None).unwrap().unwrap();
    state.files.generation += 1;
    assert!(!selected.current(&state));
    state = ready();
    state.files.namespace_head = id("commit", 2);
    assert!(!selected.current(&state));
    state = ready();
    state
        .files
        .checked_entries
        .insert(entry_key(state.files.selected.as_ref().unwrap()));
    assert!(!selected.current(&state));
    state = ready();
    let replacement = entry(json!({
        "kind":"directory","entry_id":id("entry",3),
        "repository_id":id("repo",1),"path":"empty"
    }));
    state.files.entries = vec![replacement.clone()].into();
    state.files.selected = Some(replacement);
    assert!(!selected.current(&state));
}
#[test]
fn directory_status_requires_named_exact_owner_and_noninitialized_shape() {
    let repo = id("repo", 1);
    assert!(status(ordinary_status(), &repo, "empty").is_ok());
    for patch in [
        json!({"directory_id":id("repo",1)}),
        json!({"owner":{"directory_id":id("entry",2),"path":"empty"}}),
        json!({"owner":{"directory_id":id("entry",3),"path":"elsewhere"}}),
        json!({"working_tree_id":id("dtree",1)}),
        json!({"added":"1"}),
        json!({"children":[[]]}),
    ] {
        let mut value = ordinary_status();
        for (key, child) in patch.as_object().unwrap() {
            value[key] = child.clone();
        }
        assert!(status(value, &repo, "empty").is_err());
    }
    assert!(status(json!([]), &repo, "empty").is_err());
    assert!(status(ordinary_status(), &id("repo", 2), "empty").is_err());
    assert!(status(ordinary_status(), &repo, "renamed").is_err());
}
#[test]
fn stale_directory_read_releases_only_its_own_loading_flag() {
    Owner::new().with(|| {
        let mut initial = ready();
        let location = Location::capture(&initial).unwrap();
        initial.files.deletion.directory.open = true;
        initial.files.deletion.directory.generation = 1;
        initial.files.deletion.directory.loading = true;
        initial.files.generation += 1;
        let state = RwSignal::new(initial);
        assert!(!accepts_read(state, &location, 1));
        assert!(!state.with_untracked(|s| s.files.deletion.directory.loading));
        state.update(|s| {
            s.files.deletion.directory.generation = 2;
            s.files.deletion.directory.loading = true;
        });
        assert!(!accepts_read(state, &location, 1));
        assert!(state.with_untracked(|s| s.files.deletion.directory.loading));
    });
}
#[test]
fn closing_directory_discovery_clears_account_data_without_settling_original_request() {
    let mut state = ready();
    let location = Location::capture(&state).unwrap();
    let node = Arc::new(deletion_node());
    state.files.deletion.directory.open = true;
    state.files.deletion.directory.scope = Some(Scope {
        location,
        owner: id("repo", 1),
        owner_path: String::new(),
    });
    state.files.deletion.directory.node = Some(node.clone());
    assert!(restore_button_current(&state, &node));
    assert!(!restore_button_current(
        &state,
        &Arc::new(node.as_ref().clone())
    ));
    state
        .files
        .operations
        .push(crate::files::FileOperation::unknown(&id("op", 7)));
    state.files.deletion.close();
    assert!(state.files.deletion.directory.scope.is_none());
    assert!(state.files.deletion.directory.node.is_none());
    assert!(!state.files.deletion.directory.open);
    assert!(actions::pending(&state));
    assert_eq!(state.files.operations[0].id(), id("op", 7));
}
#[test]
fn directory_original_request_roundtrips_without_new_identity_or_today_source() {
    let request = directory_request();
    let bytes = storage_contract::canonical_json(&request.stored()).unwrap();
    let restored = storage_contract::StrictJson::parse(&bytes)
        .unwrap()
        .decode::<Stored>()
        .unwrap()
        .admit()
        .unwrap();
    assert_eq!(request, restored);
    assert_eq!(
        request.request(&id("op", 7)),
        restored.request(&id("op", 7))
    );
    assert_eq!(request.input(), restored.input());
}
#[test]
fn empty_directory_receipt_matches_full_input_without_equating_selected_and_cas_tree() {
    let request = directory_request();
    let node = deletion_node();
    assert_ne!(node.source_tree_id, request.tree());
    let receipt = json!({
        "operation_id":id("op",7),"operation_kind":"directory.delete",
        "protection_at_publish":"NOT_APPLICABLE",
        "publication_sequence":"1",
        "result":{"input":request.input(),"directory_deletion":node,"deletion_nodes":[],
            "timestamp":"123","membership_epoch":"1","version_protection":[]}
    });
    assert!(request.receipt_matches(&id("op", 7), "alice", &receipt));
    let mut wrong = receipt.clone();
    wrong["result"]["input"]["request"]["items"][0]["source"]["expected_tree_id"] =
        id("dtree", 2).into();
    assert!(!request.receipt_matches(&id("op", 7), "alice", &wrong));
    let mut wrong = receipt.clone();
    wrong["result"]["deletion_nodes"] = json!([{}]);
    assert!(!request.receipt_matches(&id("op", 7), "alice", &wrong));
    let mut wrong = receipt;
    wrong["result"]["unexpected"] = true.into();
    assert!(!request.receipt_matches(&id("op", 7), "alice", &wrong));
}
#[test]
fn directory_discovery_and_restore_require_same_complete_d_and_owner() {
    let node = deletion_node();
    let selected = summary(&node);
    let value = json!(node);
    assert!(
        wire::node(value.clone(), &id("repo", 1), &id("repo", 1), &selected)
            .unwrap()
            .is_some()
    );
    assert!(wire::node(value.clone(), &id("repo", 2), &id("repo", 1), &selected).is_err());
    let mut changed = value.clone();
    changed["namespace_head"] = id("commit", 2).into();
    assert!(
        wire::unchanged_node(changed, &id("repo", 1), &id("repo", 1), &selected, &node).is_err()
    );
    assert!(
        wire::unchanged_node(
            Value::Null,
            &id("repo", 1),
            &id("repo", 1),
            &selected,
            &node
        )
        .unwrap()
        .is_none()
    );
    let page = json!({"snapshot":"snapshot","entries":[selected],"next_after":null});
    assert!(wire::page(page.clone()).is_ok());
    let mut duplicate = page;
    duplicate["entries"] = json!([summary(&node), summary(&node)]);
    assert!(wire::page(duplicate).is_err());
}
