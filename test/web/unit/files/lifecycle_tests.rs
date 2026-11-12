use super::*;
use crate::files::{history, test_entries};
use json_codec::json;
use storage_contract::file_lifecycle::{Cursor, Timestamp};
use storage_contract::history_window::{HistoryRepositoryId, HistoryTreeId};

fn id(prefix: &str, number: u64) -> String {
    format!("{prefix}_{number:032x}")
}

fn target() -> Target {
    Target {
        epoch: 1,
        navigation: 2,
        entry: test_entries::entry(json!({
            "file_id":id("file", 1), "entry_id":id("entry", 1),
            "repository_id":id("repo", 1), "repository_path":"notes/a.txt",
            "path":"notes/a.txt",
        })),
        file: HistoryFileId::parse(&id("file", 1)).unwrap(),
        root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
        context: HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
            history_tree_id: HistoryTreeId::parse(&id("history", 1)).unwrap(),
            path: Some(name_policy::PortablePath::parse("notes/a.txt").unwrap()),
            current_node_id: None,
            selected_branch_id: None,
        },
    }
}

fn event(number: u64, action: Action) -> Event {
    Event {
        operation_id: id("op", number).parse().unwrap(),
        action,
        sequence: number,
        // Repeated delete/undo operations may refer to the same deletion.
        deletion_id: id("fdel", 1).parse().unwrap(),
        occurred_at: (action == Action::Delete).then_some(Timestamp::from_unix_seconds(123)),
    }
}

fn page(target: &Target, first: u64, count: u64, more: bool) -> Page {
    let request = target.request(None).unwrap();
    let events = (first..first + count)
        .map(|number| event(number, Action::Delete))
        .collect::<Vec<_>>();
    let observation_sequence = 100;
    let next_cursor = more.then(|| Cursor {
        binding: request.binding(&target.context, observation_sequence),
        after_operation_id: events.last().unwrap().operation_id,
        after_sequence: events.last().unwrap().sequence,
    });
    Page {
        file_id: target.file.clone(),
        history_root: target.root,
        context: target.context.clone(),
        observation_sequence,
        events,
        next_cursor,
    }
}

fn failed() -> api::Error {
    api::Error {
        status: 409,
        code: storage_contract::ErrorCode::HistoryChanged,
        message: "source changed".into(),
        retryable: false,
        request_id: Some("lifecycle-read".into()),
        source: None,
    }
}

#[test]
fn lifecycle_request_keeps_location_identity_and_replaces_one_bounded_page() {
    let target = target();
    let mut model = Model::default();
    model.set_target(Some(target.clone()));
    let request = model.request(false).unwrap().unwrap();
    let url = endpoint(&request);
    assert!(url.starts_with(&format!(
        "/api/file-manager/files/{}/history/lifecycle?", target.file,
    )));
    assert!(url.contains("path=notes%2Fa.txt"));
    assert!(url.contains(&format!("repository_id={}", id("repo", 1))));
    assert!(url.contains("limit=16"));
    assert!(model.request(true).unwrap().is_none());
    let generation = model.begin();
    assert!(model.request(false).unwrap().is_none());
    assert!(model.request(true).unwrap().is_none());
    let first = page(&target, 1, PAGE_SIZE as u64, true);
    let admitted = decode(json_codec::to_value(&first).unwrap(), &request, &target.context).unwrap();
    model.complete(&target, generation, Ok(admitted));
    let next = model.request(true).unwrap().unwrap();
    assert_eq!(next.cursor, first.next_cursor);
    assert!(endpoint(&next).contains("cursor="));
    let generation = model.begin();
    let last = page(&target, 17, 1, false);
    let admitted = decode(json_codec::to_value(&last).unwrap(), &next, &target.context).unwrap();
    model.complete(&target, generation, Ok(admitted));
    assert_eq!(model.page.as_ref().unwrap().events, last.events);
    assert!(model.request(true).unwrap().is_none());
    assert!(model.request(false).unwrap().unwrap().cursor.is_none());
}

#[test]
fn lifecycle_close_and_superseding_reads_reject_late_success_and_failure() {
    let target = target();
    let mut model = Model::default();
    model.set_target(Some(target.clone()));
    let closed = model.begin();
    model.close();
    model.complete(&target, closed, Ok(page(&target, 1, 1, false)));
    assert!(model.page.is_none());
    let current = model.begin();
    model.complete(&target, closed, Err(failed()));
    assert!(model.loading && model.error.is_none());
    model.complete(&target, current, Ok(page(&target, 2, 1, false)));
    assert!(!model.loading);
    assert_eq!(model.page.as_ref().unwrap().events[0].sequence, 2);
    model.complete(&target, closed, Ok(page(&target, 1, 1, false)));
    assert_eq!(model.page.as_ref().unwrap().events[0].sequence, 2);
}

#[test]
fn lifecycle_target_changes_clear_records_and_fence_old_completions() {
    let original = target();
    let mut changes = Vec::new();
    let mut changed = original.clone();
    changed.epoch += 1;
    changes.push(changed);
    let mut changed = original.clone();
    changed.navigation += 1;
    changes.push(changed);
    let mut changed = original.clone();
    changed.root = HistoryRoot::parse(&"b".repeat(64)).unwrap();
    changes.push(changed);
    let mut changed = original.clone();
    changed.context.repository_id = HistoryRepositoryId::parse(&id("repo", 2)).unwrap();
    changes.push(changed);
    let mut changed = original.clone();
    test_entries::update(&mut changed.entry, json!({"backup":true}));
    changes.push(changed);
    let mut changed = original.clone();
    test_entries::update(&mut changed.entry, json!({"entry_id":id("entry", 2)}));
    changes.push(changed);
    for changed in changes {
        let mut model = Model::default();
        model.set_target(Some(original.clone()));
        let generation = model.begin();
        model.complete(&original, generation, Ok(page(&original, 1, 1, false)));
        model.set_target(Some(changed));
        model.complete(&original, generation, Err(failed()));
        assert!(model.page.is_none() && model.error.is_none() && !model.loading);
    }
}

