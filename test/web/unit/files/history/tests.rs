use super::*;
use json_codec::{Value, json};

pub(crate) const ROOT: &str = "c61d906894ffed82c4212168ad158ed1817aec73cda4ef3f43a9b44129634c64";
pub(crate) const OTHER_ROOT: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";
pub(crate) const OPERATION: &str = "op_0123456789abcdef0123456789abcdef";
pub(crate) const HASH: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

fn extend(mut value: Value, overrides: Value) -> Value {
    value
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    value
}

/// One wire node. `node_id == version_id` for content nodes.
pub(crate) fn node_wire(overrides: Value) -> Value {
    extend(
        json!({
            "node_id":"n1", "kind":"content", "version_id":"n1", "sequence":"1",
            "created_at":"2026-01-01T00:00:01Z", "branch_id":"branch", "path":"docs/a.txt",
            "previous_path":null, "note":"", "tags":[], "size":"0", "content_hash":HASH,
            "content_state":"LIVE", "download_name":"a.txt", "storage_name":"v1.txt",
            "operation_id":OPERATION, "author":{"bytes":5,"inline":"admin"},
            "message":{"bytes":0,"inline":""},
            "parents":{"status":"complete","edges":[],"cursor":null}, "current":false
        }),
        overrides,
    )
}
/// A node with the given id, sequence and parent ids; `in_window` is computed
/// against `page` so fixtures stay consistent with the wire rule.
pub(crate) fn node(id: &str, sequence: u64, parents: &[&str], page: &[&str]) -> Value {
    node_wire(json!({
        "node_id":id, "version_id":id, "sequence":sequence.to_string(),
        "created_at":format!("2026-01-01T00:{:02}:{:02}Z", sequence / 60 % 60, sequence % 60),
        "parents":{"status":"complete","edges":parents.iter().map(|parent|json!({"node_id":parent,"in_window":page.contains(parent)})).collect::<Vec<_>>(),"cursor":null}
    }))
}
pub(crate) fn window_wire(nodes: Vec<Value>, overrides: Value) -> Value {
    extend(
        json!({
            "file_id":"file", "repository_id":"repo", "history_tree_id":"history",
            "history_root":ROOT,
            "context":{"path":"docs/a.txt","note":"","current_node_id":null,"selected_branch_id":null,
                "version_id":"n1","materialized":true,"inherited":false,"own_version_id":"n1",
                "source_version_id":null,"content_state":"LIVE","visible_content_state":"LIVE"},
            "nodes":nodes, "previous_cursor":null, "next_cursor":null, "anchor":null,
            "budget":{"limit":50,"parent_edges":2048}
        }),
        overrides,
    )
}
pub(crate) fn window(nodes: Vec<Value>, overrides: Value) -> HistoryWindow {
    HistoryWindow::parse(window_wire(nodes, overrides), "file").unwrap()
}
/// A loaded window with `count` content nodes in a chain (newest first).
pub(crate) fn chain(count: u64) -> HistoryWindow {
    let ids = (1..=count).map(|i| format!("n{i}")).collect::<Vec<_>>();
    let page = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let nodes = (1..=count)
        .rev()
        .map(|i| {
            let parents = if i > 1 {
                vec![format!("n{}", i - 1)]
            } else {
                vec![]
            };
            let parents = parents.iter().map(String::as_str).collect::<Vec<_>>();
            node(&format!("n{i}"), i, &parents, &page)
        })
        .collect();
    window(
        nodes,
        json!({"budget":{"limit":count.max(50),"parent_edges":2048}}),
    )
}
/// Install a loaded window into the model's history slot.
pub(crate) fn loaded(model: &mut crate::files::Model, window: HistoryWindow) {
    model.history.install(window);
}
pub(crate) fn binding() -> HistoryBinding {
    HistoryBinding {
        file_id: "file".into(),
        history_tree_id: "history".into(),
        root: HistoryRoot::parse(ROOT).unwrap(),
        repository_id: "repo".into(),
        path: Some("docs/a.txt".into()),
    }
}
pub(crate) fn branch_wire(overrides: Value) -> Value {
    extend(
        json!({
            "branch_id":"branch", "history_tree_id":"history", "name":"main",
            "head_commit_id":"head", "fork_commit_id":"fork", "branch_kind":"file",
            "file_id":"file", "generation":"1", "state":"active",
            "materialized":true, "inherited":false, "content_state":"LIVE",
            "version_id":"version", "own_version_id":"version", "source_version_id":null,
            "visible_content_state":"LIVE", "read_only":false, "alias_of":null,
            "node_id":"version", "current":false, "pending_count":0
        }),
        overrides,
    )
}
pub(crate) fn branch_page_wire(branches: Vec<Value>, selected: Option<&str>) -> Value {
    json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,
        "selected_branch_id":selected,"branches":branches,"previous_cursor":null,"next_cursor":null})
}
pub(crate) fn branch_page(branches: Vec<Value>, selected: Option<&str>) -> BranchPage {
    BranchPage::parse(branch_page_wire(branches, selected), &binding()).unwrap()
}
pub(crate) fn branch(overrides: Value) -> FileBranch {
    let id = overrides["branch_id"]
        .as_str()
        .unwrap_or("branch")
        .to_owned();
    branch_page(vec![branch_wire(overrides)], None)
        .branch(&id)
        .unwrap()
        .clone()
}
pub(crate) fn merge_wire(overrides: Value) -> Value {
    extend(
        json!({"merge_id":"m1","sequence":"9","chosen_node_id":"n2","result_node_id":"n2",
            "surviving_branch_id":"branch","reused_node":true,"operation_id":OPERATION,
            "input_count":2,"parent_count":2}),
        overrides,
    )
}
pub(crate) fn merge_page_wire(merges: Vec<Value>) -> Value {
    json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,
        "merges":merges,"previous_cursor":null,"next_cursor":null})
}

