use super::*;

#[test]
fn a_blank_name_cancels_and_valid_names_are_trimmed() {
    assert_eq!(branch_name(" \t\n "), Name::Empty);
    assert_eq!(branch_name("  草稿  "), Name::Valid("草稿".into()));
    assert_eq!(branch_name(" draft 2 "), Name::Valid("draft 2".into()));
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
        assert_eq!(branch_name(value), Name::Invalid);
    }
    assert!(matches!(
        branch_name(&format!("{}ab", "稿".repeat(42))),
        Name::Valid(_)
    ));
    assert!(matches!(branch_name(&"a".repeat(128)), Name::Valid(_)));
}
#[test]
fn branching_from_a_path_node_keeps_the_body_id_and_explicit_node_parent_separate() {
    let model = super::super::model::Model {
        selected: Some(crate::files::test_entries::entry(
            json_codec::json!({"file_id":"file","version_id":"ver-one","node_id":"fnode-rename"}),
        )),
        file_history: crate::files::history::tests::history(
            json_codec::json!({"nodes":[{"node_id":"fnode-rename","version_id":"ver-one","kind":"rename"}]}),
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
