use super::*;
use crate::files::history::tests::branch;
#[test]
fn tag_deltas_preserve_heterogeneous_inherited_labels_and_do_not_mutate_the_parent() {
    let node = json!({"tags":["old","keep"]});
    let updated = result_tags(
        &node,
        &BTreeSet::from(["new".into()]),
        &BTreeSet::from(["old".into()]),
    );
    assert_eq!(updated, BTreeSet::from(["keep".into(), "new".into()]));
    assert_eq!(node["tags"], json!(["old", "keep"]));
    assert!(valid_tag(&format!("{}ab", "文".repeat(42))));
    assert!(!valid_tag(&"文".repeat(43)));
    assert!(!valid_tag("bad\nlabel"));
}
#[test]
fn requests_distinguish_historical_source_nodes_from_frozen_branch_heads() {
    let draft = Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        nodes: vec![json!({"node_id":"old-node","version_id":"old-body","tags":["inherited"]})],
        branches: vec![branch(
            json!({"branch_id":"main","file_node_id":"newer-head","node_id":"newer-head"}),
        )],
        targets: vec![Target {
            source: "old-node".into(),
            branch: "main".into(),
            expected: "newer-head".into(),
        }],
        add: BTreeSet::from(["reviewed".into()]),
        remove: BTreeSet::new(),
        input: String::new(),
    };
    let body = request(&draft).unwrap();
    assert_eq!(body["targets"][0]["source_node_id"], "old-node");
    assert_eq!(body["targets"][0]["expected_node_id"], "newer-head");
    assert_eq!(body["add"], json!(["reviewed"]));
    assert!(body.get("note").is_none());
}

#[test]
fn one_invalid_tag_target_blocks_the_entire_batch() {
    let labels = (0..storage_contract::MAX_FILE_TAGS)
        .map(|index| format!("tag-{index}"))
        .collect::<Vec<_>>();
    let mut draft = Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        nodes: vec![
            json!({"node_id":"old","tags":[]}),
            json!({"node_id":"full","tags":labels}),
        ],
        branches: vec![branch(
            json!({"branch_id":"main","file_node_id":"head","node_id":"head"}),
        )],
        targets: vec![
            Target {
                source: "old".into(),
                branch: "main".into(),
                expected: "head".into(),
            },
            Target {
                source: "full".into(),
                branch: "main".into(),
                expected: "head".into(),
            },
        ],
        add: BTreeSet::from(["extra".into()]),
        remove: BTreeSet::new(),
        input: String::new(),
    };
    assert!(
        request(&draft).is_err(),
        "a valid first target must not hide a full second target"
    );
    draft.remove.insert("tag-0".into());
    assert_eq!(
        request(&draft).unwrap()["targets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    draft.branches[0] = branch(
        json!({"branch_id":"main","file_node_id":"head","node_id":"head","alias_of":"survivor","read_only":true}),
    );
    assert!(
        request(&draft).is_err(),
        "admitted read-only aliases cannot target edits"
    );
}

fn draft_with_pending_input(input: &str) -> Draft {
    Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        nodes: vec![json!({"node_id":"source","tags":["old"]})],
        branches: vec![branch(
            json!({"branch_id":"main","file_node_id":"head","node_id":"head"}),
        )],
        targets: vec![Target {
            source: "source".into(),
            branch: "main".into(),
            expected: "head".into(),
        }],
        add: BTreeSet::from(["reviewed".into()]),
        remove: BTreeSet::from(["old".into()]),
        input: input.into(),
    }
}

#[test]
fn pending_tag_input_cannot_be_silently_dropped_from_a_valid_delta() {
    for input in ["another", "  another  ", "bad\nlabel", &"a".repeat(129)] {
        let draft = draft_with_pending_input(input);
        let error = request(&draft).expect_err("pending input must block the whole request");
        assert!(error.contains("Add tag"));
        assert_eq!(draft.input, input);
        assert_eq!(draft.add, BTreeSet::from(["reviewed".into()]));
        assert_eq!(draft.remove, BTreeSet::from(["old".into()]));
    }
    for input in ["", "   "] {
        let body = request(&draft_with_pending_input(input)).unwrap();
        assert_eq!(body["add"], json!(["reviewed"]));
        assert_eq!(body["remove"], json!(["old"]));
    }
}

#[test]
fn pending_tag_input_rejection_preserves_an_editable_draft_without_starting_an_operation() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.storage = crate::management::Storage::Ready;
            s.files.writable = true;
            s.files.tag_edit = Some(draft_with_pending_input("  another  "));
            s.files.checked_nodes.insert("source".into());
        });
        // Validation must stop before review::submit generates an operation ID
        // or schedules a browser request. This path also runs in native tests.
        submit(state);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.input, "  another  ");
            assert_eq!(draft.targets[0].source, "source");
            assert_eq!(draft.targets[0].branch, "main");
            assert_eq!(draft.targets[0].expected, "head");
            assert!(s.files.checked_nodes.contains("source"));
            assert!(s.files.operations.is_empty());
            assert!(!s.files.mutation_pending);
            assert!(
                s.files.publications[&review::Kind::Tags]
                    .operation_id
                    .is_none()
            );
            assert!(!review::error_text(s, review::Kind::Tags).is_empty());
            assert!(actions::write_allowed(s));
        });

        // The user can explicitly add the pending tag and keep the prior delta.
        add_input(state);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert!(draft.input.is_empty());
            let body = request(draft).unwrap();
            assert_eq!(body["add"], json!(["another", "reviewed"]));
            assert_eq!(body["remove"], json!(["old"]));
            assert!(s.files.operations.is_empty());
        });
    });
}

#[test]
fn archived_migrated_and_merged_branch_targets_cannot_be_submitted() {
    for state in ["archived", "migrated", "merged"] {
        let mut draft = draft_with_pending_input("");
        draft.branches[0] = branch(
            json!({"branch_id":"main", "file_node_id":"head", "node_id":"head", "state":state, "read_only":true}),
        );
        assert!(request(&draft).is_err());
    }
}