#[test]
fn window_decode_keeps_typed_facts_and_rejects_unknown_or_inconsistent_fields() {
    let page = ["n2", "n1"];
    let wire = window_wire(
        vec![
            node_wire(
                json!({"node_id":"n2","version_id":"n2","sequence":"18446744073709551615",
                "tags":["b","a"],"size":"18446744073709551615","note":"two",
                "message":{"bytes":300,"inline":null},
                "parents":{"status":"more","edges":[{"node_id":"n1","in_window":true},{"node_id":"outside","in_window":false}],"cursor":"ab"}}),
            ),
            node("n1", 1, &[], &page),
        ],
        json!({"context":{"path":"docs/a.txt","note":"file note","current_node_id":"n2","selected_branch_id":"branch",
            "version_id":"n2","materialized":true,"inherited":false,"own_version_id":"n2",
            "source_version_id":null,"content_state":"LIVE","visible_content_state":"LIVE"},
            "next_cursor":"cc","anchor":{"node_id":"n2","in_page":true}}),
    );
    let mut wire = wire;
    wire["nodes"][0]["current"] = true.into();
    let window = HistoryWindow::parse(wire.clone(), "file").unwrap();
    assert_eq!(window.binding, binding());
    assert_eq!(window.context.note, "file note");
    assert_eq!(window.context.current_node_id.as_deref(), Some("n2"));
    assert_eq!(window.context.selected_branch_id.as_deref(), Some("branch"));
    assert!(window.context.content.as_ref().unwrap().materialized);
    assert_eq!(window.nodes.len(), 2);
    let two = &window.nodes[0];
    assert_eq!(two.sequence, u64::MAX);
    assert_eq!(two.size, u64::MAX);
    assert_eq!(two.tags.iter().cloned().collect::<Vec<_>>(), ["a", "b"]);
    assert!(two.current && !window.nodes[1].current);
    assert_eq!(
        two.message,
        BoundedText {
            bytes: 300,
            inline: None
        }
    );
    assert!(two.parents.more());
    assert_eq!(two.parents.in_window().collect::<Vec<_>>(), ["n1"]);
    assert_eq!(
        two.parents.outside_window().collect::<Vec<_>>(),
        ["outside"]
    );
    assert_eq!(window.next_cursor.as_deref(), Some("cc"));
    assert_eq!(window.anchor.as_deref(), Some("n2"));
    assert!(!window.has_no_history());
    assert_eq!(window.identity().first_node.as_deref(), Some("n2"));
    assert!(HistoryWindow::parse(wire.clone(), "other").is_err());

    let reject = |patch: fn(&mut Value)| {
        let mut value = wire.clone();
        patch(&mut value);
        HistoryWindow::parse(value, "file").is_err()
    };
    assert!(reject(|v| v["extra"] = true.into()));
    assert!(reject(|v| v["nodes"][0]["extra"] = true.into()));
    assert!(reject(|v| v["history_root"] = "ABC".into()));
    assert!(reject(|v| v["history_root"] = "c".repeat(63).into()));
    assert!(reject(|v| v["nodes"][0]["sequence"] = json!(5)));
    assert!(reject(|v| v["nodes"][0]["sequence"] = "01".into()));
    assert!(
        reject(|v| v["nodes"][0]["parents"]["cursor"] = Value::Null),
        "more needs a cursor"
    );
    assert!(
        reject(|v| v["nodes"][1]["parents"]["cursor"] = "zz".into()),
        "complete has no cursor"
    );
    assert!(reject(
        |v| v["nodes"][0]["parents"]["edges"][0]["in_window"] = false.into()
    ));
    assert!(reject(
        |v| v["nodes"][0]["parents"]["edges"][1]["in_window"] = true.into()
    ));
    assert!(
        reject(|v| v["nodes"][0]["current"] = false.into()),
        "current follows context"
    );
    assert!(
        reject(|v| v["nodes"][1]["node_id"] = "n2".into()),
        "duplicate node"
    );
    assert!(
        reject(|v| v["nodes"].as_array_mut().unwrap().swap(0, 1)),
        "canonical order"
    );
    assert!(
        reject(|v| v["nodes"][0]["message"] = json!({"bytes":10,"inline":null})),
        "short text must be inline"
    );
    assert!(
        reject(|v| v["nodes"][0]["message"] = json!({"bytes":300,"inline":"x"})),
        "long text is never inline"
    );
    assert!(
        reject(|v| v["nodes"][0]["author"] = json!({"bytes":4,"inline":"admin"})),
        "bytes must match"
    );
    assert!(reject(|v| v["nodes"][0]["tags"] = json!(["a", "a"])));
    assert!(reject(|v| v["nodes"][0]["content_hash"] = "nothex".into()));
    assert!(reject(|v| v["nodes"][0]["operation_id"] = "op_short".into()));
    assert!(
        reject(|v| v["nodes"][0]["kind"] = "rename".into()),
        "rename needs previous_path"
    );
    assert!(
        reject(|v| v["nodes"][0]["version_id"] = "other".into()),
        "content node id is its version"
    );
    assert!(reject(|v| v["anchor"]["in_page"] = false.into()));
    assert!(reject(|v| v["budget"]["limit"] = json!(1)));
    assert!(reject(|v| v["budget"]["parent_edges"] = json!(1)));
    assert!(reject(|v| v["next_cursor"] = "".into()));
    assert!(reject(|v| v["next_cursor"] = "x".repeat(32_769).into()));
    assert!(
        reject(|v| v["context"]["version_id"] = Value::Null),
        "all content fields or none"
    );
    assert!(reject(|v| {
        v["context"].as_object_mut().unwrap().remove("materialized");
    }));
    assert!(reject(|v| v["file_id"] = "other".into()));
    assert!(reject(|v| v["repository_id"] = "".into()));
}

