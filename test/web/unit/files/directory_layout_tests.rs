use super::*;
use json_codec::json;
fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn fixture() -> (State, Model) {
    let target = Target {
        epoch: 1,
        generation: 2,
        repository: id("repo", 1),
        path: "docs".into(),
        namespace_head: id("commit", 1),
    };
    let mut state = State {
        authenticated: true,
        session_epoch: target.epoch,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.loaded = true;
    state.files.writable = true;
    state.files.generation = target.generation;
    state.files.repository_id = target.repository.clone();
    state.files.repository_path = target.path.clone();
    state.files.namespace_head = target.namespace_head.clone();
    let status = super::super::decode(json!({"repository_id":id("repo",1),"directory_id":id("entry",1),"path":"docs","initialized":true,"owner":{"directory_id":id("entry",1),"path":"docs"},"directory_branch_id":id("dbr",1),"working_tree_id":id("dtree",1),"head_tree_id":null,"index_generation":"0","staged":false,"unstaged":false,"needs_restage":false,"added":"0","modified":"0","deleted":"0","children":[],"legacy_history":false}), &target).unwrap();
    let mut model = Model::default();
    model.begin(Some(target.clone()));
    model.status = Some(status);
    model.review_target = Some(target);
    model.reading = false;
    (state, model)
}
fn wire() -> json_codec::Value {
    json!({"repository_id":id("repo",1),"state_sequence":"9007199254740993","directories":"2","containers":"3","unavailable_containers":"0","unavailable_histories":"0"})
}
#[test]
fn layout_requires_explicit_current_review_and_blocks_repeated_submission() {
    let (mut state, model) = fixture();
    let mut phase = Phase::Idle;
    assert!(begin(&mut state, &model, &mut phase).is_none());
    let target = review(&state, &model, &phase).unwrap();
    phase = Phase::Review(target.clone());
    assert_eq!(begin(&mut state, &model, &mut phase), Some(target));
    assert!(state.files.mutation_pending);
    assert!(begin(&mut state, &model, &mut phase).is_none());
}
#[test]
fn layout_cancel_and_changed_page_never_submit() {
    let (mut state, model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    let mut cancelled = Phase::Idle;
    assert!(begin(&mut state, &model, &mut cancelled).is_none());
    let mut phase = Phase::Review(target);
    state.files.generation += 1;
    assert!(begin(&mut state, &model, &mut phase).is_none());
    assert!(!state.files.mutation_pending);
}
#[test]
fn layout_ordinary_unowned_and_readonly_views_cannot_review() {
    let (mut state, mut model) = fixture();
    model.status.as_mut().unwrap().initialized = false;
    model.status.as_mut().unwrap().owner = None;
    assert!(review(&state, &model, &Phase::Idle).is_none());
    model.status.as_mut().unwrap().initialized = true;
    state.files.writable = false;
    assert!(review(&state, &model, &Phase::Idle).is_none());
    state.files.writable = true;
    state.management.storage = crate::management::Storage::Unavailable;
    assert!(review(&state, &model, &Phase::Idle).is_none());
}
#[test]
fn layout_old_session_cannot_clear_a_new_sessions_pending_gate() {
    let (mut state, model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    state.files.mutation_pending = true;
    state.session_epoch += 1;
    assert!(!release(&mut state, &target));
    assert!(state.files.mutation_pending);
}
#[test]
fn layout_navigation_preserves_gate_but_discards_old_result_and_can_review_again() {
    let (mut state, model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    let mut phase = Phase::Review(target.clone());
    begin(&mut state, &model, &mut phase).unwrap();
    state
        .files
        .navigate(crate::files::model::Navigation::Directory(
            "elsewhere".into(),
        ));
    assert!(state.files.mutation_pending);
    let same = release(&mut state, &target);
    assert!(!same);
    assert!(!state.files.mutation_pending);
    complete(&mut phase, target.clone(), decode(wire(), &target), same);
    assert!(matches!(phase, Phase::Idle));
}
#[test]
fn layout_unknown_result_requires_fresh_listing_and_never_fakes_operation() {
    let (mut state, mut model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    let mut phase = Phase::Running(target.clone());
    complete(
        &mut phase,
        target.clone(),
        Err(api::error(
            storage_contract::ErrorCode::InvalidResponse,
            "unknown",
        )),
        true,
    );
    assert!(matches!(phase, Phase::Finished(_, Err(_))));
    assert!(review(&state, &model, &phase).is_none());
    state.files.generation += 1;
    let fresh = Target::capture(&state).unwrap();
    model.target = Some(fresh.clone());
    model.review_target = Some(fresh);
    assert!(review(&state, &model, &phase).is_some());
    assert!(state.files.operations.is_empty());
}
#[test]
fn layout_result_is_strict_scoped_and_preserves_large_integers() {
    let (state, model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    assert_eq!(
        decode(wire(), &target).unwrap().state_sequence,
        9_007_199_254_740_993
    );
    for key in [
        "state_sequence",
        "directories",
        "containers",
        "unavailable_containers",
        "unavailable_histories",
    ] {
        let mut bad = wire();
        bad[key] = 1.into();
        assert!(decode(bad, &target).is_err());
        let mut bad = wire();
        bad.as_object_mut().unwrap().remove(key);
        assert!(decode(bad, &target).is_err());
    }
    let mut bad = wire();
    bad["repository_id"] = id("repo", 2).into();
    assert!(decode(bad, &target).is_err());
    let mut bad = wire();
    bad["operation_id"] = id("op", 1).into();
    assert!(decode(bad, &target).is_err());
}
#[test]
fn layout_partial_result_cannot_claim_full_completion_or_verified_contents() {
    let (state, model) = fixture();
    let target = review(&state, &model, &Phase::Idle).unwrap();
    let mut result = decode(wire(), &target).unwrap();
    let complete = outcome(&result);
    result.unavailable_histories = 1;
    assert_ne!(outcome(&result), complete);
    result.unavailable_histories = 0;
    result.unavailable_containers = 1;
    assert_ne!(outcome(&result), complete);
}

#[test]
fn layout_completion_after_actual_signal_disposal_releases_only_global_gate() {
    let root = Owner::new();
    let (initial, model) = fixture();
    let target = review(&initial, &model, &Phase::Idle).unwrap();
    let state = root.with(|| RwSignal::new(initial));
    let page = root.with(Owner::new);
    let phase = page.with(|| RwSignal::new(Phase::Running(target.clone())));
    state.update(|state| state.files.mutation_pending = true);
    page.cleanup();
    assert!(phase.try_get_untracked().is_none());
    finish(state, phase, target.clone(), decode(wire(), &target));
    assert!(!state.with_untracked(|state| state.files.mutation_pending));
    assert!(phase.try_get_untracked().is_none());
    root.cleanup();
}
#[test]
fn layout_async_completion_from_old_epoch_cannot_clear_remounted_pending_or_result() {
    let root = Owner::new();
    let (initial, model) = fixture();
    let target = review(&initial, &model, &Phase::Idle).unwrap();
    let state = root.with(|| RwSignal::new(initial));
    let phase = root.with(|| RwSignal::new(Phase::Idle));
    state.update(|state| {
        state.session_epoch += 1;
        state.files.mutation_pending = true;
    });
    finish(state, phase, target.clone(), decode(wire(), &target));
    assert!(state.with_untracked(|state| state.files.mutation_pending));
    assert!(matches!(phase.get_untracked(), Phase::Idle));
    root.cleanup();
}
