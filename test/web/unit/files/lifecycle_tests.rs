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

fn deletion(number: u64) -> Event {
    event(
        number,
        Detail::Delete {
            deletion_id: id("fdel", 1).parse().unwrap(),
        },
    )
}

fn event(number: u64, detail: Detail) -> Event {
    let occurred_at = matches!(&detail, Detail::Delete { .. } | Detail::PathChanged { .. })
        .then_some(Timestamp::from_unix_seconds(123));
    Event {
        operation_id: id("op", number).parse().unwrap(),
        sequence: number,
        detail,
        occurred_at,
    }
}

fn path_event(number: u64, source: u64) -> Event {
    event(
        number,
        Detail::PathChanged {
            event_id: id("fpath", source).parse().unwrap(),
            before: PathLocation {
                repository_id: id("repo", 1).parse().unwrap(),
                path: name_policy::PortablePath::parse("old/a.txt").unwrap(),
            },
            after: PathLocation {
                repository_id: id("repo", 2).parse().unwrap(),
                path: name_policy::PortablePath::parse("new/b.txt").unwrap(),
            },
        },
    )
}

fn page(target: &Target, first: u64, count: u64, more: bool) -> Page {
    let request = target.request(None).unwrap();
    let events = (first..first + count).map(deletion).collect::<Vec<_>>();
    let observation_sequence = 100;
    let next_cursor = more.then(|| Cursor {
        binding: request.binding(&target.context, observation_sequence),
        after_operation_id: events.last().unwrap().operation_id,
        after_sequence: events.last().unwrap().sequence,
        after_source_id: events.last().unwrap().source_id(),
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
fn lifecycle_decoder_rejects_an_oversized_valid_page_within_the_transport_cap() {
    let target = target();
    // Exercise the shared endpoint's maximum, not the UI's smaller 16-row request.
    let request = Request::parse(
        target.file.as_str(),
        &target.root.to_string(),
        Some(target.context.repository_id.as_str()),
        Some("notes/a.txt"),
        None,
        64,
    )
    .unwrap();
    let path = ["界".repeat(60), "界".repeat(60), "界".repeat(60)].join("/");
    let path = name_policy::PortablePath::parse(&path).unwrap();
    let mut response = page(&target, 1, 1, false);
    response.events = (1..=64)
        .map(|number| {
            let mut event = path_event(number, number);
            let Detail::PathChanged { before, after, .. } = &mut event.detail else {
                panic!("fixture must contain path events");
            };
            before.path = path.clone();
            after.path = path.clone();
            event
        })
        .collect();
    let encoded = json_codec::to_vec(&response).unwrap();
    assert!(encoded.len() > storage_contract::file_lifecycle::MAX_RESPONSE_BYTES);
    assert!(encoded.len() < storage_contract::client_protocol::MAX_RESPONSE_BYTES);
    let error = decode(
        json_codec::to_value(&response).unwrap(),
        &request,
        &target.context,
    )
    .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);

    response.events.truncate(1);
    assert!(decode(
        json_codec::to_value(&response).unwrap(),
        &request,
        &target.context,
    )
    .is_ok());
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
    let deleted = deletion(1);
    let deletion_id = id("fdel", 1).parse().unwrap();
    let restored = event(2, Detail::Undo { deletion_id });
    let imported = event(3, Detail::Import { deletion_id });
    assert_eq!(deleted.source_id(), restored.source_id());
    assert_ne!(deleted.operation_id, restored.operation_id);
    assert_eq!(event_time(&deleted), "1970-01-01T00:02:03Z");
    assert_eq!(event_time(&restored), t("时间未记录", "Time not recorded"));
    assert_eq!(event_time(&imported), t("时间未记录", "Time not recorded"));
    assert_ne!(action_label(&imported.detail), action_label(&deleted.detail));
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
    assert_eq!(
        model.error.as_ref().unwrap().code,
        storage_contract::ErrorCode::PublicationInvalid,
    );
    assert!(model.request(true).unwrap().is_none());
}

#[test]
fn lifecycle_path_members_share_an_operation_without_sharing_cards_or_cursors() {
    let target = target();
    let request = target.request(None).unwrap();
    let first = path_event(10, 1);
    let second = path_event(10, 2);
    assert_eq!(first.operation_id, second.operation_id);
    assert_ne!(event_identity(&first), event_identity(&second));
    assert_ne!(
        event_anchor(&target.file, &first),
        event_anchor(&target.file, &second),
    );
    assert_eq!(action_label(&first.detail), t("路径变更", "Path changed"));
    assert_eq!(event_time(&first), "1970-01-01T00:02:03Z");
    let Detail::PathChanged { before, after, .. } = &first.detail else {
        panic!("fixture is a path record");
    };
    assert_eq!(location_label(before), format!("{} / old/a.txt", id("repo", 1)));
    assert_eq!(location_label(after), format!("{} / new/b.txt", id("repo", 2)));

    let mut first_page = page(&target, 1, 1, false);
    first_page.events = vec![first.clone()];
    first_page.next_cursor = Some(Cursor {
        binding: request.binding(&target.context, first_page.observation_sequence),
        after_operation_id: first.operation_id,
        after_sequence: first.sequence,
        after_source_id: first.source_id(),
    });
    // A short nonempty page may continue at another member of the same operation.
    let admitted = decode(
        json_codec::to_value(&first_page).unwrap(),
        &request,
        &target.context,
    )
    .unwrap();
    assert_eq!(
        page_status(&admitted),
        t("后面还有保留记录。", "More retained records are available."),
    );
    let mut model = Model::default();
    model.set_target(Some(target.clone()));
    let generation = model.begin();
    model.complete(&target, generation, Ok(admitted));
    let next = model.request(true).unwrap().unwrap();
    assert_eq!(next.cursor, first_page.next_cursor);
    assert!(endpoint(&next).contains(&format!("%3A{}", first.source_id())));
    let mut last_page = page(&target, 1, 1, false);
    last_page.events = vec![second.clone()];
    let admitted = decode(
        json_codec::to_value(&last_page).unwrap(),
        &next,
        &target.context,
    )
    .unwrap();
    let generation = model.begin();
    model.complete(&target, generation, Ok(admitted));
    assert_eq!(model.page.as_ref().unwrap().events, vec![second]);
    assert!(model.request(true).unwrap().is_none());

    let mut repeated = last_page;
    repeated.events = vec![first];
    assert!(decode(
        json_codec::to_value(&repeated).unwrap(),
        &next,
        &target.context,
    )
    .is_err());
    repeated.events[0].sequence += 1;
    assert!(decode(json_codec::to_value(repeated).unwrap(), &next, &target.context).is_err());
    let mut wrong_source = first_page;
    wrong_source.next_cursor.as_mut().unwrap().after_source_id =
        SourceId::PathChange(id("fpath", 3).parse().unwrap());
    assert!(decode(
        json_codec::to_value(wrong_source).unwrap(),
        &request,
        &target.context,
    )
    .is_err());
}

#[test]
fn lifecycle_path_evidence_rejects_old_wire_and_forged_times_or_locations() {
    let target = target();
    let request = target.request(None).unwrap();
    let mut response = page(&target, 1, 1, false);
    response.events = vec![path_event(1, 1)];
    let wire = json_codec::to_value(&response).unwrap();
    assert!(decode(wire.clone(), &request, &target.context).is_ok());
    for bad_detail in [
        json!({
            "action":"path_changed", "event_id":id("fmeta", 1),
            "before":{"repository_id":id("repo", 1),"path":"a.txt"},
            "after":{"repository_id":id("repo", 1),"path":"b.txt"},
        }),
        json!({
            "action":"path_changed", "event_id":id("fpath", 1),
            "before":{"repository_id":id("repo", 1),"path":"a.txt"},
            "after":{"repository_id":id("repo", 1),"path":"a.txt"},
        }),
        json!({
            "action":"path_changed", "event_id":id("fpath", 1),
            "before":{"repository_id":id("repo", 1),"path":"../a.txt"},
            "after":{"repository_id":id("repo", 1),"path":"b.txt"},
        }),
        json!({"action":"undo", "deletion_id":id("fdel", 1)}),
        json!({"action":"import", "deletion_id":id("fdel", 1)}),
    ] {
        let mut invalid = wire.clone();
        invalid["events"][0]["detail"] = bad_detail;
        assert!(decode(invalid, &request, &target.context).is_err());
    }
    let mut no_time = wire.clone();
    no_time["events"][0]["occurred_at"] = json_codec::Value::Null;
    assert!(decode(no_time, &request, &target.context).is_err());
    let mut unknown = wire.clone();
    unknown["events"][0]["detail"]["rename"] = true.into();
    assert!(decode(unknown, &request, &target.context).is_err());
    let mut prior_wire = wire;
    prior_wire["events"][0] = json!({
        "operation_id":id("op", 1), "sequence":"1", "occurred_at":"123",
        "action":"delete", "deletion_id":id("fdel", 1),
    });
    assert!(decode(prior_wire, &request, &target.context).is_err());
    let prior_cursor = format!("{}:{}:1", "a".repeat(64), id("op", 1));
    assert!(target.request(Some(&prior_cursor)).is_err());
}