#[test]
fn deleted_file_context_has_no_path_or_content_but_nodes_stay_readable() {
    let wire = window_wire(
        vec![node("n1", 1, &[], &["n1"])],
        json!({"context":{"path":null,"note":"","current_node_id":null,"selected_branch_id":null}}),
    );
    let window = HistoryWindow::parse(wire.clone(), "file").unwrap();
    assert!(window.binding.path.is_none());
    assert!(window.context.content.is_none());
    assert_eq!(window.nodes.len(), 1);
    let mut with_content = wire;
    with_content["context"]["version_id"] = "n1".into();
    with_content["context"]["materialized"] = true.into();
    with_content["context"]["inherited"] = false.into();
    with_content["context"]["own_version_id"] = "n1".into();
    with_content["context"]["source_version_id"] = Value::Null;
    with_content["context"]["content_state"] = "LIVE".into();
    with_content["context"]["visible_content_state"] = "LIVE".into();
    assert!(
        HistoryWindow::parse(with_content, "file").is_err(),
        "content disposition requires a current entry path"
    );
}

#[test]
fn an_empty_first_page_without_continuation_is_the_only_proof_of_no_history() {
    let empty = window(vec![], json!({}));
    assert!(empty.has_no_history());
    let later = window(vec![], json!({"previous_cursor":"aa"}));
    assert!(!later.has_no_history());
    let more = window(vec![], json!({"next_cursor":"bb"}));
    assert!(!more.has_no_history());
}

