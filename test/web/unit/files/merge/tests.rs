use super::*;
use crate::files::history::tests::branch;
fn draft() -> Draft {
    Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        branches: vec![
            branch(json!({"branch_id":"left","file_node_id":"fnode-left","node_id":"fnode-left"})),
            branch(
                json!({"branch_id":"right","file_node_id":"fnode-right","node_id":"fnode-right"}),
            ),
            branch(
                json!({"branch_id":"alias","file_node_id":"fnode-old","node_id":"fnode-old","read_only":true,"alias_of":"left"}),
            ),
        ],
        nodes: vec![
            json!({"node_id":"fnode-unrelated","version_id":"body-old"}),
            json!({"node_id":"fnode-left","version_id":"body-left"}),
        ],
        selected: BTreeSet::from(["left".into(), "right".into()]),
        chosen: "fnode-unrelated".into(),
        survivor: String::new(),
    }
}

#[test]
fn navigation_discards_an_open_merge_review() {
    use crate::files::model::{Model, Navigation};
    for destination in [
        Navigation::Root,
        Navigation::Disk("B01".into()),
        Navigation::Directory("other".into()),
    ] {
        let mut model = Model {
            merge: Some(draft()),
            ..Default::default()
        };
        assert!(model.merge.is_some());
        model.navigate(destination);
        assert!(model.merge.is_none());
    }
}

#[test]
fn merging_requires_an_explicit_survivor_and_accepts_any_file_node() {
    let mut draft = draft();
    assert!(request(&draft).is_err());
    draft.survivor = "left".into();
    let body = request(&draft).unwrap();
    assert_eq!(body["chosen_node_id"], "fnode-unrelated");
    assert_eq!(body["inputs"][0]["expected_node_id"], "fnode-left");
    assert_eq!(body["surviving_branch_id"], "left");
    assert_eq!(body["confirmed"], true);
    assert!(body.get("content").is_none());
}
#[test]
fn aliases_and_missing_baselines_cannot_be_submitted() {
    let mut draft = draft();
    draft.survivor = "left".into();
    draft.selected.insert("alias".into());
    assert!(request(&draft).is_err());
    draft.selected.remove("alias");
    draft.branches[0] = branch(
        json!({"branch_id":"left","node_id":null,"version_id":null,"own_version_id":null,"materialized":false}),
    );
    assert!(request(&draft).is_err());
}

#[test]
fn availability_counts_only_admitted_writable_branches_and_keeps_loading_fences() {
    use crate::files::history::tests::{branch_wire, history};
    for (state, alias, read_only) in [
        ("active", None, false),
        ("active", Some("left"), true),
        ("archived", None, true),
        ("migrated", None, true),
        ("merged", Some("left"), true),
    ] {
        let mut model = Model {
            file_history: history(json!({"branches":[
                branch_wire(json!({"branch_id":"left"})),
                branch_wire(json!({"branch_id":"right","state":state,"alias_of":alias,"read_only":read_only}))
            ]})),
            ..Default::default()
        };
        assert_eq!(available(&model), !read_only);
        model.history_loading = true;
        assert!(!available(&model));
    }
}
