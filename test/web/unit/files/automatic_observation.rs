use super::*;
use storage_contract::{
    EntryId, FileNodeId, OperationId, Sequence, VersionId,
    automatic_observation::{NodeKey, Page, Row, Scope},
};

fn target(path: Option<&str>) -> Target {
    Target {
        principal: "alice".parse().unwrap(),
        epoch: 1,
        navigation: 7,
        slot: "A".into(),
        repository: RepositoryId::from_random([3; 16]),
        path: path.map(|p| PortablePath::parse(p).unwrap()),
    }
}
fn sample(nested: bool) -> (Window, Reply) {
    let t = target(nested.then_some("folder"));
    let start = Timestamp::from_unix_seconds(100);
    let end = Timestamp::from_unix_seconds(160);
    let scope = Scope {
        repository: t.repository,
        directory: if nested {
            NamespaceDirectoryId::Entry(EntryId::from_random([1; 16]))
        } else {
            NamespaceDirectoryId::Repository(t.repository)
        },
        start,
        end,
    };
    let request = Request {
        scope,
        position: Position::First,
    };
    let observed = Sequence::new(500);
    let row = |i: u8| Row {
        key: NodeKey {
            file: FileId::from_random([1; 16]),
            node: FileNodeId::Version(VersionId::from_random([i; 16])),
        },
        operation: OperationId::from_random([i; 16]),
        sequence: Sequence::new(u64::from(i) + 1),
        published_at: start,
    };
    let rows = (0..64).map(row).collect::<Vec<_>>();
    let page = Page {
        request,
        observed,
        next: Some(rows[63].key),
        rows,
    };
    let binding =
        Binding::for_session(&t.principal, "login", &page.continuation().unwrap()).unwrap();
    (
        Window {
            start,
            end,
            next: None,
        },
        Reply {
            path: t.path,
            page,
            next_binding: Some(binding),
        },
    )
}
#[test]
fn split_file_across_pages_counts_original_independent_nodes_once() {
    let t = target(Some("folder"));
    let (window, first) = sample(true);
    window.admit(&t, &first).unwrap();
    let next = first.page.continuation().unwrap();
    let binding = first.next_binding.unwrap();
    let mut last = first.clone();
    last.page.rows = vec![first.page.rows[0].clone(), first.page.rows[1].clone()];
    last.page.request = next;
    last.page.rows[0].key.node = FileNodeId::Version(VersionId::from_random([65; 16]));
    last.page.rows[1].key.file = FileId::from_random([2; 16]);
    last.page.next = None;
    last.next_binding = None;
    let second = Window {
        next: Some((next, binding)),
        ..window
    };
    second.admit(&t, &last).unwrap();
    let mut m = Model::default();
    m.apply(first).unwrap();
    m.apply(last.clone()).unwrap();
    assert_eq!((m.totals.files, m.totals.nodes, m.carried), (2, 66, 64));
    assert!(window.admit(&t, &last).is_err());
    assert!(second.admit(&target(Some("other")), &last).is_err());
    last.page.observed = Sequence::new(501);
    assert!(second.admit(&t, &last).is_err());
}
#[test]
fn resolved_path_and_original_window_must_match_before_any_count() {
    let (window, mut first) = sample(false);
    let t = target(None);
    window.admit(&t, &first).unwrap();
    assert!(window.admit(&target(Some("folder")), &first).is_err());
    first.page.request.scope.end = Timestamp::from_unix_seconds(161);
    assert!(window.admit(&t, &first).is_err());
    first.page.request.scope.end = window.end;
    first.next_binding = None;
    assert!(window.admit(&t, &first).is_err());
}
#[test]
fn utc_controls_never_convert_through_local_time_or_accept_invalid_calendar_days() {
    assert_eq!(
        calendar::parse("1970-01-01T00:00").unwrap().unix_seconds(),
        0
    );
    assert_eq!(
        calendar::parse("1970-01-02T00:00:01")
            .unwrap()
            .unix_seconds(),
        86401
    );
    assert!(calendar::parse("2024-02-29T12:05:03").is_some());
    for bad in [
        "1969-12-31T23:59:59",
        "2025-02-29T00:00",
        "2024-04-31T12:00",
        "2024-01-01T24:00",
        "2024-01-01T01:02Z",
        "2024-01-01T00:00:60",
        "٢٠٢٤-01-01T00:00",
    ] {
        assert!(calendar::parse(bad).is_none());
    }
}
