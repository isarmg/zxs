use super::*;
use crate::files::history::tests::node;
use json_codec::json;

#[test]
fn content_note_is_the_title_without_inventing_metadata_events() {
    let record = node(json!({
        "kind":"content", "note":"not the body note", "body_note":"Saved draft",
        "sequence":"8"
    }));
    assert_eq!(file_title(&record), "Saved draft");
    assert_eq!(file_actions(&record), &[Action::Content]);
}

#[test]
fn metadata_note_is_the_title_not_the_reused_body_note() {
    let record = node(json!({
        "kind":"merge", "note":"Approved", "body_note":"old content note"
    }));
    assert_eq!(file_title(&record), "Approved");
    assert_eq!(file_actions(&record), &[Action::Merge]);
}

#[test]
fn empty_note_has_an_explicit_sequence_label_not_an_invented_comment() {
    let record = node(json!({"kind":"rename", "sequence":"42", "note":""}));
    assert_eq!(
        file_title(&record),
        format!("{} #42", t("无备注", "No note"))
    );
    assert_eq!(title("", 3), format!("{} #3", t("无备注", "No note")));
}

#[test]
fn full_title_is_not_truncated_or_split_at_a_utf8_boundary() {
    let note = "中文备注 😀 abcdefghijklmnopqrstuvwxyz";
    assert_eq!(title(note, 5), note);
    assert_eq!(title("  ", 5), "  ");
}

#[test]
fn same_parent_and_changed_basename_is_rename() {
    for (before, after) in [("one.txt", "two.txt"), ("目录/one", "目录/two")] {
        assert_eq!(path_actions(Some(before), Some(after)), &[Action::Rename]);
    }
}

#[test]
fn changed_parent_and_unchanged_basename_is_move() {
    for (before, after) in [("one.txt", "dir/one.txt"), ("甲/one", "乙/one")] {
        assert_eq!(path_actions(Some(before), Some(after)), &[Action::Move]);
    }
}

#[test]
fn one_path_event_can_show_both_move_and_rename() {
    assert_eq!(
        path_actions(Some("from/old.txt"), Some("to/new.txt")),
        &[Action::Move, Action::Rename]
    );
}

#[test]
fn missing_or_unchanged_paths_do_not_guess_a_more_specific_action() {
    for (before, after) in [
        (None, None),
        (None, Some("new")),
        (Some("old"), None),
        (Some("same"), Some("same")),
    ] {
        assert_eq!(path_actions(before, after), &[Action::PathChange]);
    }
}

#[test]
fn case_changes_are_not_normalized_away() {
    assert_eq!(path_actions(Some("file"), Some("File")), &[Action::Rename]);
    assert_eq!(
        path_actions(Some("DIR/file"), Some("dir/file")),
        &[Action::Move]
    );
}

#[test]
fn kind_and_saved_paths_not_comment_words_determine_actions() {
    for (kind, expected) in [
        ("content", Action::Content),
        ("merge", Action::Merge),
    ] {
        let record = node(
            json!({"kind":kind, "note":"Deleted; moved and renamed", "previous_path":"old", "path":"new"}),
        );
        assert_eq!(file_actions(&record), &[expected]);
    }
}

#[test]
fn rename_uses_the_admitted_path_pair() {
    let record = node(json!({"kind":"rename", "previous_path":"from/file", "path":"to/file"}));
    assert_eq!(file_actions(&record), &[Action::Move]);
}

#[test]
fn every_displayed_action_has_a_distinct_name_and_accessible_label() {
    let actions = [
        Action::Content,
        Action::Rename,
        Action::Move,
        Action::PathChange,
        Action::Merge,
        Action::Merge,
    ];
    let names = actions
        .iter()
        .map(|action| action.name())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names.len(), actions.len());
    for action in actions {
        assert!(!action.label().is_empty());
        assert_ne!(action.name(), "delete");
    }
}
