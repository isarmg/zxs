use super::{Read, *};
use crate::files::deletion::{request, restore as file_restore};
use json_codec::json;
use web_interface::prelude::*;

fn id(prefix: &str, number: u32) -> String {
    format!("{prefix}_{number:032x}")
}
fn summary() -> Summary {
    json_codec::from_value(json!({
        "file_id":id("file", 1), "node_id":id("fdel", 1),
        "repository_id":id("repo", 1), "history_tree_id":id("history", 1),
        "path":"same.txt", "operation_id":id("op", 1), "author":"alice",
        "sequence":"1", "created_at":"123", "branch_count":"4097", "parent_count":"4097"
    }))
    .unwrap()
}
fn state() -> State {
    let mut state = State::default();
    state.authenticated = true;
    state.principal = Some("alice".parse().unwrap());
    state.session_epoch = 7;
    state.management.storage = crate::management::Storage::Ready;
    state.files.deletion.open = true;
    state.files.deletion.node = Some(Inspection::Paged(Model {
        summary: Arc::new(summary()),
        page: AuditPage::Empty,
    }));
    state
}
fn target(state: &State) -> Target {
    Target {
        epoch: state.session_epoch,
        generation: state.files.deletion.generation,
        summary: state
            .files
            .deletion
            .node
            .as_ref()
            .unwrap()
            .paged()
            .unwrap()
            .summary
            .clone(),
    }
}
fn receipt() -> Value {
    json!({
        "operation_id":id("op", 2), "operation_kind":"file.delete.undo",
        "request_digest":digest(&summary(), &"alice".parse().unwrap()).unwrap(),
        "state":"completed", "commit_state":"PUBLISHED", "publication_sequence":"2",
        "protection_at_publish":"NOT_APPLICABLE", "error":null,
        "result":{
            "restored_file_id":id("file", 1), "removed_deletion_summary":summary(),
            "timestamp":"124", "membership_epoch":"1", "version_protection":[]
        }
    })
}
#[test]
fn complete_read_budget_codes_choose_pages_without_masking_other_errors() {
    use storage_contract::ErrorCode;
    for code in [
        ErrorCode::ResourceLimitExceeded,
        ErrorCode::ResponseTooLarge,
        ErrorCode::ResourceLimit,
    ] {
        assert!(needs_pages(code));
    }
    for code in [
        ErrorCode::Unauthorized,
        ErrorCode::Forbidden,
        ErrorCode::InvalidRequest,
        ErrorCode::FileNodeNotFound,
        ErrorCode::StateInvalid,
    ] {
        assert!(!needs_pages(code));
    }
}
#[test]
fn summary_selection_binds_file_repository_path_and_exact_d() {
    let summary = summary();
    let selected = wire::Summary {
        file_id: summary.file_id.to_string().parse().unwrap(),
        deletion_node_id: summary.node_id.to_string(),
        repository_id: summary.repository_id.to_string().parse().unwrap(),
        path: summary.path.clone(),
    };
    let value = json!(summary);
    assert_eq!(decode(value.clone(), &selected).unwrap(), summary);
    for (field, changed) in [
        ("file_id", id("file", 2)),
        ("node_id", id("fdel", 2)),
        ("repository_id", id("repo", 2)),
        ("path", "other.txt".into()),
    ] {
        let mut wrong = value.clone();
        wrong[field] = json!(changed);
        assert!(decode(wrong, &selected).is_err());
    }
}
#[test]
fn restore_reread_requires_the_entire_original_summary() {
    let summary = summary();
    assert_eq!(unchanged(json!(summary), &summary).unwrap(), summary);
    for (field, changed) in [
        ("branch_count", json!("4098")),
        ("parent_count", json!("4098")),
        ("operation_id", json!(id("op", 3))),
        ("author", json!("bob")),
        ("sequence", json!("3")),
        ("created_at", json!("125")),
    ] {
        let mut wrong = json!(summary);
        wrong[field] = changed;
        assert!(unchanged(wrong, &summary).is_err());
    }
}
#[test]
fn branch_and_parent_pages_keep_exact_identity_offset_and_complete_entries() {
    let summary = summary();
    let branch = json!({
        "file_id":summary.file_id, "deletion_node_id":summary.node_id,
        "entries":[{"branch_id":id("br", 101), "node_id":id("ver", 101)}],
        "next_after":id("br", 101)
    });
    let read = Read::Branches {
        after: Some(id("br", 100).parse().unwrap()),
        start: 100,
    };
    assert!(
        read.endpoint(&summary)
            .ends_with(&format!("&after={}", id("br", 100)))
    );
    let AuditPage::Branches { start, page } = read.decode(branch.clone(), &summary).unwrap() else {
        panic!("expected the requested branch page");
    };
    assert_eq!(start, 100);
    assert_eq!(page.entries[0].node_id.to_string(), id("ver", 101));
    let parent = json!({
        "file_id":summary.file_id, "deletion_node_id":summary.node_id, "offset":"100",
        "entries":[id("ver", 101), id("fmeta", 102)], "next_offset":"102"
    });
    let read = Read::Parents { offset: 100 };
    let AuditPage::Parents(page) = read.decode(parent.clone(), &summary).unwrap() else {
        panic!("expected the requested parent page");
    };
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.next_offset, Some(102));
    assert!(
        Read::Parents { offset: 0 }
            .decode(parent.clone(), &summary)
            .is_err()
    );
    let mut wrong = parent;
    wrong["deletion_node_id"] = json!(id("fdel", 2));
    assert!(read.decode(wrong, &summary).is_err());
    let mut wrong = branch;
    wrong["file_id"] = json!(id("file", 2));
    assert!(
        Read::Branches {
            after: None,
            start: 0,
        }
        .decode(wrong, &summary)
        .is_err()
    );
}
#[test]
fn page_requests_reject_repeated_clicks_and_superseded_responses() {
    let mut state = state();
    let shown = target(&state);
    let pending = shown.begin(&mut state).unwrap();
    assert!(shown.begin(&mut state).is_none());
    assert!(pending.matches(&state));
    assert!(!pending.current(&state));
    state.files.deletion.loading = false;
    assert!(pending.current(&state));
    let next = pending.begin(&mut state).unwrap();
    assert!(!pending.matches(&state));
    assert!(next.matches(&state));
    state.files.deletion.close();
    assert!(!next.matches(&state));
}
#[test]
fn same_named_replacement_and_new_session_invalidate_summary_controls() {
    let mut state = state();
    let old = target(&state);
    state.files.deletion.node = Some(Inspection::Paged(Model {
        summary: Arc::new(summary()),
        page: AuditPage::Empty,
    }));
    assert!(!old.current(&state));
    let fresh = target(&state);
    state.session_epoch += 1;
    assert!(!fresh.current(&state));
    assert!(old.begin(&mut state).is_none());
}
#[test]
fn compact_undo_uses_the_original_body_and_native_request_digest() {
    let summary = Arc::new(summary());
    let intent = request::Intent::UndoSummary(summary.clone());
    let operation = id("op", 2).parse().unwrap();
    let (endpoint, body) = intent.request(operation);
    assert_eq!(
        endpoint,
        format!("/api/files/{}/deletion/undo-summary", summary.file_id)
    );
    assert_eq!(
        json_codec::to_value(body).unwrap(),
        json!({"deletion_node_id":summary.node_id, "operation_id":operation})
    );
    let expected = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"file.delete.undo", "principal":"alice",
            "body":{"file_id":summary.file_id, "deletion_node_id":summary.node_id}
        }),
    )
    .unwrap();
    assert_eq!(intent.digest(&"alice".parse().unwrap()).unwrap(), expected);
}
#[test]
fn compact_receipt_requires_original_operation_summary_and_confirmed_publication() {
    let summary = summary();
    let id = id("op", 2);
    assert!(receipt_matches(&id, &summary, &receipt()));
    for (field, changed) in [
        ("operation_id", json!("op_33333333333333333333333333333333")),
        ("operation_kind", json!("file.delete")),
        ("publication_sequence", json!("1")),
        ("commit_state", json!("IN_DOUBT")),
        ("state", json!("in_doubt")),
    ] {
        let mut wrong = receipt();
        wrong[field] = changed;
        assert!(!receipt_matches(&id, &summary, &wrong));
    }
    let mut partial = receipt();
    partial["result"]
        .as_object_mut()
        .unwrap()
        .remove("removed_deletion_summary");
    assert!(!receipt_matches(&id, &summary, &partial));
    let mut other = receipt();
    other["result"]["removed_deletion_summary"]["node_id"] =
        json!("fdel_33333333333333333333333333333333");
    assert!(!receipt_matches(&id, &summary, &other));
}
#[test]
fn summary_confirmation_cancels_without_sending_and_old_cancel_keeps_new_review() {
    Owner::new().with(|| {
        let mut initial = state();
        initial.files.deletion.review_generation = 3;
        initial.files.deletion.restore_review =
            Some(file_restore::Review::Summary(restore::Review {
                target: target(&initial),
                generation: 3,
            }));
        let state = RwSignal::new(initial);
        assert!(state.with_untracked(|s| restore::reviewed(s, 7, 3)).is_some());
        restore::cancel(state, 7, 2);
        assert!(state.with_untracked(|s| restore::reviewed(s, 7, 3)).is_some());
        restore::cancel(state, 7, 3);
        assert!(state.with_untracked(|s| s.files.deletion.restore_review.is_none()));
        assert!(state.with_untracked(|s| s.files.deletion.attempt.is_none()));
        assert!(state.with_untracked(|s| s.files.operations.is_empty()));
    });
}
