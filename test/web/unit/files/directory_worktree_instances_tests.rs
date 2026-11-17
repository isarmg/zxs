use super::{Listing, Scope, Selection, admit, endpoint};
use crate::files::directory_snapshot::worktree::tests::state;
use crate::files::directory_snapshot::worktree::{
    Observed, bookmark, can_commit, requests, status,
};
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::instances::{
    DIRECTORY_WORKTREE_LIST_LIMIT, DirectoryWorktrees,
};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeId};
use storage_contract::working_tree::DirectoryId;

fn listing(state: &crate::ui::State) -> DirectoryWorktrees {
    let current = state
        .files
        .directory_snapshot
        .worktree
        .observed(state)
        .unwrap()
        .status
        .clone();
    let other = storage_contract::snapshot_worktree::WorktreeStatus {
        target: WorktreeHandle {
            publication: OperationId::from_random([81; 16]),
            worktree_id: WorktreeId::from_random([82; 16]),
        },
        name: "worktree-actual-002".parse().unwrap(),
        ..current.clone()
    };
    DirectoryWorktrees {
        repository_id: state
            .files
            .directory_snapshot
            .reviewed(state)
            .unwrap()
            .target
            .repository,
        context: state
            .files
            .directory_snapshot
            .reviewed(state)
            .unwrap()
            .head
            .context
            .clone(),
        instances: vec![current, other],
    }
}

#[test]
fn instance_list_selects_an_earlier_exact_handle_but_requires_fresh_status_before_commit() {
    let mut state = state();
    let scope = Scope::capture(&state).unwrap();
    let value = listing(&state);
    let first = value.instances[0].clone();
    let second = value.instances[1].clone();
    let admitted = admit(
        json_codec::to_value(&value).unwrap(),
        &scope,
        &value.context,
    )
    .unwrap();
    state.files.directory_snapshot.worktree.instances.listing = Some(Listing {
        scope: scope.clone(),
        value: admitted,
    });
    state.files.directory_snapshot.worktree.reference = Some(bookmark::Reference::new(
        state.principal.clone().unwrap(),
        scope.repository,
        second.target,
    ));
    state.files.directory_snapshot.worktree.observed = Some(Observed {
        scope: scope.clone(),
        status: second,
    });
    assert!(can_commit(&state));
    let operation = OperationId::from_random([83; 16]);
    let creation = requests::create_command(&state, operation).unwrap();
    let selected = Selection::capture(&state, first.target).unwrap();
    assert_eq!(selected.reference.target, first.target);
    assert!(selected.apply(&mut state));
    assert_eq!(
        state.files.directory_snapshot.worktree.handle(),
        Some(first.target)
    );
    assert!(
        !can_commit(&state),
        "a list revision is not permission to commit"
    );
    assert_eq!(
        requests::create_command(&state, operation).unwrap(),
        creation,
        "opening an instance does not change the selected source for a new one"
    );
    assert!(state.files.directory_snapshot.pending.is_none());
    let verified = status(
        json_codec::to_value(&first).unwrap(),
        &scope,
        first.target,
        None,
    )
    .unwrap();
    state.files.directory_snapshot.worktree.observed = Some(Observed {
        scope,
        status: verified,
    });
    assert!(
        can_commit(&state),
        "only the separate exact status read restores commit admission"
    );
}

#[test]
fn instance_list_is_complete_bounded_and_bound_to_the_current_root() {
    let mut state = state();
    let scope = Scope::capture(&state).unwrap();
    let value = listing(&state);
    assert_eq!(
        endpoint(value.repository_id),
        format!(
            "/api/repositories/{}/directory-worktrees",
            value.repository_id,
        )
    );
    let mut foreign = value.clone();
    foreign.instances[0].parent_directory_id = DirectoryId::from_random([90; 16]);
    assert!(
        admit(
            json_codec::to_value(foreign).unwrap(),
            &scope,
            &value.context
        )
        .is_err()
    );
    let oversized = DirectoryWorktrees {
        instances: vec![
            value.instances[0].clone();
            usize::from(DIRECTORY_WORKTREE_LIST_LIMIT.get()) + 1
        ],
        ..value.clone()
    };
    assert!(
        admit(
            json_codec::to_value(oversized).unwrap(),
            &scope,
            &value.context
        )
        .is_err()
    );
    state.files.directory_snapshot.worktree.instances.listing = Some(Listing { scope, value });
    let missing = WorktreeHandle {
        publication: OperationId::from_random([91; 16]),
        worktree_id: WorktreeId::from_random([92; 16]),
    };
    assert!(
        Selection::capture(&state, missing).is_none(),
        "no name-based handle substitution"
    );
    state.session_epoch += 1;
    assert!(
        state
            .files
            .directory_snapshot
            .worktree
            .instances
            .listing(&state)
            .is_none()
    );
}
