use super::*;
use crate::files::{model::Navigation, test_entries::entry};

fn id(prefix: &str, n: u32) -> String {
    format!("{prefix}_{n:032x}")
}
fn selected(n: u32, path: &str) -> Entry {
    entry(
        json!({"entry_id":id("entry",n),"file_id":id("file",n),"repository_id":id("repo",1),"path":path}),
    )
}
fn ready() -> State {
    let chosen_entry = selected(1, "A.txt");
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
    state.files.entries = vec![chosen_entry.clone(), selected(2, "B.txt")].into();
    state.files.selected = Some(chosen_entry);
    state
}
fn target() -> Target {
    Target {
        file: HistoryFileId::parse(&id("file", 1)).unwrap(),
        repository: HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
        path: name_policy::PortablePath::parse("A.txt").unwrap(),
    }
}
fn node(operation: &str) -> storage_contract::FileDeletionNode {
    json_codec::from_value(json!({"node_id":id("fdel",1),"history_tree_id":id("history",1),"file_id":id("file",1),"repository_id":id("repo",1),"path":"A.txt",
        "branch_heads":{id("br",1):id("ver",1),id("br",2):id("fmeta",2)},"parent_node_ids":[id("ver",1),id("fmeta",2)],
        "operation_id":operation,"author":"alice","sequence":"1","created_at":"123"})).unwrap()
}
fn receipt(intent: &Intent, operation: &str) -> Value {
    let (kind, result) = match intent {
        Intent::Delete(_) => ("file.delete", json!({"deletion_node":node(operation)})),
        Intent::Undo(node) => (
            "file.delete.undo",
            json!({"restored_file_id":node.file_id,"removed_deletion_node":node.as_ref()}),
        ),
    };
    json!({"operation_id":operation,"operation_kind":kind,"request_digest":intent.digest("alice").unwrap(),"state":"completed","commit_state":"PUBLISHED","protection_at_publish":"NOT_APPLICABLE","publication_sequence":"1","result":result,"error":null})
}
fn attempt(intent: Intent, operation: &str) -> Attempt {
    Attempt {
        epoch: 0,
        principal: "alice".into(),
        operation: operation.into(),
        request_digest: intent.digest("alice").unwrap(),
        intent,
        in_flight: false,
    }
}

