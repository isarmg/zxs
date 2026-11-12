use super::*;
use storage_contract::{
    Generation,
    file_container_tags::{Replacement, TagsError},
};

fn target() -> Target {
    Target {
        epoch: 0,
        navigation: 0,
        entry: "entry".into(),
        read_only: false,
        request: ReadRequest {
            file_id: "file_00000000000000000000000000000001".parse().unwrap(),
            context: Context {
                repository_id: "repo_00000000000000000000000000000001".parse().unwrap(),
                path: name_policy::PortablePath::parse("a.txt").unwrap(),
            },
        },
    }
}

fn set(values: &[&str]) -> TagSet {
    TagSet::new(values.iter().map(|value| Tag::parse((*value).into()).unwrap())).unwrap()
}
fn snapshot() -> Snapshot {
    let target = target();
    Snapshot {
        file_id: target.request.file_id,
        context: target.request.context,
        tags: set(&["reviewed"]),
        generation: Generation::new(7),
    }
}
pub(crate) fn read_model() -> ReadModel {
    ReadModel {
        target: Some(target()),
        snapshot: Some(snapshot()),
        ..Default::default()
    }
}
pub(crate) fn has_snapshot(model: &ReadModel) -> bool {
    model.snapshot.is_some()
}
pub(crate) fn seed(state: &mut State) {
    state.files.container_tags = read_model();
    state.files.selected = Some(crate::files::test_entries::entry(json!({
        "file_id":"file_00000000000000000000000000000001",
        "repository_id":"repo_00000000000000000000000000000001",
        "repository_path":"a.txt",
        "path":"a.txt"
    })));
}
pub(crate) fn draft_for_instance(instance: u64, input: &str) -> Draft {
    let value = snapshot();
    Draft {
        owner: TagOwner {
            instance,
            target: target(),
        },
        command: ReplaceCommand {
            file_id: value.file_id,
            context: value.context,
            expected_generation: value.generation,
            tags: value.tags,
        },
        input: input.into(),
        needs_review: false,
    }
}
pub(crate) fn submission_fixture() -> (Draft, Value) {
    let mut draft = draft_for_instance(1, "");
    draft.command.tags = set(&["saved"]);
    let body = json!({
        "repository_id":draft.command.context.repository_id,
        "path":draft.command.context.path,
        "expected_generation":"7",
        "tags":["saved"]
    });
    (draft, body)
}
pub(crate) fn replacement() -> Value {
    let command = submission_fixture().0.command;
    json_codec::to_value(Replacement {
        tags: command.tags.clone(),
        request: command,
        generation: Generation::new(8),
        changed: true,
    })
    .unwrap()
}
fn state() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    seed(&mut state);
    state
}

#[test]
fn response_is_bound_to_the_exact_file_repository_path_and_current_format() {
    let request = target().request;
    let wire = json_codec::to_value(snapshot()).unwrap();
    assert_eq!(decode(wire.clone(), &request).unwrap(), snapshot());
    for patch in [
        json!({"file_id":"file_00000000000000000000000000000002"}),
        json!({"context":{
            "repository_id":"repo_00000000000000000000000000000002", "path":"a.txt"
        }}),
        json!({"context":{
            "repository_id":"repo_00000000000000000000000000000001", "path":"b.txt"
        }}),
        json!({"generation":null}),
        json!({"tags":["dup","dup"]}),
        json!({"tags":[" padded "]}),
        json!({"nodes":[]}),
        json!({"tags":["\n"]}),
    ] {
        let mut bad = wire.clone();
        for (key, value) in patch.as_object().unwrap() {
            bad[key] = value.clone();
        }
        assert!(decode(bad, &request).is_err());
    }
    let mut bad = wire;
    bad.as_object_mut().unwrap().remove("generation");
    assert!(decode(bad, &request).is_err());
    assert!(
        read_endpoint(&request)
            .ends_with("repository_id=repo_00000000000000000000000000000001&path=a.txt")
    );
}