#[test]
fn lifecycle_read_error_keeps_the_actual_error_and_is_not_an_empty_success() {
    let target = target();
    let mut model = Model::default();
    model.set_target(Some(target.clone()));
    let generation = model.begin();
    model.complete(&target, generation, Ok(page(&target, 1, 1, false)));
    let generation = model.begin();
    model.complete(&target, generation, Err(failed()));
    assert!(model.page.is_none() && !model.loading);
    let error = model.error.as_ref().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::HistoryChanged);
    assert_eq!(error.request_id.as_deref(), Some("lifecycle-read"));
    assert!(model.request(true).unwrap().is_none());
    assert!(model.request(false).unwrap().unwrap().cursor.is_none());
}

#[test]
fn lifecycle_response_is_bound_to_the_loaded_file_root_and_resolved_context() {
    let target = target();
    let request = target.request(None).unwrap();
    let page = page(&target, 1, 1, false);
    let wire = json_codec::to_value(page).unwrap();
    assert!(decode(wire.clone(), &request, &target.context).is_ok());
    for (field, value) in [
        ("file_id", id("file", 2)),
        ("history_root", "b".repeat(64)),
        ("write_target", id("file", 2)),
    ] {
        let mut bad = wire.clone();
        bad[field] = value.into();
        assert!(decode(bad, &request, &target.context).is_err());
    }
    let mut crossed = wire.clone();
    crossed["context"]["history_tree_id"] = id("history", 2).into();
    assert!(decode(crossed, &request, &target.context).is_err());
    let mut missing = wire;
    missing["events"][0].as_object_mut().unwrap().remove("occurred_at");
    assert!(decode(missing, &request, &target.context).is_err());
}

#[test]
fn lifecycle_retained_history_request_does_not_forge_a_current_location() {
    let mut target = target();
    let retained = storage_contract::detached_history::File {
        file_id: target.file.clone(),
        repository_id: target.context.repository_id.clone(),
        history_tree_id: target.context.history_tree_id.clone(),
        history_root: target.root,
        representative_version_id: id("ver", 1).parse().unwrap(),
        recorded_name: "old.txt".into(),
        representative_size: 1,
        retained_versions: 1,
    };
    target.entry = Entry::retained(&retained);
    target.context.path = None;
    let request = target.request(None).unwrap();
    assert!(request.location.is_none());
    assert!(!endpoint(&request).contains("old.txt"));
    assert!(!endpoint(&request).contains("repository_id="));
}

#[test]
fn lifecycle_capture_requires_an_active_matching_history_and_navigation_generation() {
    let selected = target();
    let mut state = State {
        authenticated: true,
        session_epoch: selected.epoch,
        ..Default::default()
    };
    state.files.selected = Some(selected.entry);
    state.files.file_history = history::tests::history(json!({}));
    let before = Target::capture(&state).unwrap();
    state.files.cancel_listing();
    assert_ne!(Target::capture(&state).unwrap(), before);
    state.files.file_history.invalidate();
    assert!(Target::capture(&state).is_none());
    state.files.file_history = history::tests::history(json!({}));
    state.authenticated = false;
    assert!(Target::capture(&state).is_none());
    state.authenticated = true;
    test_entries::update(state.files.selected.as_mut().unwrap(), json!({"file_id":id("file", 2)}));
    assert!(Target::capture(&state).is_none());
}

#[test]
fn lifecycle_operation_identity_and_unknown_times_are_not_deletion_or_node_aliases() {
    let deleted = event(1, Action::Delete);
    let restored = event(2, Action::Undo);
    let imported = event(3, Action::Import);
    assert_eq!(deleted.deletion_id, restored.deletion_id);
    assert_ne!(deleted.operation_id, restored.operation_id);
    assert_eq!(event_time(&deleted), "1970-01-01T00:02:03Z");
    assert_eq!(event_time(&restored), t("时间未记录", "Time not recorded"));
    assert_eq!(event_time(&imported), t("时间未记录", "Time not recorded"));
    assert_ne!(action_label(imported.action), action_label(deleted.action));
    let mut beyond_calendar = deleted;
    beyond_calendar.occurred_at = Some(Timestamp::from_unix_seconds(u64::MAX));
    assert!(event_time(&beyond_calendar).ends_with(&u64::MAX.to_string()));
}

#[test]
fn lifecycle_incomplete_source_is_an_error_not_an_empty_or_final_page() {
    let target = target();
    let mut model = Model::default();
    model.set_target(Some(target.clone()));
    let generation = model.begin();
    model.complete(&target, generation, Ok(page(&target, 1, PAGE_SIZE as u64, true)));
    let generation = model.begin();
    let mut error = failed();
    error.code = storage_contract::ErrorCode::PublicationInvalid;
    error.status = 500;
    error.message = "retained lifecycle evidence is incomplete".into();
    model.complete(&target, generation, Err(error));
    assert!(model.page.is_none());
    assert_eq!(model.error.as_ref().unwrap().code, storage_contract::ErrorCode::PublicationInvalid);
    assert!(model.request(true).unwrap().is_none());
}
