use crate::file_lifecycle::{
    Cursor, Detail, Event, Page, PathLocation, Request, SourceId, Timestamp,
};
use crate::history_window::{
    HistoryFileId, HistoryRepositoryId, HistoryResolvedContext, HistoryTreeId,
};

fn request(limit: usize) -> Request {
    Request::parse(&format!("file_{:032x}", 1), &"a".repeat(64), None, None, None, limit).unwrap()
}

fn location(path: &str) -> PathLocation {
    PathLocation {
        repository_id: format!("repo_{:032x}", 1).parse().unwrap(),
        path: name_policy::PortablePath::parse(path).unwrap(),
    }
}

fn deletion() -> Event {
    Event {
        operation_id: format!("op_{:032x}", 1).parse().unwrap(),
        sequence: 1,
        detail: Detail::Delete {
            deletion_id: format!("fdel_{:032x}", 1).parse().unwrap(),
        },
        occurred_at: Some(Timestamp::from_unix_seconds(100)),
    }
}

fn path_change(id: u8) -> Event {
    Event {
        operation_id: format!("op_{:032x}", 2).parse().unwrap(),
        sequence: 2,
        detail: Detail::PathChanged {
            event_id: format!("fpath_{id:032x}").parse().unwrap(),
            before: location("before.txt"),
            after: location("after.txt"),
        },
        occurred_at: Some(Timestamp::from_unix_seconds(100)),
    }
}

fn page(request: &Request) -> Page {
    Page {
        file_id: request.file_id.clone(),
        history_root: request.history_root,
        context: HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&format!("repo_{:032x}", 1)).unwrap(),
            history_tree_id: HistoryTreeId::parse(&format!("history_{:032x}", 1)).unwrap(),
            path: None,
            current_node_id: None,
            selected_branch_id: None,
        },
        observation_sequence: 3,
        events: vec![deletion()],
        next_cursor: None,
    }
}

fn cursor(request: &Request, page: &Page) -> Cursor {
    let event = page.events.last().unwrap();
    Cursor {
        binding: request.binding(&page.context, page.observation_sequence),
        after_operation_id: event.operation_id,
        after_sequence: event.sequence,
        after_source_id: event.source_id(),
    }
}

#[test]
fn file_lifecycle_request_response_and_cursor_are_bounded_and_bound() {
    let request = request(1);
    crate::validate_api_path(&request.route()).unwrap();
    for limit in [0, 65, usize::MAX] {
        assert!(Request::parse(
            request.file_id.as_str(), &request.history_root.to_string(),
            None, None, None, limit,
        ).is_err());
    }
    assert!(Request::parse(
        request.file_id.as_str(), &request.history_root.to_string(),
        Some("repo_bad"), None, None, 1,
    ).is_err());
    let mut page = page(&request);
    page.validate(&request).unwrap();
    let cursor = cursor(&request, &page);
    assert_eq!(Cursor::parse(&cursor.to_string()).unwrap(), cursor);
    assert!(Cursor::parse(&format!("{cursor}x")).is_err());
    page.next_cursor = Some(cursor.clone());
    page.validate(&request).unwrap();
    let bytes = json_codec::to_vec(&page).unwrap();
    assert_eq!(json_codec::from_slice::<Page>(&bytes).unwrap(), page);
    let mut replay_request = request.clone();
    replay_request.cursor = Some(cursor.clone());
    let mut replay = page.clone();
    replay.next_cursor = None;
    assert!(replay.validate(&replay_request).is_err());
    let mut crossed = request.clone();
    crossed.cursor = Some(cursor);
    crossed.file_id = HistoryFileId::parse(&format!("file_{:032x}", 2)).unwrap();
    assert!(page.validate(&crossed).is_err());
    page.events[0].detail = Detail::Undo {
        deletion_id: format!("fdel_{:032x}", 1).parse().unwrap(),
    };
    assert!(page.validate(&request).is_err());
    page.events[0].occurred_at = None;
    page.validate(&request).unwrap();
    page.observation_sequence = 0;
    assert!(page.validate(&request).is_err());
}