#[test]
fn reads_reject_late_completions_after_reload_close_navigation_and_identity_changes() {
    let target = target();
    let mut model = read_model();
    let first = model.begin();
    let second = model.begin();
    model.complete(&target, first, Ok(snapshot()));
    assert!(model.loading && model.snapshot.is_none());
    model.complete(&target, second, Ok(snapshot()));
    assert_eq!(model.snapshot.as_ref(), Some(&snapshot()));
    for changed in [
        None,
        Some(Target {
            navigation: 1,
            ..target.clone()
        }),
        Some(Target {
            epoch: 1,
            ..target.clone()
        }),
        Some(Target {
            request: ReadRequest {
                file_id: "file_00000000000000000000000000000002".parse().unwrap(),
                ..target.request.clone()
            },
            ..target.clone()
        }),
    ] {
        model.set_target(Some(target.clone()));
        let generation = model.begin();
        model.set_target(changed.clone());
        model.complete(&target, generation, Ok(snapshot()));
        assert_eq!(model.target, changed);
        assert!(model.snapshot.is_none() && !model.loading);
    }
    model.set_target(Some(target.clone()));
    let generation = model.begin();
    model.clear();
    model.complete(&target, generation, Ok(snapshot()));
    assert!(model.snapshot.is_none());
}

#[test]
fn branch_content_and_history_paging_do_not_change_container_tags_or_target() {
    let mut state = state();
    let target = Target::capture(&state).unwrap();
    let original = state.files.container_tags.snapshot.clone();
    state.files.file_history = crate::files::history::tests::history(json!({
        "selected_branch_id":"other", "nodes":[{"node_id":"new-version"}]
    }));
    state.files.history_generation += 1;
    state.files.history_loading = true;
    assert_eq!(Target::capture(&state).as_ref(), Some(&target));
    assert_eq!(state.files.container_tags.snapshot, original);
    state.session_epoch += 1;
    assert_ne!(Target::capture(&state).as_ref(), Some(&target));
    state.authenticated = false;
    assert!(Target::capture(&state).is_none());
}

#[test]
fn editor_uses_a_whole_set_without_trimming_and_preserves_failed_input() {
    Owner::new().with(|| {
        let state = RwSignal::new(state());
        start(state);
        let instance = state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().instance());
        for invalid in [" padded ", "", "\n", "reviewed"] {
            edit(state, instance, |draft| draft.input = invalid.into());
            add_input(state, instance);
            state.with_untracked(|s| {
                let draft = s.files.tag_edit.as_ref().unwrap();
                assert_eq!(draft.input, invalid);
                assert_eq!(draft.command.tags, set(&["reviewed"]));
            });
        }
        edit(state, instance, |draft| draft.input = "新标签".into());
        add_input(state, instance);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.command.tags, set(&["reviewed", "新标签"]));
            assert!(draft.input.is_empty());
            assert_eq!(draft.command.expected_generation.get(), 7);
        });
        remove(state, instance, "reviewed");
        assert_eq!(
            state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().command.tags.clone()),
            set(&["新标签"]),
        );
    });
}

#[test]
fn limits_are_bytes_and_full_original_count_not_lossy_set_collection() {
    assert!(Tag::parse("中".repeat(42)).is_ok());
    assert_eq!(Tag::parse("中".repeat(43)), Err(TagsError::InvalidTag));
    assert!(TagSet::new((0..16).map(|i| Tag::parse(i.to_string()).unwrap())).is_ok());
    assert_eq!(
        TagSet::new((0..17).map(|i| Tag::parse(i.to_string()).unwrap())),
        Err(TagsError::Limit),
    );
    assert_eq!(
        TagSet::new([
            Tag::parse("x".into()).unwrap(),
            Tag::parse("x".into()).unwrap(),
        ]),
        Err(TagsError::Duplicate),
    );
}