#[test]
fn backup_pinning_moves_only_the_current_marker_and_keeps_every_node() {
    let page = ["primary", "stored"];
    let mut wire = window_wire(
        vec![
            node("primary", 2, &["stored"], &page),
            node("stored", 1, &[], &page),
        ],
        json!({"context":{"path":"docs/a.txt","note":"","current_node_id":"primary","selected_branch_id":null,
            "version_id":"primary","materialized":true,"inherited":false,"own_version_id":"primary",
            "source_version_id":null,"content_state":"LIVE","visible_content_state":"LIVE"}}),
    );
    wire["nodes"][0]["current"] = true.into();
    let admitted = HistoryWindow::parse(wire, "file").unwrap();
    let entry = crate::files::test_entries::entry(
        json!({"backup":true,"version_id":"stored","node_id":"stored"}),
    );
    let pinned = admitted.clone().for_entry(&entry);
    assert_eq!(pinned.context.current_node_id.as_deref(), Some("stored"));
    assert_eq!(
        pinned
            .context
            .content
            .as_ref()
            .unwrap()
            .version_id
            .as_deref(),
        Some("stored")
    );
    assert!(!pinned.nodes[0].current && pinned.nodes[1].current);
    assert_eq!(pinned.nodes.len(), 2);
    assert_eq!(pinned.binding, admitted.binding);
    let primary = crate::files::test_entries::entry(json!({"version_id":"primary"}));
    assert_eq!(admitted.clone().for_entry(&primary), admitted);
}

#[test]
fn branch_page_decode_binds_the_resident_root_and_keeps_the_write_disposition() {
    for state in ["active", "archived", "migrated", "merged"] {
        for alias in [None, Some("survivor")] {
            let readonly = state != "active" || alias.is_some();
            let value = branch_wire(json!({"state":state, "alias_of":alias, "read_only":readonly}));
            let page = branch_page(vec![value.clone()], None);
            assert_eq!(page.branches()[0].writable(), !readonly);
            let mut contradictory = value;
            contradictory["read_only"] = (!readonly).into();
            assert!(
                BranchPage::parse(branch_page_wire(vec![contradictory], None), &binding()).is_err()
            );
        }
    }
    let current = branch_wire(json!({"current":true}));
    let page = branch_page(vec![current.clone()], Some("branch"));
    assert!(page.branch("branch").is_some());
    assert_eq!(page.root().as_str(), ROOT);
    assert!(BranchPage::parse(branch_page_wire(vec![current], None), &binding()).is_err());
    let mut other_root = branch_page_wire(vec![], None);
    other_root["history_root"] = OTHER_ROOT.into();
    assert!(BranchPage::parse(other_root, &binding()).is_err());
    let mut other_history = branch_page_wire(vec![], None);
    other_history["history_tree_id"] = "other".into();
    assert!(BranchPage::parse(other_history, &binding()).is_err());
    let duplicate = branch_wire(json!({}));
    assert!(
        BranchPage::parse(
            branch_page_wire(vec![duplicate.clone(), duplicate], None),
            &binding()
        )
        .is_err()
    );
    let unordered = vec![
        branch_wire(json!({"branch_id":"b"})),
        branch_wire(json!({"branch_id":"a"})),
    ];
    assert!(BranchPage::parse(branch_page_wire(unordered, None), &binding()).is_err());
    let mut legacy = branch_page_wire(vec![branch_wire(json!({}))], None);
    legacy["branches"][0]["pending_file_versions"] = json!({});
    assert!(
        BranchPage::parse(legacy, &binding()).is_err(),
        "the whole pending map is not part of the summary"
    );
    let pending = branch(json!({
        "materialized":false, "inherited":true, "content_state":"PENDING",
        "visible_content_state":"AVAILABLE", "source_version_id":"source",
        "version_id":"source", "own_version_id":null, "node_id":null, "pending_count":1
    }));
    assert!(pending.pending());
    assert_eq!(pending.head(), Some("source"));
    assert!(
        BranchPage::parse(
            branch_page_wire(vec![branch_wire(json!({"pending_count":1}))], None),
            &binding()
        )
        .is_err(),
        "a materialized branch has no pending entries"
    );
}