#[test]
fn ordinary_delete_routes_to_one_whole_file_review_without_browser_work_or_batch() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready());
        crate::files::batch::start(state, crate::files::batch::Kind::Delete, None);
        state.with_untracked(|s| {
            let review = s.files.deletion.review.as_ref().unwrap();
            assert_eq!(review.target, target());
            assert!(review_allowed(s, review.generation));
            assert!(s.files.batch.is_none());
            assert!(s.files.operations.is_empty());
            assert!(!s.files.mutation_pending);
        });
    });
}
#[test]
fn multiple_missing_read_only_or_directory_selections_never_fall_back_to_batch() {
    let mut state = ready();
    state.files.checked_entries = state.files.entries.iter().map(entry_key).collect();
    assert!(!available(&state));
    assert!(capture(&state, None).unwrap().is_none());
    state.files.checked_entries = ["stale-row".into()].into();
    assert!(capture(&state, None).unwrap().is_none());
    for patch in [
        json!({"kind":"directory"}),
        json!({"read_only":true}),
        json!({"backup":true}),
    ] {
        let current = entry(patch);
        state.files.checked_entries.clear();
        state.files.entries = vec![current.clone()].into();
        state.files.selected = Some(current);
        assert!(capture(&state, None).unwrap().is_none());
    }
    Owner::new().with(|| {
        let state = RwSignal::new(state);
        crate::files::batch::start(state, crate::files::batch::Kind::Delete, None);
        assert!(state.with_untracked(|s| s.files.batch.is_none()
            && s.files.deletion.review.is_none()
            && s.files.operations.is_empty()));
    });
}
#[test]
fn review_is_invalidated_by_refresh_navigation_close_and_replacement_identity() {
    let mut state = ready();
    let original = state.files.selected.clone().unwrap();
    let review = capture(&state, None).unwrap().unwrap();
    let generation = review.generation;
    state.files.deletion.review_generation = generation;
    state.files.deletion.review = Some(review.clone());
    assert!(review_allowed(&state, generation));
    state.files.generation += 1;
    assert!(!review_allowed(&state, generation));
    state.files.generation -= 1;
    let replacement = selected(9, "A.txt");
    state.files.entries = vec![replacement.clone()].into();
    state.files.selected = Some(replacement);
    assert!(!review_allowed(&state, generation));
    assert!(capture(&state, Some(&original)).unwrap().is_none());
    state = ready();
    state.files.deletion.review_generation = generation;
    state.files.deletion.review = Some(review);
    state.files.deletion.cancel_review();
    let replacement = capture(&state, None).unwrap().unwrap();
    assert_ne!(replacement.generation, generation);
    state.files.deletion.review_generation = replacement.generation;
    state.files.deletion.review = Some(replacement);
    assert!(!review_allowed(&state, generation));
    state
        .files
        .navigate(Navigation::Directory("elsewhere".into()));
    assert!(state.files.deletion.review.is_none());
}
#[test]
fn request_body_is_only_exact_file_location_and_original_operation() {
    let operation = id("op", 7);
    let intent = Intent::Delete(target());
    let (endpoint, body) = intent.request(&operation);
    assert_eq!(endpoint, format!("/api/files/{}/deletion", id("file", 1)));
    assert_eq!(
        body,
        json!({"repository_id":id("repo",1),"path":"A.txt","operation_id":operation})
    );
    assert_eq!(intent.digest("alice").unwrap(),storage_contract::object_hash("request",&json!({"kind":"file.delete","principal":"alice","body":{"repository_id":id("repo",1),"file_id":id("file",1),"path":"a.txt"}})).unwrap());
}
#[test]
fn delete_receipt_must_match_kind_owner_file_complete_frontier_and_request_digest() {
    let id = id("op", 7);
    let intent = Intent::Delete(target());
    let valid = receipt(&intent, &id);
    let mut state = ready();
    state.files.deletion.attempt = Some(attempt(intent, &id));
    assert!(receipt_matches(&state, &id, &valid));
    assert_eq!(
        FileOperation::from_receipt(&id, &valid).status(),
        SaveStatus::Published
    );
    for (field, replacement) in [
        ("operation_kind", json!("file.delete.undo")),
        ("request_digest", json!("0".repeat(64))),
    ] {
        let mut wrong = valid.clone();
        wrong[field] = replacement;
        assert!(!receipt_matches(&state, &id, &wrong), "{field}");
    }
    for (field, replacement) in [
        ("file_id", json!(id_from_number("file", 2))),
        ("author", json!("other")),
        ("operation_id", json!(id_from_number("op", 8))),
        ("parent_node_ids", json!([id_from_number("ver", 1)])),
        ("version_id", json!(id_from_number("ver", 1))),
    ] {
        let mut wrong = valid.clone();
        wrong["result"]["deletion_node"][field] = replacement;
        assert!(!receipt_matches(&state, &id, &wrong), "{field}");
    }
    state.principal = "other".into();
    assert!(!receipt_matches(&state, &id, &valid));
    state.principal = "alice".into();
    state.session_epoch += 1;
    assert!(!receipt_matches(&state, &id, &valid));
}
// Keep the original operation variable readable without shadowing fixture IDs.
fn id_from_number(prefix: &str, n: u32) -> String {
    id(prefix, n)
}
#[test]
fn undo_receipt_cannot_restore_another_node_and_close_keeps_unknown_fence() {
    Owner::new().with(|| {
        let operation = id("op", 8);
        let deleted = Arc::new(node(&id("op", 7)));
        let intent = Intent::Undo(deleted.clone());
        let valid = receipt(&intent, &operation);
        let mut initial = ready();
        initial.files.deletion.open = true;
        initial.files.deletion.node = Some(deleted);
        initial.files.deletion.attempt = Some(attempt(intent, &operation));
        initial.files.slot.clear();
        initial
            .files
            .operations
            .push(FileOperation::unknown(&operation));
        let state = RwSignal::new(initial);
        let mut wrong = valid.clone();
        wrong["result"]["removed_deletion_node"]["node_id"] = id("fdel", 2).into();
        assert_eq!(
            actions::record_receipt(state, &operation, wrong),
            SaveStatus::Unknown
        );
        state.update(|s| {
            s.files.deletion.close();
            s.files.navigate(Navigation::Root);
        });
        assert!(state.with_untracked(actions::pending));
        assert!(state.with_untracked(|s| receipt_matches(s, &operation, &valid)));
        assert_eq!(
            actions::record_receipt(state, &operation, valid),
            SaveStatus::Published
        );
        assert!(!state.with_untracked(actions::pending));
        assert!(state.with_untracked(
            |s| s.files.deletion.attempt.is_none() && s.files.deletion.node.is_none()
        ));
    });
}
#[test]
fn stale_undo_button_and_unknown_attempt_do_not_generate_new_browser_requests() {
    Owner::new().with(|| {
        let state = RwSignal::new(ready());
        state.update(|s| {
            s.files.deletion.open = true;
            s.files.deletion.node = Some(Arc::new(node(&id("op", 7))));
        });
        super::super::undo(state, &id("file", 1), &id("fdel", 2));
        assert!(state.with_untracked(|s| s.files.operations.is_empty()));
        state.update(|s| {
            s.files
                .operations
                .push(FileOperation::unknown(&id("op", 8)))
        });
        super::super::undo(state, &id("file", 1), &id("fdel", 1));
        assert!(!state.with_untracked(|s| s.files.mutation_pending));
    });
}
