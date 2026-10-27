use super::*;
use json_codec::json;

fn baseline() -> State {
    let mut state = State::default();
    for (branch, history, file) in [("b1", "h1", "f1"), ("b2", "h2", "f2")] {
        state.branches.insert(
            branch.into(),
            json_codec::from_value(json!({
                "branch_id": branch, "history_tree_id": history, "name": branch,
                "head_commit_id": "commit", "fork_commit_id": "commit",
                "branch_kind": "file", "file_id": file, "generation": "1", "state": "active"
            }))
            .unwrap(),
        );
    }
    state
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

#[test]
fn stable_changes_are_grouped_by_real_history_and_ignore_independent_fields() {
    let before = baseline();
    let mut after = candidate(&before);
    after.branches.get_mut("b2").unwrap().name = "new second".into();
    after.branches.get_mut("b1").unwrap().name = "new first".into();
    after.operations = Default::default();
    after.operation_principals = Default::default();
    after.plans = Default::default();
    after.sequence = 15;
    after.business_generation = 7;
    after.topology_generation = 2;
    after.control_auth = Some(json!({"authentication": "not a history projection"}));
    let ProjectionDelta::StableBranches(edits) = ProjectionDelta::between(&before, &after) else {
        panic!("precise branch writes should be incrementally projectable");
    };
    assert_eq!(edits.len(), 2);
    assert_eq!(edits["h1"].len(), 1);
    assert_eq!(edits["h2"].len(), 1);
    assert_eq!(edits["h1"]["b1"], after.branches["b1"]);
    assert_eq!(edits["h2"]["b2"], after.branches["b2"]);
}

#[test]
fn unchanged_or_same_value_branch_writes_have_no_projection_edits() {
    let before = baseline();
    for same_value_write in [false, true] {
        let mut after = candidate(&before);
        if same_value_write {
            after
                .branches
                .insert("b1".into(), before.branches["b1"].clone());
        }
        let ProjectionDelta::StableBranches(edits) = ProjectionDelta::between(&before, &after)
        else {
            panic!("no semantic branch change is not a full projection");
        };
        assert!(edits.is_empty());
    }
}

#[test]
fn ownership_membership_and_unknown_mutations_require_complete_projection() {
    let before = baseline();
    for change in 0..6 {
        let mut after = candidate(&before);
        match change {
            0 => after.branches.get_mut("b1").unwrap().history_tree_id = "h2".into(),
            1 => after.branches.get_mut("b1").unwrap().file_id = Some("other-file".into()),
            2 => {
                after.branches.remove("b1");
            }
            3 => {
                let mut added = before.branches["b1"].clone();
                added.branch_id = "b3".into();
                after.branches.insert("b3".into(), added);
            }
            4 => after.branches.values_mut().next().unwrap().name = "untracked mutation".into(),
            5 => {
                after.branches = before
                    .branches
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                ProjectionDelta::between(&before, &after),
                ProjectionDelta::Full
            ),
            "case {change} must not lose projection dependencies"
        );
    }
}

#[test]
fn every_other_projection_field_requires_complete_projection_when_lineage_changes() {
    let before = baseline();
    macro_rules! check {
        (branch { $($branch:ident,)* }
         history { $($history:ident,)* }
         repository { $($repository:ident,)* }
         version { $($version:ident,)* }
         file { $($file:ident,)* }
         directory { $($directory:ident,)* }
         special { $($special:ident,)* }
         independent { $($independent:ident,)* }) => {
            check!(@fields $($history,)* $($repository,)* $($version,)* $($file,)* $($directory,)* $($special,)*);
        };
        (@fields $($field:ident,)*) => {$(
            let mut after = candidate(&before);
            after.branches.get_mut("b1").unwrap().name = "rename".into();
            after.$field = Default::default();
            assert!(
                matches!(ProjectionDelta::between(&before, &after), ProjectionDelta::Full),
                concat!(stringify!($field), " replacement must not be ignored")
            );)*};
    }
    projection_fields!(check);
}