#[test]
fn merge_page_decode_requires_canonical_order_and_consistent_reuse() {
    let page = MergePage::parse(
        merge_page_wire(vec![
            merge_wire(json!({"merge_id":"b","sequence":"9"})),
            merge_wire(json!({"merge_id":"a","sequence":"8","chosen_node_id":"n1","result_node_id":"n3","reused_node":false})),
        ]),
        &binding(),
    )
    .unwrap();
    assert_eq!(page.merges.len(), 2);
    assert_eq!(page.merges[1].input_count, 2);
    for merges in [
        vec![
            merge_wire(json!({"merge_id":"a","sequence":"8"})),
            merge_wire(json!({"merge_id":"b","sequence":"9"})),
        ],
        vec![
            merge_wire(json!({"merge_id":"b","sequence":"9"})),
            merge_wire(json!({"merge_id":"a","sequence":"9"})),
        ],
        vec![merge_wire(json!({})), merge_wire(json!({}))],
        vec![merge_wire(json!({"reused_node":false}))],
        vec![merge_wire(json!({"input_count":1}))],
        vec![merge_wire(json!({"operation_id":"op_x"}))],
    ] {
        assert!(MergePage::parse(merge_page_wire(merges), &binding()).is_err());
    }
}

#[test]
fn parents_inputs_and_field_slices_decode_with_exact_continuation_rules() {
    let parents = ParentsPage::parse(
        json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,"node_id":"n9",
            "parents":[{"node_id":"a"},{"node_id":"b"}],"previous_cursor":null,"next_cursor":"cc"}),
        &binding(),
        "n9",
    )
    .unwrap();
    assert_eq!(parents.parents, ["a", "b"]);
    assert_eq!(parents.next_cursor.as_deref(), Some("cc"));
    for (parents, node) in [
        (json!([{"node_id":"b"},{"node_id":"a"}]), "n9"),
        (json!([{"node_id":"n9"}]), "n9"),
        (json!([{"node_id":"a"}]), "other"),
    ] {
        assert!(
            ParentsPage::parse(
                json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,"node_id":"n9",
                    "parents":parents,"previous_cursor":null,"next_cursor":null}),
                &binding(),
                node,
            )
            .is_err()
        );
    }
    let inputs = |offset: usize, total: usize, items: Value, next: Value| {
        MergeInputsPage::parse(
            json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,"merge_id":"m",
                "offset":offset,"total":total,"inputs":items,"next_offset":next}),
            &binding(),
            "m",
        )
    };
    let page = inputs(
        2,
        4,
        json!([{"index":2,"branch_id":"b","node_id":"n"},{"index":3,"branch_id":"c","node_id":"n"}]),
        Value::Null,
    )
    .unwrap();
    assert_eq!(page.previous_offset(), Some(0));
    assert!(page.next_offset.is_none());
    assert!(
        inputs(
            0,
            4,
            json!([{"index":0,"branch_id":"b","node_id":"n"}]),
            json!(1)
        )
        .is_ok()
    );
    assert!(
        inputs(
            0,
            4,
            json!([{"index":0,"branch_id":"b","node_id":"n"}]),
            Value::Null
        )
        .is_err()
    );
    assert!(
        inputs(
            0,
            4,
            json!([{"index":1,"branch_id":"b","node_id":"n"}]),
            json!(1)
        )
        .is_err()
    );
    assert!(
        inputs(0, 1, json!([]), Value::Null).is_err(),
        "non-final empty slice"
    );
    assert!(inputs(0, 0, json!([]), Value::Null).is_ok());
    let slice = |offset: usize, total: usize, text: &str, complete: bool, next: Value| {
        FieldSlice::parse(
            json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,"node_id":"n",
                "field":"message","total_bytes":total,"offset":offset,"text":text,"complete":complete,"next_offset":next}),
            &binding(),
            "n",
            NodeField::Message,
        )
    };
    assert!(slice(0, 6, "文", false, json!(3)).is_ok());
    assert!(slice(3, 6, "字", true, Value::Null).is_ok());
    assert!(
        slice(0, 0, "", true, Value::Null).is_ok(),
        "empty field is complete"
    );
    assert!(slice(0, 6, "文", true, json!(3)).is_err());
    assert!(slice(3, 6, "字", false, Value::Null).is_err());
    assert!(slice(0, 6, "文字", true, Value::Null).is_ok());
    assert!(
        FieldSlice::parse(
            json!({"file_id":"file","history_tree_id":"history","history_root":ROOT,"node_id":"n",
                "field":"author","total_bytes":1,"offset":0,"text":"a","complete":true,"next_offset":null}),
            &binding(),
            "n",
            NodeField::Message,
        )
        .is_err(),
        "field enum must match the request"
    );
}

