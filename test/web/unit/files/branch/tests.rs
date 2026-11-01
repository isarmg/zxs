use super::*;

#[test]
fn a_blank_name_cancels_but_nonempty_names_are_not_trimmed() {
    assert_eq!(branch_name(" \t\n "), Name::Empty);
    assert!(matches!(branch_name("  草稿  "), Name::Rejected(_)));
    assert!(matches!(branch_name(" draft 2 "), Name::Rejected(_)));
    assert_eq!(branch_name("draft 2"), Name::Valid("draft 2".into()));
}

#[test]
fn invalid_names_never_become_branch_requests() {
    for value in [
        ".",
        "..",
        "feature/a",
        "feature\\a",
        "a\nb",
        "a\n",
        "CON",
        "trailing.",
        "e\u{301}",
        &"稿".repeat(43),
        &"a".repeat(129),
    ] {
        assert!(matches!(
            branch_name(value),
            Name::Invalid | Name::Rejected(_)
        ));
    }
    assert!(matches!(
        branch_name(&format!("{}ab", "稿".repeat(42))),
        Name::Valid(_)
    ));
    assert!(matches!(branch_name(&"a".repeat(128)), Name::Valid(_)));
}
#[test]
fn branching_from_a_path_node_keeps_the_body_id_and_explicit_node_parent_separate() {
    let mut model = super::super::model::Model {
        selected: Some(crate::files::test_entries::entry(
            json_codec::json!({"file_id":"file","version_id":"ver-one","node_id":"fnode-rename"}),
        )),
        file_history: crate::files::history::tests::history(
            json_codec::json!({"current_node_id":"fnode-rename","version_id":"ver-one","nodes":[{"node_id":"fnode-rename","version_id":"ver-one","kind":"rename"}]}),
        ),
        ..Default::default()
    };
    assert_eq!(
        file_source(&model, &Target::HistoryNode("fnode-rename".into())),
        Some(("ver-one".into(), Some("fnode-rename".into())))
    );
    assert_eq!(
        file_source(&model, &Target::CurrentFileVersion),
        Some(("ver-one".into(), Some("fnode-rename".into())))
    );
    model.file_history.invalidate();
    assert_eq!(file_source(&model, &Target::CurrentFileVersion), None);
    assert_eq!(
        file_source(&model, &Target::HistoryNode("fnode-rename".into())),
        None
    );
}

#[test]
fn current_source_uses_the_admitted_summary_even_outside_the_node_page() {
    let model = super::super::model::Model {
        selected: Some(crate::files::test_entries::entry(json_codec::json!({
            "version_id":"old-body", "node_id":"old-node"
        }))),
        file_history: crate::files::history::tests::history(json_codec::json!({
            "version_id":"current-body", "current_node_id":"current-node", "nodes":[]
        })),
        ..Default::default()
    };
    assert_eq!(
        file_source(&model, &Target::CurrentFileVersion),
        Some(("current-body".into(), Some("current-node".into())))
    );
    assert!(model.file_history.nodes().is_empty());
}

#[test]
fn an_unloaded_history_does_not_admit_a_branch_from_cached_selected_content() {
    let model = super::super::model::Model {
        selected: Some(crate::files::test_entries::entry(
            json_codec::json!({"file_id":"file","version_id":"body","node_id":"current-node"}),
        )),
        ..Default::default()
    };
    assert_eq!(file_source(&model, &Target::CurrentFileVersion), None);
    assert_eq!(
        file_source(&model, &Target::HistoryNode("current-node".into())),
        None
    );
}

#[test]
fn a_missing_visible_version_never_uses_the_source_id_for_a_branch_request() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.selected = Some(crate::files::test_entries::entry(json_codec::json!({
                "file_id":"file", "materialized":false, "version_id":null, "source_version_id":""
            })));
        });
        let _ = Target::CurrentFileVersion.submit(state, "another".into());
        assert!(!state.with_untracked(|s| s.files.mutation_pending));
    });
}

#[test]
fn nonempty_branch_input_must_never_be_silently_trimmed() {
    for original in [" 草稿 ", "\u{a0}draft", "draft\u{2003}"] {
        assert_ne!(
            branch_name(original),
            Name::Valid(original.trim().to_owned())
        );
    }
}

#[test]
fn branch_nfc_errors_keep_original_and_candidate_without_producing_a_name() {
    for (original, candidate) in [("e\u{301}", "é"), ("\u{1100}\u{1161}", "가")] {
        let Name::Rejected(error) = branch_name(original) else {
            panic!("NFD branch name must wait for confirmation");
        };
        assert_eq!(error.name.as_bytes(), original.as_bytes());
        assert_eq!(error.candidate(), Some(candidate));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(branch_name(candidate), Name::Valid(candidate.into()));
    }
    for original in ["\u{a0}draft", "draft\u{2003}"] {
        let Name::Rejected(error) = branch_name(original) else {
            panic!("Unicode whitespace cannot be silently removed");
        };
        assert_eq!(error.name, original);
        assert!(matches!(
            error.source,
            name_policy::Error::UnsupportedCharacter(_)
        ));
    }
}
