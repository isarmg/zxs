use super::*;
use json_codec::json;

fn id(prefix: &str, value: u8) -> String {
    format!("{prefix}_{value:032x}")
}
fn target() -> Target {
    Target {
        epoch: 1,
        generation: 2,
        repository: id("repo", 1),
        path: "folder".into(),
        namespace_head: id("commit", 1),
    }
}
fn wire() -> json_codec::Value {
    json!({"repository_id":id("repo",1),"directory_id":id("entry",1),"path":"folder",
        "initialized":true,"owner":{"directory_id":id("entry",1),"path":"folder"},
        "directory_branch_id":id("dbr",1),"working_tree_id":id("dtree",1),"head_tree_id":null,
        "index_generation":"9007199254740993","staged":true,"unstaged":false,"needs_restage":false,
        "added":"2","modified":"1","deleted":"0","children":[],"legacy_history":false})
}
fn child() -> json_codec::Value {
    json!({"directory_id":id("entry",2),"path":"folder/child","directory_branch_id":id("dbr",2),
        "head_tree_id":id("dtree",2),"staged_tree_id":id("dtree",3),"committed_tree_id":id("dtree",3),
        "dirty":true,"reference_changed":true,"staged":false,"needs_restage":false})
}
fn uninitialized() -> json_codec::Value {
    let mut value = wire();
    value["initialized"] = false.into();
    value["owner"] = json!({"directory_id":id("repo",1),"path":""});
    value["directory_branch_id"] = json!(null);
    value["working_tree_id"] = json!(null);
    value["index_generation"] = "0".into();
    value["staged"] = false.into();
    value["added"] = "0".into();
    value["modified"] = "0".into();
    value
}
fn reviewed(value: json_codec::Value) -> (State, Model) {
    let target = target();
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
    let mut model = Model::default();
    model.begin(Some(target.clone()));
    model.status = Some(decode(value, &target).unwrap());
    model.review_target = Some(target);
    model.reading = false;
    model.message = "Review current work".into();
    (state, model)
}
#[test]
fn directory_git_decode_keeps_large_generations_and_exact_location() {
    let status = decode(wire(), &target()).unwrap();
    assert_eq!(status.index_generation, 9_007_199_254_740_993);
    assert_eq!((status.added, status.modified, status.deleted), (2, 1, 0));
    assert!(status.initialized && status.head_tree_id.is_none());
    for field in ["repository_id", "path"] {
        let mut value = wire();
        value[field] = "changed".into();
        assert_eq!(
            decode(value, &target()).unwrap_err().code,
            storage_contract::ErrorCode::HistoryChanged
        );
    }
    for field in ["index_generation", "added", "modified", "deleted"] {
        let mut value = wire();
        value[field] = 2.into();
        assert!(decode(value, &target()).is_err());
    }
    let mut value = wire();
    value["extra"] = true.into();
    assert!(decode(value, &target()).is_err());
}

#[test]
fn empty_directory_git_status_keeps_unborn_head_staged() {
    let mut value = wire();
    value["index_generation"] = "1".into();
    for field in ["added", "modified", "deleted"] {
        value[field] = "0".into();
    }
    let (state, model) = reviewed(value.clone());
    let status = model.status.as_ref().unwrap();
    assert!(status.initialized && status.head_tree_id.is_none() && status.staged);
    assert_eq!(status.index_generation, 1);
    assert!(commit_review(&state, &model).is_some());

    value["staged"] = false.into();
    assert_eq!(
        decode(value.clone(), &target()).unwrap_err().code,
        storage_contract::ErrorCode::InvalidResponse
    );

    value["head_tree_id"] = id("dtree", 1).into();
    value["index_generation"] = "2".into();
    let (state, model) = reviewed(value);
    let status = model.status.as_ref().unwrap();
    assert_eq!(
        status.head_tree_id.as_deref(),
        Some(id("dtree", 1).as_str())
    );
    assert!(!status.staged);
    assert!(commit_review(&state, &model).is_none());
}