#[test]
fn one_operation_can_page_multiple_real_path_facts_without_skipping_aliases() {
    let request = request(2);
    let mut first = page(&request);
    first.events = vec![path_change(1)];
    first.next_cursor = Some(cursor(&request, &first));
    first.validate(&request).unwrap();
    let mut next_request = request.clone();
    next_request.cursor = first.next_cursor.clone();
    let mut next = first.clone();
    next.events = vec![path_change(2), path_change(3)];
    next.next_cursor = None;
    next.validate(&next_request).unwrap();
    next.events.reverse();
    assert!(next.validate(&next_request).is_err());
    next.events = vec![path_change(2), path_change(2)];
    assert!(next.validate(&next_request).is_err());
    next.events[1].sequence = 3;
    assert!(next.validate(&next_request).is_err());
    next.events = vec![path_change(1)];
    assert!(next.validate(&next_request).is_err());
    next.events[0].sequence = 3;
    assert!(next.validate(&next_request).is_err());
    next.events = vec![];
    next.next_cursor = first.next_cursor;
    assert!(next.validate(&next_request).is_err());
}

#[test]
fn cursor_requires_the_actual_record_domain_and_rejects_prior_encoding() {
    let request = request(1);
    let page = page(&request);
    let cursor = cursor(&request, &page);
    let prefix = format!("{}:{}", cursor.binding, cursor.after_operation_id);
    for invalid in [
        format!("{prefix}:1"),
        format!("{prefix}:0:{}", cursor.after_source_id),
        format!("{prefix}:01:{}", cursor.after_source_id),
        format!("{prefix}:1:ver_{:032x}", 1),
        format!("{prefix}:1:fnode_{:032x}", 1),
        format!("{prefix}:1:fpath_bad"),
        format!("{prefix}:1:{}:extra", cursor.after_source_id),
    ] {
        assert!(Cursor::parse(&invalid).is_err(), "{invalid}");
    }
    let path = path_change(1);
    assert!(path.source_id().to_string().parse::<crate::FileNodeId>().is_err());
    assert!(SourceId::Deletion(format!("fdel_{:032x}", 1).parse().unwrap()) < path.source_id());
    assert_eq!(SourceId::parse(&path.source_id().to_string()).unwrap(), path.source_id());
}

#[test]
fn path_facts_require_a_real_change_and_known_time_without_guessing_a_move_kind() {
    let request = request(1);
    let mut page = page(&request);
    page.events = vec![path_change(1)];
    page.validate(&request).unwrap();
    page.events[0].occurred_at = None;
    assert!(page.validate(&request).is_err());
    page.events[0].occurred_at = Some(Timestamp::from_unix_seconds(100));
    let Detail::PathChanged { before, after, .. } = &mut page.events[0].detail else {
        unreachable!();
    };
    *after = before.clone();
    assert!(page.validate(&request).is_err());
    let Detail::PathChanged { after, .. } = &mut page.events[0].detail else {
        unreachable!();
    };
    after.repository_id = format!("repo_{:032x}", 2).parse().unwrap();
    page.validate(&request).unwrap();
    page.events[0].detail = Detail::Import {
        deletion_id: format!("fdel_{:032x}", 1).parse().unwrap(),
    };
    assert!(page.validate(&request).is_err());
    page.events[0].occurred_at = None;
    page.validate(&request).unwrap();
}

#[test]
fn event_wire_is_strict_and_does_not_accept_the_previous_flat_shape() {
    let event = path_change(1);
    let value = json_codec::to_value(&event).unwrap();
    assert_eq!(json_codec::from_value::<Event>(value.clone()).unwrap(), event);
    for (field, extra) in [
        ("deletion_id", json_codec::json!(format!("fdel_{:032x}", 1))),
        ("ignored", json_codec::json!(true)),
    ] {
        let mut malformed = value.clone();
        malformed["detail"][field] = extra;
        assert!(json_codec::from_value::<Event>(malformed).is_err());
    }
    let mut malformed = value.clone();
    malformed["detail"]["before"]["path"] = json_codec::json!("../outside");
    assert!(json_codec::from_value::<Event>(malformed).is_err());
    let mut malformed = value;
    malformed.as_object_mut().unwrap().remove("occurred_at");
    assert!(json_codec::from_value::<Event>(malformed).is_err());
    let mut prior = json_codec::to_value(deletion()).unwrap();
    let detail = prior.as_object_mut().unwrap().remove("detail").unwrap();
    prior["action"] = detail["action"].clone();
    prior["deletion_id"] = detail["deletion_id"].clone();
    assert!(json_codec::from_value::<Event>(prior).is_err());
}