#[test]
fn conflict_requires_explicit_fresh_base_confirmation_without_merging_draft() {
    Owner::new().with(|| {
        let mut value = state();
        let mut draft = draft_for_instance(1, "unfinished");
        draft.command.tags = set(&["mine"]);
        draft.require_review();
        value.files.tag_edit = Some(draft);
        value.files.container_tags.clear();
        let state = RwSignal::new(value);
        accept_current(state, 1);
        assert!(state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().requires_review()));
        state.update(|s| {
            s.files.container_tags = read_model();
            let snapshot = s.files.container_tags.snapshot.as_mut().unwrap();
            snapshot.generation = Generation::new(9);
            snapshot.tags = set(&["concurrent"]);
        });
        accept_current(state, 1);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.command.expected_generation.get(), 9);
            assert_eq!(draft.command.tags, set(&["mine"]));
            assert_eq!(draft.input, "unfinished");
            assert!(!draft.requires_review());
        });
        submit(state, 1);
        assert!(state.with_untracked(|s| s.files.tag_submission.is_none()));
    });
}

#[test]
fn old_editor_callbacks_cannot_change_or_cancel_a_new_draft_and_pending_cannot_cancel() {
    Owner::new().with(|| {
        let mut value = state();
        value.files.tag_edit = Some(draft_for_instance(2, "new"));
        let state = RwSignal::new(value);
        edit(state, 1, |draft| draft.input = "late".into());
        cancel(state, 1);
        assert_eq!(
            state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().input.clone()),
            "new",
        );
        state.update(|s| s.files.mutation_pending = true);
        cancel(state, 2);
        assert!(state.with_untracked(|s| s.files.tag_edit.is_some()));
        state.update(|s| {
            s.files.mutation_pending = false;
            s.files.writable = false;
        });
        cancel(state, 2);
        assert!(state.with_untracked(|s| s.files.tag_edit.is_none()));
    });
}

#[test]
fn logout_and_current_session_revocation_cancel_the_tag_read_without_accepting_late_data() {
    let mut value = state();
    let original = target();
    let generation = value.files.container_tags.begin();
    assert!(crate::ui::begin_logout(&mut value).is_some());
    value
        .files
        .container_tags
        .complete(&original, generation, Ok(snapshot()));
    assert!(value.files.container_tags.snapshot.is_none());
    assert!(!value.files.container_tags.loading);
    Owner::new().with(|| {
        let state = RwSignal::new(state());
        let error = api::Error {
            code: storage_contract::ErrorCode::RecoveryRequired,
            status: 401,
            message: "expired".into(),
            retryable: false,
            request_id: None,
            source: None,
        };
        assert!(!accept_read_response::<Snapshot>(state, 0, &Err(error)));
        state.with_untracked(|s| {
            assert!(!s.authenticated);
            assert!(s.files.container_tags.snapshot.is_none());
        });
    });
}

#[test]
fn tag_read_errors_are_not_rendered_as_an_empty_success_and_keep_their_typed_source() {
    let target = target();
    let mut model = read_model();
    let generation = model.begin();
    let mut wrong = snapshot();
    wrong.context.path = name_policy::PortablePath::parse("different.txt").unwrap();
    let error = decode(json_codec::to_value(wrong).unwrap(), &target.request).unwrap_err();
    assert!(matches!(
        error.source.as_deref(),
        Some(api::ErrorSource::ContainerTags(DecodeError::Authority(_)))
    ));
    model.complete(&target, generation, Err(error));
    assert!(model.snapshot.is_none() && model.error.is_some() && !model.loading);
}

#[test]
fn complete_target_and_write_gates_reject_stale_editor_callbacks() {
    for change in 0..5 {
        Owner::new().with(|| {
            let mut value = state();
            value.files.tag_edit = Some(draft_for_instance(1, "original"));
            match change {
                0 => value.session_epoch += 1,
                1 => value.files.generation += 1,
                2 => value.authenticated = false,
                3 => {
                    value.files.selected = Some(crate::files::test_entries::entry(json!({
                        "file_id":"file_00000000000000000000000000000001",
                        "repository_id":"repo_00000000000000000000000000000001",
                        "repository_path":"elsewhere.txt", "path":"elsewhere.txt"
                    })));
                }
                _ => value.management.storage = crate::management::Storage::ReadOnly,
            }
            let state = RwSignal::new(value);
            edit(state, 1, |draft| draft.input = "late".into());
            add_input(state, 1);
            state.with_untracked(|s| {
                let draft = s.files.tag_edit.as_ref().unwrap();
                assert_eq!(draft.input, "original");
                assert_eq!(draft.command.tags, set(&["reviewed"]));
            });
        });
    }
}
