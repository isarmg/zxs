use super::{Page, Scope, admit, endpoint, select, select_head, source};
use crate::files::directory_snapshot::wire::{self, Command, Record};
use crate::files::directory_snapshot::worktree::tests::state;
use crate::files::directory_snapshot::worktree::{can_create, requests};
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::snapshot_worktree::discovery::SnapshotHeadDiscovery;
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SnapshotHistoryCursor, SnapshotHistoryNode,
};
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{BranchName, Generation, OperationId, Sequence};

fn history(state: &crate::ui::State) -> DirectorySnapshotHistory {
    let review = state.files.directory_snapshot.reviewed(state).unwrap();
    let head = wire::ready_source(&review.head).unwrap();
    let older = SnapshotHandle {
        publication: OperationId::from_random([71; 16]),
        node_id: SnapshotNodeId::from_random([72; 16]),
        ..head
    };
    DirectorySnapshotHistory {
        repository_id: review.target.repository,
        context: review.head.context.clone(),
        nodes: vec![
            SnapshotHistoryNode {
                snapshot: head,
                branch_name: BranchName::parse("actual").unwrap(),
                parent: Some(older),
                is_head: true,
            },
            SnapshotHistoryNode {
                snapshot: older,
                branch_name: BranchName::parse("actual").unwrap(),
                parent: None,
                is_head: false,
            },
        ],
        next_cursor: None,
    }
}

#[test]
fn history_selection_freezes_exact_old_source_without_changing_head_or_open_writer() {
    let mut state = state();
    let scope = Scope::capture(&state).unwrap();
    let value = history(&state);
    let older = value.nodes[1].snapshot;
    let reviewed_head = value.nodes[0].snapshot;
    let writer = state.files.directory_snapshot.worktree.handle().unwrap();
    let value = admit(
        json_codec::to_value(&value).unwrap(),
        &scope,
        &value.context,
        None,
    )
    .unwrap();
    state.files.directory_snapshot.worktree.history.page = Some(Page { scope, value });
    select(&mut state, older);
    assert!(can_create(&state));
    assert_eq!(source(&state).unwrap().0, older);
    assert_eq!(
        state.files.directory_snapshot.worktree.handle(),
        Some(writer)
    );
    assert_eq!(
        wire::ready_source(&state.files.directory_snapshot.review.as_ref().unwrap().head),
        Some(reviewed_head),
        "ordinary directory draft commits still use the actual reviewed HEAD",
    );
    let Command::CommitWorktree {
        source: held,
        request,
        ..
    } = requests::commit_command(&state, OperationId::from_random([76; 16])).unwrap()
    else {
        panic!("commit from the already opened writer");
    };
    assert_eq!(
        held, reviewed_head,
        "history selection is not the writer's source"
    );
    assert_eq!(request.target, writer);
    let command = requests::create_command(&state, OperationId::from_random([73; 16])).unwrap();
    let Command::AllocateWorktree { request, .. } = &command else {
        panic!("only Automatic creates are available");
    };
    assert_eq!(request.source, older);
    let record = Record::new(state.principal.clone().unwrap(), command).unwrap();
    let frozen = record.encode().unwrap();
    let newer = SnapshotHandle {
        publication: OperationId::from_random([74; 16]),
        node_id: SnapshotNodeId::from_random([75; 16]),
        ..reviewed_head
    };
    state
        .files
        .directory_snapshot
        .review
        .as_mut()
        .unwrap()
        .head
        .head = SnapshotHeadDiscovery::Head {
        snapshot: newer,
        branch_name: BranchName::parse("actual").unwrap(),
    };
    state.files.directory_snapshot.worktree.history.invalidate();
    assert_eq!(
        source(&state).unwrap().0,
        older,
        "refresh never substitutes newer HEAD"
    );
    assert_eq!(Record::parse(&frozen, &record.principal).unwrap(), record);
    state
        .files
        .directory_snapshot
        .review
        .as_mut()
        .unwrap()
        .head
        .context
        .host
        .expected_root_generation = Generation::new(99);
    assert!(
        !can_create(&state),
        "a stale selected root is not permission to use HEAD"
    );
    select_head(&mut state);
    assert_eq!(
        source(&state).unwrap().0,
        newer,
        "only an explicit choice uses HEAD again"
    );
}

#[test]
fn history_page_admission_binds_cursor_context_and_exact_snapshot_repository() {
    let state = state();
    let scope = Scope::capture(&state).unwrap();
    let mut value = history(&state);
    let cursor = SnapshotHistoryCursor {
        sequence: Sequence::new(9_007_199_254_740_993),
        after: value.nodes[1].snapshot,
    };
    value.next_cursor = Some(cursor);
    let bytes = json_codec::to_value(&value).unwrap();
    assert_eq!(bytes["next_cursor"]["sequence"], "9007199254740993");
    let admitted = admit(bytes, &scope, &value.context, None).unwrap();
    assert_eq!(admitted, value);
    assert_eq!(
        endpoint(value.repository_id, Some(cursor)),
        format!(
            concat!(
                "/api/repositories/{}/snapshot-history?sequence={}&publication={}",
                "&repository_id={}&branch_id={}&node_id={}",
            ),
            value.repository_id,
            cursor.sequence,
            cursor.after.publication,
            cursor.after.repository_id,
            cursor.after.branch_id,
            cursor.after.node_id,
        )
    );
    let mut wrong = value.clone();
    wrong.nodes[0].snapshot.repository_id = SnapshotRepositoryId::from_random([90; 16]);
    assert!(
        admit(
            json_codec::to_value(wrong).unwrap(),
            &scope,
            &value.context,
            None
        )
        .is_err()
    );
    let mut wrong = value.clone();
    wrong.context.host.expected_root_generation = Generation::new(91);
    assert!(
        admit(
            json_codec::to_value(wrong).unwrap(),
            &scope,
            &value.context,
            None
        )
        .is_err()
    );
    let mut next = value.clone();
    next.next_cursor.as_mut().unwrap().sequence = Sequence::new(92);
    assert!(
        admit(
            json_codec::to_value(next).unwrap(),
            &scope,
            &value.context,
            Some(cursor)
        )
        .is_err()
    );
    let empty = DirectorySnapshotHistory {
        nodes: Vec::new(),
        next_cursor: None,
        ..value
    };
    assert!(
        admit(
            json_codec::to_value(&empty).unwrap(),
            &scope,
            &empty.context,
            None
        )
        .is_ok()
    );
    assert!(
        wire::ready_source(&state.files.directory_snapshot.review.as_ref().unwrap().head).is_some(),
        "an empty history response is not an initialization signal"
    );
}
