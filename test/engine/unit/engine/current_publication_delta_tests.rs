use super::*;
use crate::{CommitState, CurrentHealth, ErrorCode, ProtectionAtPublish};
use json_codec::json;

const KIND: &str = "branch.archive";

fn id(number: u64) -> OperationId {
    format!("op_{number:032x}").parse().unwrap()
}

fn invalid() -> Error {
    Error::new(ErrorCode::PublicationInvalid, "family-specific refusal")
}

fn accepts(receipt: &OperationResult) -> bool {
    receipt.kind == KIND || receipt.result.get("namespace_archive").is_some()
}

fn result(number: u64, kind: &str) -> OperationResult {
    OperationResult {
        operation_id: id(number),
        kind: kind.into(),
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

fn event(number: u64, kind: &str) -> Event {
    Event {
        sequence: number,
        operation_id: id(number),
        kind: kind.into(),
        object_id: format!("object-{number}"),
    }
}

fn before() -> State {
    let mut state = State::default();
    state.operations.insert(id(1), result(1, KIND));
    state.operations.insert(id(2), result(2, "unrelated"));
    state.operation_principals.insert(id(1), "alice".into());
    state.operation_principals.insert(id(2), "bob".into());
    state.events.extend([event(1, KIND), event(2, "unrelated")]);
    state
}

fn candidate(before: &State) -> State {
    let mut next = before.clone();
    next.begin_tracking();
    next
}

fn cold(state: &State) -> State {
    json_codec::from_slice(&crate::canonical_json(state).unwrap()).unwrap()
}

#[test]
fn tracked_and_cold_receipt_scans_select_one_new_family_member() {
    let before = before();
    let mut next = candidate(&before);
    assert!(receipt(&before, &next, accepts, invalid).unwrap().is_none());
    next.operations.get_mut(&id(2)).unwrap().result = json!({"changed":true});
    next.operation_principals.insert(id(2), "carol".into());
    assert!(receipt(&before, &next, accepts, invalid).unwrap().is_none());
    next.operations.insert(id(3), result(3, KIND));
    next.operation_principals.insert(id(3), "alice".into());
    for next in [next.clone(), cold(&next)] {
        let selected = receipt(&before, &next, accepts, invalid).unwrap().unwrap();
        assert_eq!(selected.operation_id, id(3));
        assert!(std::ptr::eq(selected, &next.operations[&id(3)]));
    }
}

#[test]
fn receipt_scan_rejects_rewrites_owner_only_changes_key_mismatch_and_second_receipt() {
    let before = before();
    for change in 0..8 {
        let mut next = candidate(&before);
        match change {
            0 => {
                next.operations.remove(&id(1));
            }
            1 => next.operations.get_mut(&id(1)).unwrap().kind = "retagged".into(),
            2 => next.operations.get_mut(&id(1)).unwrap().result = json!({"rewritten":true}),
            3 => {
                next.operation_principals.insert(id(1), "mallory".into());
            }
            4 => {
                next.operation_principals.remove(&id(1));
            }
            5 => {
                next.operations.insert(id(3), result(4, KIND));
            }
            6 => {
                next.operations.insert(id(3), result(3, KIND));
                next.operations.insert(id(4), result(4, KIND));
            }
            7 => next.operations.get_mut(&id(2)).unwrap().kind = KIND.into(),
            _ => unreachable!(),
        }
        for next in [next.clone(), cold(&next)] {
            let error = receipt(&before, &next, accepts, invalid).unwrap_err();
            assert_eq!(error.code(), ErrorCode::PublicationInvalid);
            assert!(error.to_string().contains("family-specific refusal"));
        }
    }
}

#[test]
fn marker_based_membership_preserves_its_existing_owner_rule() {
    let mut before = before();
    before.operations.get_mut(&id(2)).unwrap().result = json!({"namespace_archive":{}});
    let mut next = candidate(&before);
    next.operation_principals.insert(id(2), "mallory".into());
    for next in [next.clone(), cold(&next)] {
        assert!(receipt(&before, &next, accepts, invalid).is_err());
        assert!(receipt(&before, &next, |row| row.kind == KIND, invalid).is_ok());
    }
}

#[test]
fn event_scan_keeps_old_ordinals_and_binds_only_new_family_events() {
    let before = before();
    let mut valid = candidate(&before);
    valid.events.push(event(3, KIND));
    valid.events.push(event(4, "unrelated"));
    for next in [valid.clone(), cold(&valid)] {
        events(&before, &next, Some(id(3)), |kind| kind == KIND, invalid).unwrap();
        assert!(events(&before, &next, None, |kind| kind == KIND, invalid).is_err());
    }
    for change in 0..5 {
        let mut next = candidate(&before);
        match change {
            0 => next.events[0].kind = "retagged".into(),
            1 => next.events[0].object_id = "rewritten".into(),
            2 => next.events.clear(),
            3 => next.events[1].kind = KIND.into(),
            4 => next.events.push(event(4, KIND)),
            _ => unreachable!(),
        }
        for next in [next.clone(), cold(&next)] {
            assert!(events(&before, &next, Some(id(3)), |kind| kind == KIND, invalid).is_err());
        }
    }
}

#[test]
fn one_event_retains_exact_count_value_and_prefix_rules() {
    let before = before();
    let expected = event(3, KIND);
    let mut next = candidate(&before);
    assert!(!one_event(&before, &next, &expected));
    next.events.push(expected.clone());
    assert!(one_event(&before, &next, &expected));
    assert!(one_event(&before, &cold(&next), &expected));
    let mut rewritten = next.clone();
    rewritten.events[0].object_id = "rewritten".into();
    assert!(!one_event(&before, &rewritten, &expected));
    next.events.push(expected.clone());
    assert!(!one_event(&before, &next, &expected));
}

#[test]
fn only_added_rejects_replacements_removals_and_unrelated_writes() {
    let before = before();
    let expected = result(3, KIND);
    let mut valid = candidate(&before);
    valid.operations.insert(id(3), expected.clone());
    for next in [valid.clone(), cold(&valid)] {
        assert!(only_added(
            &before.operations,
            &next.operations,
            id(3),
            &expected
        ));
        assert!(!only_added(
            &before.operations,
            &next.operations,
            id(1),
            &before.operations[&id(1)]
        ));
    }
    for change in 0..4 {
        let mut next = valid.clone();
        match change {
            0 => {
                next.operations.remove(&id(1));
            }
            1 => next.operations.get_mut(&id(1)).unwrap().kind = "rewritten".into(),
            2 => {
                next.operations.insert(id(4), result(4, "unrelated"));
            }
            3 => next.operations.get_mut(&id(3)).unwrap().kind = "wrong".into(),
            _ => unreachable!(),
        }
        for next in [next.clone(), cold(&next)] {
            assert!(!only_added(
                &before.operations,
                &next.operations,
                id(3),
                &expected
            ));
        }
    }
}