#[test]
fn directory_git_nullable_fields_are_required_not_legacy_defaults() {
    for field in [
        "owner",
        "directory_branch_id",
        "working_tree_id",
        "head_tree_id",
    ] {
        let mut value = wire();
        value.as_object_mut().unwrap().remove(field);
        assert!(decode(value, &target()).is_err(), "missing {field}");
    }
    for field in ["head_tree_id", "staged_tree_id", "committed_tree_id"] {
        let mut row = child();
        row.as_object_mut().unwrap().remove(field);
        let mut value = wire();
        value["children"] = json!([row]);
        assert!(decode(value, &target()).is_err(), "missing child {field}");
    }
}
#[test]
fn uninitialized_directory_belongs_to_ancestor_and_requires_manual_initialization() {
    let (state, model) = reviewed(uninitialized());
    assert_eq!(
        initialize_review(&state, &model),
        Some(Review::Initialize {
            repository: id("repo", 1),
            path: "folder".into(),
            namespace_head: id("commit", 1)
        })
    );
    assert!(commit_review(&state, &model).is_none());
    assert!(child_review(&state, &model, &id("entry", 2)).is_none());
    for change in 0..6 {
        let mut value = uninitialized();
        match change {
            0 => value["directory_branch_id"] = id("dbr", 1).into(),
            1 => value["index_generation"] = "1".into(),
            2 => value["staged"] = true.into(),
            3 => value["owner"] = json!({"directory_id":id("entry",1),"path":"folder"}),
            4 => value["owner"] = json!({"directory_id":id("entry",3),"path":"other"}),
            5 => value["children"] = json!([child()]),
            _ => unreachable!(),
        }
        assert!(decode(value, &target()).is_err(), "change {change}");
    }
}
#[test]
fn commit_pins_working_tree_and_index_without_committing_children() {
    let mut value = wire();
    value["children"] = json!([child()]);
    let (state, model) = reviewed(value);
    assert!(initialize_review(&state, &model).is_none());
    assert_eq!(
        commit_review(&state, &model),
        Some(Review::Commit {
            repository: id("repo", 1),
            branch: id("dbr", 1),
            tree: id("dtree", 1),
            index_generation: 9_007_199_254_740_993,
            message: "Review current work".into()
        })
    );
    assert_eq!(
        child_review(&state, &model, &id("entry", 2)),
        Some(Review::StageChild {
            repository: id("repo", 1),
            directory: id("entry", 1),
            branch: id("dbr", 1),
            index_generation: 9_007_199_254_740_993,
            child: id("entry", 2),
            head: id("dtree", 2)
        })
    );
}
#[test]
fn child_reference_is_explicit_and_never_inferred_from_parent_or_dirty_work() {
    let mut value = wire();
    let mut pending = child();
    pending["head_tree_id"] = json!(null);
    pending["staged_tree_id"] = json!(null);
    pending["committed_tree_id"] = json!(null);
    pending["reference_changed"] = false.into();
    pending["staged"] = true.into();
    value["children"] = json!([pending]);
    let (state, model) = reviewed(value);
    assert!(model.status.as_ref().unwrap().children[0].staged);
    assert!(child_review(&state, &model, &id("entry", 2)).is_none());
    for change in 0..5 {
        let mut value = wire();
        value["children"] = json!([child()]);
        match change {
            0 => value["children"][0]["path"] = "outside".into(),
            1 => value["children"][0]["directory_id"] = "bad".into(),
            2 => value["children"][0]["reference_changed"] = false.into(),
            3 => value["children"] = json!([child(), child()]),
            4 => value["children"][0]["head_tree_id"] = "bad".into(),
            _ => unreachable!(),
        }
        assert!(decode(value, &target()).is_err(), "change {change}");
    }
}
#[test]
fn writes_require_fresh_review_same_session_and_no_pending_operation() {
    let (original, model) = reviewed(wire());
    for change in 0..6 {
        let mut state = original.clone();
        match change {
            0 => state.session_epoch += 1,
            1 => state.files.generation += 1,
            2 => state.files.repository_path = "other".into(),
            3 => state.files.namespace_head = id("commit", 2),
            4 => state.files.loading = true,
            5 => state.files.mutation_pending = true,
            _ => unreachable!(),
        }
        assert!(commit_review(&state, &model).is_none(), "change {change}");
    }
    let mut changed = model.clone();
    changed.reading = true;
    assert!(commit_review(&original, &changed).is_none());
    changed.reading = false;
    changed.error = Some(invalid());
    assert!(commit_review(&original, &changed).is_none());
}
#[test]
fn repeated_reads_have_distinct_tokens_and_navigation_discards_draft() {
    let (_, mut model) = reviewed(wire());
    let target = target();
    let first = model.begin(Some(target.clone()));
    let second = model.begin(Some(target.clone()));
    assert!(!model.current(&target, first));
    assert!(model.current(&target, second));
    assert_eq!(model.message, "Review current work");
    assert!(
        model.status.is_some(),
        "failed refresh retains old read-only context"
    );
    let mut next = target;
    next.path = "another".into();
    model.begin(Some(next));
    assert!(model.message.is_empty() && model.status.is_none());
}
#[test]
fn directory_commit_message_uses_utf8_byte_limit() {
    let (state, mut model) = reviewed(wire());
    for message in ["".into(), " \n ".into(), "字".repeat(43)] {
        model.message = message;
        assert!(commit_review(&state, &model).is_none());
    }
    model.message = format!("{}ab", "字".repeat(42));
    assert_eq!(model.message.len(), 128);
    assert!(commit_review(&state, &model).is_some());
    model.status.as_mut().unwrap().staged = false;
    assert!(commit_review(&state, &model).is_none());
}