#[test]
fn decoding_keeps_the_concrete_error_source() {
    use std::error::Error;
    let error = HistoryWindow::parse(json!({"file_id":"file"}), "file").unwrap_err();
    assert_eq!(error.status, 200);
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    assert!(error.source().unwrap().source().unwrap().source().is_some());
}

#[test]
fn directory_and_file_branch_contracts_are_not_interchangeable() {
    let directory = crate::files::test_entries::branch_wire(json!({}));
    assert!(BranchPage::parse(branch_page_wire(vec![directory], None), &binding()).is_err());
    assert!(crate::files::page::DirectoryBranch::parse(branch_wire(json!({}))).is_err());
}

#[test]
fn actual_server_branch_fixtures_enter_the_web_model_without_shape_reconstruction() {
    // The server test compares actual capture + response serialization with
    // these same bytes.
    let fixtures = storage_contract::strict_json(include_bytes!(
        "../../../../fixtures/file-history-branches.json"
    ))
    .unwrap();
    let fixtures = fixtures.as_object().unwrap();
    assert_eq!(fixtures.len(), 6);
    for (case, selected, branch) in [
        (
            "explicit-node",
            Some("branch"),
            Some(("file-node", false, true)),
        ),
        ("visible-node", None, Some(("body", false, true))),
        ("merged-pending", Some("branch"), Some(("body", true, true))),
        (
            "empty-source",
            Some("full-branch"),
            Some(("body", true, false)),
        ),
        ("no-selection", None, None),
        ("full-selection", Some("full-branch"), None),
    ] {
        let page = BranchPage::parse(fixtures[case].clone(), &binding()).unwrap();
        assert_eq!(
            fixtures[case]["selected_branch_id"].as_str(),
            selected,
            "{case}"
        );
        if let Some((head, pending, writable)) = branch {
            assert_eq!(page.branches().len(), 1, "{case}");
            let branch = &page.branches()[0];
            assert_eq!(branch.id(), "branch", "{case}");
            assert_eq!(branch.name(), "main", "{case}");
            assert_eq!(branch.head(), Some(head), "{case}");
            assert_eq!(branch.pending(), pending, "{case}");
            assert_eq!(branch.writable(), writable, "{case}");
        } else {
            assert!(page.branches().is_empty(), "{case}");
        }
    }
}

#[test]
fn actual_server_window_fixtures_enter_the_web_model_without_shape_reconstruction() {
    // The server test compares its real node-page encoder output with these
    // same bytes, so the strict decoder is checked against the producer.
    let fixtures = storage_contract::strict_json(include_bytes!(
        "../../../../fixtures/file-history-window.json"
    ))
    .unwrap();
    let fixtures = fixtures.as_object().unwrap();
    assert_eq!(fixtures.len(), 2);

    let live = HistoryWindow::parse(fixtures["live"].clone(), "file").unwrap();
    assert_eq!(live.nodes.len(), 2);
    assert!(live.next_cursor.is_some() && live.previous_cursor.is_none());
    let content = &live.nodes[0];
    assert!(content.current, "current follows context.current_node_id");
    assert_eq!(
        content.operation_id, None,
        "content nodes carry no operation"
    );
    assert_eq!(content.size, u64::MAX);
    assert_eq!(content.message.bytes, 300);
    assert!(
        content.message.inline.is_none(),
        "long text is read through field slices"
    );
    assert_eq!(
        content.parents.in_window().collect::<Vec<_>>(),
        ["fnode_00000000000000000000000000000002"]
    );
    assert_eq!(
        content.parents.outside_window().collect::<Vec<_>>(),
        ["ver_00000000000000000000000000000001"]
    );
    let rename = &live.nodes[1];
    assert!(!rename.current);
    assert!(rename.operation_id.is_some());
    assert!(
        matches!(rename.parents.status, ParentStatus::More { .. }),
        "an unbudgeted parent set is 'more' with a continuation, never complete"
    );

    let deleted = HistoryWindow::parse(fixtures["deleted"].clone(), "file").unwrap();
    assert!(deleted.binding.path.is_none());
    assert!(deleted.context.content.is_none());
    assert!(deleted.has_no_history());
}
