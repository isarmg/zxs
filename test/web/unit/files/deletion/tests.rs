use super::*;
use crate::files::{FileOperation, SaveStatus};
use json_codec::{Value, json};
fn id(prefix: &str, n: u32) -> String {
    format!("{prefix}_{n:032x}")
}
fn summary() -> wire::Summary {
    wire::page(json!({"snapshot":"snapshot","entries":[{"file_id":id("file",1),"deletion_node_id":id("fdel",1),"repository_id":id("repo",1),"path":"A.txt"}],"next_after":null})).unwrap().entries.pop().unwrap()
}
fn node() -> Value {
    json!({"node_id":id("fdel",1),"history_tree_id":id("history",1),"file_id":id("file",1),"repository_id":id("repo",1),"path":"A.txt",
        "branch_heads":{id("br",1):id("ver",1),id("br",2):id("fmeta",2)},"parent_node_ids":[id("ver",1),id("fmeta",2)],
        "operation_id":id("op",1),"author":"alice","sequence":"1","created_at":"123"})
}
#[test]
fn deletion_frontier_is_bodyless_and_exactly_joins_all_retained_branch_heads() {
    let parsed = wire::node(node(), &summary()).unwrap().unwrap();
    assert_eq!(parsed.branch_heads.len(), 2);
    assert_eq!(parsed.parent_node_ids.len(), 2);
    let mut counterfeit = node();
    counterfeit["version_id"] = id("ver", 1).into();
    assert!(wire::node(counterfeit, &summary()).is_err());
    assert!(wire::node(Value::Null, &summary()).unwrap().is_none());
}
#[test]
fn deletion_frontier_rejects_missing_or_extra_edges_and_changed_identity() {
    for field in ["file_id", "repository_id", "node_id", "path"] {
        let mut wrong = node();
        wrong[field] = "changed".into();
        assert!(wire::node(wrong, &summary()).is_err(), "{field}");
    }
    for edges in [
        json!([id("ver", 1)]),
        json!([id("ver", 1), id("fmeta", 2), id("ver", 3)]),
        json!([id("ver", 1), id("fmeta", 2), id("ver", 1)]),
    ] {
        let mut wrong = node();
        wrong["parent_node_ids"] = edges;
        assert!(wire::node(wrong, &summary()).is_err());
    }
}
#[test]
fn deletion_page_requires_bounded_unique_ordered_identity_and_explicit_cursor() {
    let entry = json!({"file_id":id("file",1),"deletion_node_id":id("fdel",1),"repository_id":id("repo",1),"path":"A.txt"});
    let good = json!({"snapshot":"abc","entries":[entry.clone()],"next_after":null});
    assert!(wire::page(good.clone()).is_ok());
    let mut wrong = good.clone();
    wrong.as_object_mut().unwrap().remove("next_after");
    assert!(wire::page(wrong).is_err());
    let mut wrong = good.clone();
    wrong["entries"] = json!([entry.clone(), entry.clone()]);
    assert!(wire::page(wrong).is_err());
    let mut wrong = good.clone();
    wrong["entries"] = Value::Array(vec![entry; 101]);
    assert!(wire::page(wrong).is_err());
    let mut wrong = good.clone();
    wrong["next_after"] = id("file", 1).into();
    assert!(wire::page(wrong).is_err());
    let mut wrong = good.clone();
    wrong["snapshot"] = "".into();
    assert!(wire::page(wrong).is_err());
    let mut wrong = good;
    wrong["extra"] = true.into();
    assert!(wire::page(wrong).is_err());
}
#[test]
fn closed_or_superseded_deletion_reads_cannot_repopulate_state() {
    let mut model = Model {
        open: true,
        ..Default::default()
    };
    let first = model.begin();
    assert!(model.accepts(first));
    let second = model.begin();
    assert!(!model.accepts(first));
    assert!(model.accepts(second));
    model.close();
    assert!(!model.accepts(second));
    model.open = true;
    assert!(!model.accepts(second));
}
#[test]
fn restore_action_requires_active_session_exact_read_and_settled_global_operation() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        undo(state, &id("file", 1), &id("fdel", 1)); // Does not enter a browser UUID/network operation.
        state.update(|s| {
            s.authenticated = true;
            s.management.storage = crate::management::Storage::Ready;
            s.files.deletion.open = true;
            s.files.deletion.node =
                Some(Arc::new(wire::node(node(), &summary()).unwrap().unwrap()));
            assert!(undo_allowed(s));
            s.files.operations.push(FileOperation::unknown("original"));
        });
        assert!(!state.with_untracked(undo_allowed));
        undo(state, &id("file", 1), &id("fdel", 1));
        state.update(|s| s.files.deletion.close());
        assert!(state.with_untracked(actions::pending));
        assert_eq!(
            state.with_untracked(|s| s.files.operations[0].status()),
            SaveStatus::Unknown
        );
        assert_eq!(
            state.with_untracked(|s| s.files.operations[0].id().to_owned()),
            "original"
        );
        assert!(!state.with_untracked(|s| s.files.mutation_pending));
    });
}
#[test]
fn undo_errors_use_global_original_operation_fence_even_after_panel_closes() {
    Owner::new().with(|| {
        for (status, expected) in [
            (0, SaveStatus::Unknown),
            (200, SaveStatus::Unknown),
            (500, SaveStatus::Unknown),
            (409, SaveStatus::Rejected),
        ] {
            let state = RwSignal::new(State::default());
            actions::record_failure(
                state,
                0,
                "undo-original",
                api::Error {
                    status,
                    code: storage_contract::ErrorCode::FileConflict,
                    message: "error".into(),
                    retryable: false,
                    request_id: None,
                    source: None,
                },
            );
            state.update(|s| s.files.deletion.close());
            assert_eq!(
                state.with_untracked(|s| s.files.operations[0].status()),
                expected
            );
            assert_eq!(
                state.with_untracked(|s| s.files.operations[0].id().to_owned()),
                "undo-original"
            );
        }
    });
}