#[test]
fn changed_repository_boundaries_require_explicit_owned_restage() {
    let mut value = wire();
    value["needs_restage"] = true.into();
    let (state, model) = reviewed(value);
    assert!(commit_review(&state, &model).is_none());
    assert_eq!(
        owned_review(&state, &model),
        Some(Review::StageOwned {
            repository: id("repo", 1),
            directory: id("entry", 1),
            branch: id("dbr", 1),
            tree: id("dtree", 1),
            index_generation: 9_007_199_254_740_993
        })
    );
    let mut value = wire();
    value["children"] = json!([child()]);
    value["children"][0]["needs_restage"] = true.into();
    let (state, model) = reviewed(value);
    assert!(
        commit_review(&state, &model).is_some(),
        "a child's boundary work never requires committing that child"
    );
    assert!(owned_review(&state, &model).is_none());
    let mut value = uninitialized();
    value["needs_restage"] = true.into();
    assert!(decode(value, &target()).is_err());
}

#[test]
fn fresh_child_pin_can_precede_owned_restage_without_authorizing_parent_commit() {
    let mut value = wire();
    value["needs_restage"] = true.into();
    value["children"] = json!([child()]);
    let (state, model) = reviewed(value);
    assert!(owned_review(&state, &model).is_some());
    assert!(child_review(&state, &model, &id("entry", 2)).is_some());
    assert!(commit_review(&state, &model).is_none());

    let mut refreshed = model.clone();
    refreshed.status.as_mut().unwrap().index_generation += 1;
    let Some(Review::StageChild {
        index_generation, ..
    }) = child_review(&state, &refreshed, &id("entry", 2))
    else {
        panic!("a refreshed direct child HEAD remains explicitly stageable");
    };
    assert_eq!(index_generation, 9_007_199_254_740_994);
    assert!(commit_review(&state, &refreshed).is_none());
}

#[test]
fn all_directory_reviews_share_fresh_scope_session_and_write_fences() {
    let (original, initialize) = reviewed(uninitialized());
    let (_, commit) = reviewed(wire());
    let mut value = wire();
    value["needs_restage"] = true.into();
    value["children"] = json!([child()]);
    let (_, staging) = reviewed(value);
    for change in 0..9 {
        let mut state = original.clone();
        match change {
            0 => state.session_epoch += 1,
            1 => state.files.generation += 1,
            2 => state.files.repository_path = "other".into(),
            3 => state.files.repository_id = id("repo", 2),
            4 => state.files.namespace_head = id("commit", 2),
            5 => state.files.loading = true,
            6 => state.files.mutation_pending = true,
            7 => state.files.writable = false,
            8 => state.authenticated = false,
            _ => unreachable!(),
        }
        assert!(
            initialize_review(&state, &initialize).is_none(),
            "initialize change {change}"
        );
        assert!(
            commit_review(&state, &commit).is_none(),
            "commit change {change}"
        );
        assert!(
            owned_review(&state, &staging).is_none(),
            "owned change {change}"
        );
        assert!(
            child_review(&state, &staging, &id("entry", 2)).is_none(),
            "child change {change}"
        );
    }
}

#[test]
fn unstaged_work_can_be_staged_without_forcing_it_into_an_existing_index_commit() {
    let mut value = wire();
    value["unstaged"] = true.into();
    let (state, mut model) = reviewed(value);
    assert!(owned_review(&state, &model).is_some());
    assert!(
        commit_review(&state, &model).is_some(),
        "the existing index remains committable"
    );
    model.status.as_mut().unwrap().staged = false;
    assert!(commit_review(&state, &model).is_none());
    assert!(owned_review(&state, &model).is_some());
    let mut value = uninitialized();
    value["unstaged"] = true.into();
    assert!(decode(value, &target()).is_err());
    let mut value = wire();
    value.as_object_mut().unwrap().remove("unstaged");
    assert!(
        decode(value, &target()).is_err(),
        "older responses are not silently upgraded"
    );
}
