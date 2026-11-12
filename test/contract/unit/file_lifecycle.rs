use crate::file_lifecycle::{Action, Cursor, Event, Page, Request, Timestamp};
use crate::history_window::{HistoryRepositoryId, HistoryResolvedContext, HistoryTreeId};

fn request() -> Request {
    Request::parse(&format!("file_{:032x}", 1), &"a".repeat(64), None, None, None, 1).unwrap()
}
fn page(request: &Request) -> Page {
    Page {
        file_id: request.file_id.clone(), history_root: request.history_root,
        context: HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&format!("repo_{:032x}", 1)).unwrap(),
            history_tree_id: HistoryTreeId::parse(&format!("history_{:032x}", 1)).unwrap(),
            path: None, current_node_id: None, selected_branch_id: None,
        },
        observation_sequence: 3,
        events: vec![Event {
            operation_id: format!("op_{:032x}", 1).parse().unwrap(), action: Action::Delete,
            sequence: 1, deletion_id: format!("fdel_{:032x}", 1).parse().unwrap(),
            occurred_at: Some(Timestamp::from_unix_seconds(100)),
        }],
        next_cursor: None,
    }
}
#[test]
fn file_lifecycle_request_response_and_cursor_are_bounded_and_bound() {
    let request = request();
    crate::validate_api_path(&request.route()).unwrap();
    for limit in [0, 65, usize::MAX] {
        assert!(Request::parse(request.file_id.as_str(), &request.history_root.to_string(),
            None, None, None, limit).is_err());
    }
    assert!(Request::parse(request.file_id.as_str(), &request.history_root.to_string(),
        Some("repo_bad"), None, None, 1).is_err());
    let mut page = page(&request);
    page.validate(&request).unwrap();
    let cursor = Cursor {
        binding: request.binding(&page.context, page.observation_sequence),
        after_operation_id: page.events[0].operation_id,
        after_sequence: page.events[0].sequence,
    };
    assert_eq!(Cursor::parse(&cursor.to_string()).unwrap(), cursor);
    assert!(Cursor::parse(&format!("{}x", cursor)).is_err());
    page.next_cursor = Some(cursor.clone());
    page.validate(&request).unwrap();
    let bytes = json_codec::to_vec(&page).unwrap();
    let decoded: Page = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(decoded, page);
    let mut replay_request = request.clone();
    replay_request.cursor = Some(cursor.clone());
    let mut replay = page.clone();
    replay.events[0].operation_id = format!("op_{:032x}", 2).parse().unwrap();
    replay.next_cursor = None;
    assert!(replay.validate(&replay_request).is_err());
    let mut crossed = request.clone();
    crossed.cursor = Some(cursor);
    crossed.file_id = crate::history_window::HistoryFileId::parse(
        &format!("file_{:032x}", 2),
    ).unwrap();
    assert!(page.validate(&crossed).is_err());
    page.events[0].action = Action::Undo;
    assert!(page.validate(&request).is_err());
    page.events[0].occurred_at = None;
    page.validate(&request).unwrap();
    page.observation_sequence = 0;
    assert!(page.validate(&request).is_err());
}
