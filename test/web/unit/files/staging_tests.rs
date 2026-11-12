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
    let (_, model) = reviewed(value.clone());
    let status = model.status.as_ref().unwrap();
    assert!(status.initialized && status.head_tree_id.is_none() && status.staged);
    assert_eq!(status.index_generation, 1);

    value["staged"] = false.into();
    assert_eq!(
        decode(value.clone(), &target()).unwrap_err().code,
        storage_contract::ErrorCode::InvalidResponse
    );

    value["head_tree_id"] = id("dtree", 1).into();
    value["index_generation"] = "2".into();
    let (_, model) = reviewed(value);
    let status = model.status.as_ref().unwrap();
    assert_eq!(
        status.head_tree_id.as_deref(),
        Some(id("dtree", 1).as_str())
    );
    assert!(!status.staged);
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
fn uninitialized_directory_context_rejects_invalid_ownership_and_branch_fields() {
    let status = decode(uninitialized(), &target()).unwrap();
    assert!(!status.initialized);
    let owner = status.owner.unwrap();
    assert_eq!(owner.directory_id, id("repo", 1));
    assert_eq!(owner.path, "");
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
fn child_reference_is_explicit_and_never_inferred_from_parent_or_dirty_work() {
    let mut value = wire();
    let mut pending = child();
    pending["head_tree_id"] = json!(null);
    pending["staged_tree_id"] = json!(null);
    pending["committed_tree_id"] = json!(null);
    pending["reference_changed"] = false.into();
    pending["staged"] = true.into();
    value["children"] = json!([pending]);
    let (_, model) = reviewed(value);
    assert!(model.status.as_ref().unwrap().children[0].staged);
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
        assert!(model.ready(&state).is_none(), "change {change}");
    }
    let mut changed = model.clone();
    changed.reading = true;
    assert!(changed.ready(&original).is_none());
    changed.reading = false;
    changed.error = Some(invalid());
    assert!(changed.ready(&original).is_none());
}
#[test]
fn repeated_reads_have_distinct_tokens_and_navigation_discards_old_context() {
    let (_, mut model) = reviewed(wire());
    let target = target();
    let first = model.begin(Some(target.clone()));
    let second = model.begin(Some(target.clone()));
    assert!(!model.current(&target, first));
    assert!(model.current(&target, second));
    assert!(
        model.status.is_some(),
        "failed refresh retains old read-only context"
    );
    let mut next = target;
    next.path = "another".into();
    model.begin(Some(next));
    assert!(model.status.is_none() && model.review_target.is_none());
}
#[test]
fn shared_directory_context_requires_fresh_scope_session_and_write_fences() {
    let (original, model) = reviewed(wire());
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
        assert!(model.ready(&state).is_none(), "change {change}");
    }
}

#[test]
fn directory_status_keeps_required_work_and_boundary_fields() {
    for field in ["unstaged", "needs_restage"] {
        let mut value = uninitialized();
        value[field] = true.into();
        assert!(decode(value, &target()).is_err());
        let mut value = wire();
        value.as_object_mut().unwrap().remove(field);
        assert!(decode(value, &target()).is_err());
    }
    let mut value = wire();
    value["children"] = json!([child()]);
    value["children"][0]["needs_restage"] = true.into();
    assert!(decode(value, &target()).unwrap().children[0].needs_restage);
}

#[test]
fn initialized_repository_cannot_supply_a_zero_source_generation() {
    let mut value = wire();
    value["index_generation"] = "0".into();
    assert!(decode(value, &target()).is_err());
}
