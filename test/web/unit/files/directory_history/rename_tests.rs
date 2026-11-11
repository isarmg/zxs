use super::*;
use crate::files::test_entries::branch;
use json_codec::json;

fn id(prefix: &str, value: u8) -> String {
    format!("{prefix}_{value:032x}")
}

fn state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.loaded = true;
    state.files.writable = true;
    state.files.directory_branches_supported = true;
    state.files.generation = 9;
    state.files.slot = "P01".into();
    state.files.directory = "Repository/folder".into();
    state.files.repository_id = id("repo", 1);
    state.files.repository_path = "folder".into();
    state.files.namespace_head = id("commit", 1);
    state.files.directory_branches = vec![branch(json!({
        "directory_branch_id":id("dbr", 1), "repository_id":id("repo", 1),
        "path":"folder", "current":true,
    }))];
    state
}

fn target() -> Target {
    Target {
        epoch: 7,
        repository: id("repo", 1),
        directory: id("entry", 1),
        slot: "P01".into(),
        path: "Repository/folder".into(),
        preview: false,
        scope: Scope::CurrentDirectory {
            initialized: true,
            generation: 9,
            repository_path: "folder".into(),
            namespace_head: id("commit", 1),
        },
    }
}

#[test]
fn history_row_renames_its_noncurrent_identity_not_the_viewed_or_working_list_branch() {
    let state = state();
    let target = target();
    assert!(target.can_manage_branches(&state));
    let mut row = DirectoryHistoryBranch {
        committed_tree_id: Some(id("dtree", 3)),
        staged: false,
        directory_branch_id: id("dbr", 3),
        name: "Original label".into(),
        parent_branch_id: Some(id("dbr", 1)),
        head_tree_id: id("dtree", 4),
        materialized: true,
        current: false,
        sequence: 1,
    };
    assert!(state.files.directory_branches.iter().all(|b| b.id() != row.directory_branch_id));
    let Some(inline::Target::RenameDirectoryBranch {
        repository,
        branch_id,
        name,
    }) = rename_target(&target, &row)
    else {
        panic!("a writable noncurrent history row has a rename editor");
    };
    assert_eq!(repository, id("repo", 1));
    assert_eq!(branch_id, id("dbr", 3));
    assert_eq!(name, "Original label");
    row.current = true;
    assert!(rename_target(&target, &row).is_none());
}

#[test]
fn same_repository_old_history_target_and_late_read_generation_cannot_admit_a_rename() {
    let original = state();
    let target = target();
    for change in 0..9 {
        let mut next = original.clone();
        match change {
            0 => next.files.generation += 1,
            1 => next.files.directory = "Repository/other".into(),
            2 => next.files.repository_path = "other".into(),
            3 => next.files.namespace_head = id("commit", 2),
            4 => next.session_epoch += 1,
            5 => next.session_transition = crate::ui::SessionTransition::LoggingOut,
            6 => next.files.loading = true,
            7 => next.files.writable = false,
            8 => next.files.operations.push(crate::files::model::FileOperation::unknown("old")),
            _ => unreachable!(),
        }
        assert_eq!(next.files.repository_id, target.repository);
        assert!(!target.can_manage_branches(&next), "{change}");
    }
    Owner::new().with(|| {
        let state = RwSignal::new(original);
        let model = RwSignal::new(Model::default());
        assert!(owned(state, model, &target, 0));
        model.update(Model::cancel);
        assert!(!owned(state, model, &target, 0));
        assert!(owned(state, model, &target, 1));
        state.update(|s| s.files.generation += 1);
        assert!(!owned(state, model, &target, 1));
    });
}

