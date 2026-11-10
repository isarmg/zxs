use super::*;
use crate::{
    CommitState, ContentState, CurrentHealth, Event, FileDeletionNode, ProtectionAtPublish,
};
use json_codec::json;

const TABLES: [ChangedTable; 3] = [
    ChangedTable::ContentStates,
    ChangedTable::FileDeletions,
    ChangedTable::FilePathNotes,
];

fn id(number: u64) -> OperationId {
    format!("op_{number:032x}").parse().unwrap()
}

fn receipt(number: u64) -> OperationResult {
    OperationResult {
        operation_id: id(number),
        kind: "fixture.operation".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: "fixture".into(),
        sequence: number,
        result: json!({}),
    }
}

fn event(number: u64) -> Event {
    Event {
        sequence: number,
        operation_id: id(number),
        kind: "fixture.operation".into(),
        object_id: id(number).to_string(),
    }
}

fn previous() -> State {
    let mut state = State {
        sequence: 1,
        business_generation: 1,
        ..State::default()
    };
    state.operations.insert(id(1), receipt(1));
    state.operation_principals.insert(id(1), "alice".into());
    state.events.push(event(1));
    state
}

fn candidate(previous: &State) -> State {
    let mut state = previous.clone();
    state.begin_tracking();
    state.sequence = 2;
    state.business_generation = 2;
    state.operations.insert(id(2), receipt(2));
    state.operation_principals.insert(id(2), "bob".into());
    state.events.push(event(2));
    state
}

fn cold(state: &State) -> State {
    json_codec::from_slice(&crate::canonical_json(state).unwrap()).unwrap()
}

fn invalid() -> Error {
    Error::new(
        crate::ErrorCode::PublicationInvalid,
        "original domain error",
    )
}

fn rejects(previous: &State, next: &State) {
    for next in [next.clone(), cold(next)] {
        for table in TABLES {
            let error = verify(previous, &next, &receipt(2), table, invalid).unwrap_err();
            assert_eq!(error.code(), crate::ErrorCode::PublicationInvalid);
            assert_eq!(error.message(), "original domain error");
        }
    }
}

fn change_table(state: &mut State, table: ChangedTable) {
    match table {
        ChangedTable::ContentStates => {
            state
                .content_states
                .insert("version".into(), ContentState::Retired);
        }
        ChangedTable::FileDeletions => {
            state.file_deletions.insert(
                "file".into(),
                FileDeletionNode {
                    node_id: "node".into(),
                    repository_id: "repository".into(),
                    history_tree_id: "history".into(),
                    file_id: "file".into(),
                    path: name_policy::PortablePath::parse("file").unwrap(),
                    branch_heads: Default::default(),
                    parent_node_ids: Vec::new(),
                    operation_id: id(2),
                    author: "bob".into(),
                    sequence: 2,
                    created_at: 0,
                },
            );
        }
        ChangedTable::FilePathNotes => {
            state.file_path_notes.insert("node".into(), "note".into());
        }
    }
}

#[test]
fn each_publication_changes_only_its_selected_table_in_hot_and_cold_states() {
    let previous = previous();
    for (changed_index, changed) in TABLES.into_iter().enumerate() {
        let mut next = candidate(&previous);
        change_table(&mut next, changed);
        for next in [next.clone(), cold(&next)] {
            for (allowed_index, allowed) in TABLES.into_iter().enumerate() {
                assert_eq!(
                    verify(&previous, &next, &receipt(2), allowed, invalid).is_ok(),
                    changed_index == allowed_index
                );
            }
        }
    }
}

#[test]
fn shared_tables_and_generations_remain_unchanged() {
    let previous = previous();
    for change in 0..4 {
        let mut next = candidate(&previous);
        match change {
            0 => {
                next.file_names.insert("file".into(), "renamed".into());
            }
            1 => next.sequence = previous.sequence,
            2 => next.business_generation += 1,
            3 => next.topology_generation += 1,
            _ => unreachable!(),
        }
        rejects(&previous, &next);
    }
}

#[test]
fn receipts_and_principals_allow_one_new_pair_and_no_old_changes() {
    let previous = previous();
    for change in 0..6 {
        let mut next = candidate(&previous);
        match change {
            0 => next.operations.get_mut(&id(1)).unwrap().kind = "changed".into(),
            1 => {
                next.operation_principals.insert(id(1), "changed".into());
            }
            2 => {
                next.operations.remove(&id(2));
            }
            3 => {
                next.operation_principals.remove(&id(2));
            }
            4 => {
                next.operations.insert(id(3), receipt(3));
            }
            5 => {
                next.operation_principals.insert(id(3), "carol".into());
            }
            _ => unreachable!(),
        }
        rejects(&previous, &next);
    }
}

#[test]
fn event_prefix_and_exact_new_event_remain_bound() {
    let previous = previous();
    for change in 0..7 {
        let mut next = candidate(&previous);
        match change {
            0 => next.events[0].kind = "changed".into(),
            1 => next.events[1].kind = "changed".into(),
            2 => next.events[1].operation_id = id(3),
            3 => next.events[1].object_id = "changed".into(),
            4 => next.events[1].sequence += 1,
            5 => {
                next.events.pop();
            }
            6 => next.events.push(event(3)),
            _ => unreachable!(),
        }
        rejects(&previous, &next);
    }
}
