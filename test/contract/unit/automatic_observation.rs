use super::*;
use crate::{EntryId, VersionId};

fn request() -> Request {
    let repository = RepositoryId::from_random([3; 16]);
    Request {
        scope: Scope {
            repository,
            directory: NamespaceDirectoryId::Repository(repository),
            start: Timestamp::from_unix_seconds(100),
            end: Timestamp::from_unix_seconds(160),
        },
        position: Position::First,
    }
}
fn row(index: u8) -> Row {
    Row {
        key: NodeKey {
            file: FileId::from_random([index / 4; 16]),
            node: FileNodeId::Version(VersionId::from_random([index; 16])),
        },
        operation: OperationId::from_random([index; 16]),
        sequence: Sequence::new(u64::from(index) + 1),
        published_at: Timestamp::from_unix_seconds(100),
    }
}
fn page(count: u8) -> Page {
    Page {
        request: request(),
        observed: Sequence::new(100),
        rows: (0..count).map(row).collect(),
        next: None,
    }
}

#[test]
fn cursor_binds_account_scope_interval_sequence_and_last_stable_pair() {
    let principal: Principal = "alice".parse().unwrap();
    let mut first = page(64);
    let last = first.rows.last().unwrap().key;
    first.next = Some(last);
    let next = first.continuation().unwrap();
    let original = Binding::for_session(&principal, "admitted-one", &next).unwrap();
    assert_eq!(original.to_string().parse::<Binding>().unwrap(), original);
    assert_ne!(
        original,
        Binding::for_session(&principal, "relogin", &next).unwrap()
    );
    assert!(
        original
            .to_string()
            .to_uppercase()
            .parse::<Binding>()
            .is_err()
    );
    let mut other_dir = next;
    other_dir.scope.directory = NamespaceDirectoryId::Entry(EntryId::from_random([1; 16]));
    let mut other_time = next;
    other_time.scope.end = Timestamp::from_unix_seconds(161);
    let mut other_sequence = next;
    other_sequence.position = Position::After {
        observed: Sequence::new(101),
        last,
    };
    let mut other_last = next;
    other_last.position = Position::After {
        observed: Sequence::new(100),
        last: row(4).key,
    };
    for changed in [other_dir, other_time, other_sequence, other_last, request()] {
        assert_ne!(
            Binding::for_session(&principal, "admitted-one", &changed).unwrap(),
            original
        );
    }
    assert_ne!(
        Binding::for_session(&"bob".parse().unwrap(), "admitted-one", &next).unwrap(),
        original
    );
}

#[test]
fn exact_bounded_rows_preserve_half_open_interval_unique_identity_and_continuity() {
    let mut first = page(64);
    first.next = Some(first.rows.last().unwrap().key);
    first.validate(&request()).unwrap();
    let next = first.continuation().unwrap();
    let mut second = Page {
        request: next,
        observed: first.observed,
        rows: vec![row(64)],
        next: None,
    };
    second.validate(&next).unwrap();
    assert!(second.validate(&request()).is_err());
    second.observed = Sequence::new(101);
    assert!(second.validate(&next).is_err());
    second.observed = first.observed;
    second.rows[0] = row(63);
    assert!(second.validate(&next).is_err());
    second.rows[0] = row(64);
    for seconds in [99, 160] {
        second.rows[0].published_at = Timestamp::from_unix_seconds(seconds);
        assert!(second.validate(&next).is_err());
    }
    second.rows[0].published_at = Timestamp::from_unix_seconds(159);
    second.validate(&next).unwrap();
    second.rows[0].sequence = Sequence::new(101);
    assert!(second.validate(&next).is_err());
    first.rows.swap(0, 1);
    assert!(first.validate(&request()).is_err());
}

#[test]
fn over_budget_missing_or_forged_continuation_is_never_a_complete_page() {
    let too_large = json_codec::to_value(page(65)).unwrap();
    assert!(json_codec::from_value::<Page>(too_large).is_err());
    let mut missing = json_codec::to_value(page(2)).unwrap();
    missing.as_object_mut().unwrap().remove("next");
    assert!(json_codec::from_value::<Page>(missing).is_err());
    let mut short = page(2);
    short.next = Some(short.rows.last().unwrap().key);
    assert!(short.validate(&request()).is_err());
    let mut full = page(64);
    full.next = Some(full.rows[5].key);
    assert!(full.validate(&request()).is_err());
    let mut bad = request();
    bad.scope.end = bad.scope.start;
    assert!(bad.validate().is_err());
    let mut bad = request();
    bad.scope.directory = NamespaceDirectoryId::Repository(RepositoryId::from_random([9; 16]));
    assert!(bad.validate().is_err());
}
