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

fn replay_outcome(previous: &State, change: usize) -> State {
    let mut next = previous.clone();
    next.begin_tracking();
    match change {
        0 => {}
        1 => {
            next.file_notes.insert("file".into(), "note".into());
        }
        2 => {
            next.file_notes.insert("file".into(), "note".into());
            next.file_notes.remove("file");
        }
        3 => {
            next.operations.insert(id(2), receipt(2));
        }
        4 => next.operations.get_mut(&id(1)).unwrap().kind = "changed".into(),
        5 => {
            next.operation_principals.insert(id(1), "changed".into());
        }
        6 => next.events.push(event(2)),
        7 => next.events[0].kind = "changed".into(),
        8 => next.sequence += 1,
        9 => next.business_generation += 1,
        10 => next.topology_generation += 1,
        11 => next.control_auth = Some(json!({"state": "changed"})),
        12 => {
            next.directory_selection_generations
                .insert("directory".into(), 3);
        }
        13 => {
            next.file_node_parents
                .insert("node".into(), vec!["parent".into()]);
        }
        14 | 15 => {
            let mut first = event(2);
            if change == 15 {
                first.kind = "different first appended event".into();
            }
            next.events.extend([first, event(3)]);
        }
        16 => next.file_notes = [("file".into(), "note".into())].into(),
        17 => next.events.clear(),
        18 => return cold(&next),
        _ => unreachable!(),
    }
    next
}

#[test]
fn replay_comparison_matches_full_state_for_independent_hot_and_cold_outcomes() {
    let previous = previous();
    let outcomes = (0..19)
        .flat_map(|change| {
            let hot = replay_outcome(&previous, change);
            [cold(&hot), hot]
        })
        .collect::<Vec<_>>();
    for (left, expected) in outcomes.iter().enumerate() {
        for (right, actual) in outcomes.iter().enumerate() {
            assert_eq!(
                same_transition(&previous, expected, actual),
                expected == actual,
                "outcomes {left} and {right}"
            );
        }
    }
}

#[test]
fn table_permissions_preserve_unrelated_rows_for_hot_and_cold_changes() {
    let mutations: [(Table, fn(&mut State)); 5] = [
        (Table::FileNotes, |state| {
            state.file_notes.insert("file".into(), "note".into());
        }),
        (Table::FilePathNotes, |state| {
            state.file_path_notes.insert("node".into(), "note".into());
        }),
        (Table::FileBackupPolicies, |state| {
            state
                .file_backup_policies
                .insert("file".into(), "policy".into());
        }),
        (Table::DirectorySelectionGenerations, |state| {
            state
                .directory_selection_generations
                .insert("directory".into(), 1);
        }),
        (Table::ContentStates, |state| {
            state
                .content_states
                .insert("version".into(), ContentState::Retired);
        }),
    ];
    let previous = previous();
    for (table, mutate) in mutations {
        let mut next = previous.clone();
        next.begin_tracking();
        mutate(&mut next);
        for next in [cold(&next), next] {
            assert!(!unchanged_except(&previous, &next, &[]));
            assert!(unchanged_except(&previous, &next, &[table]));
            for (other, _) in mutations {
                assert_eq!(unchanged_except(&previous, &next, &[other]), table == other);
            }
        }
    }
    let no_net_change = replay_outcome(&previous, 2);
    assert!(unchanged_except(&previous, &no_net_change, &[]));
    assert!(unchanged_except(&previous, &cold(&no_net_change), &[]));
}
