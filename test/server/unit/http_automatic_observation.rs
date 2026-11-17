use super::*;
use storage_contract::{
    EntryId, Principal, VersionId,
    automatic_observation::{Request, Scope},
    snapshot_worktree::NamespaceDirectoryId,
};

fn base() -> Query {
    Query {
        path: None,
        start: Timestamp::from_unix_seconds(100),
        end: Timestamp::from_unix_seconds(160),
        observed: None,
        last_file: None,
        last_node: None,
        binding: None,
    }
}

#[test]
fn first_or_exact_continuation_never_downgrades_a_partial_query() {
    let repository = RepositoryId::from_random([7; 16]);
    let file = FileId::from_random([4; 16]);
    let node = FileNodeId::Version(VersionId::from_random([1; 16]));
    let position = Position::After {
        observed: Sequence::new(12),
        last: NodeKey { file, node },
    };
    let binding = Binding::for_session(
        &"alice".parse::<Principal>().unwrap(),
        "login",
        &Request {
            scope: Scope {
                repository,
                directory: NamespaceDirectoryId::Entry(EntryId::from_random([2; 16])),
                start: base().start,
                end: base().end,
            },
            position,
        },
    )
    .unwrap();
    assert_eq!(
        base().request(repository).unwrap().position,
        Position::First
    );
    for mask in 1..16 {
        let query = Query {
            observed: (mask & 1 != 0).then_some(Sequence::new(12)),
            last_file: (mask & 2 != 0).then_some(file),
            last_node: (mask & 4 != 0).then_some(node),
            binding: (mask & 8 != 0).then_some(binding),
            ..base()
        };
        let reply = query.request(repository);
        if mask == 15 {
            assert_eq!(reply.unwrap().position, position);
        } else {
            assert!(reply.is_err());
        }
    }
    assert!(
        Query {
            end: base().start,
            ..base()
        }
        .request(repository)
        .is_err()
    );
    assert!(
        Query {
            observed: Some(Sequence::new(0)),
            last_file: Some(file),
            last_node: Some(node),
            binding: Some(binding),
            ..base()
        }
        .request(repository)
        .is_err()
    );
}
