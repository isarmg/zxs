use super::*;
use crate::{NamespaceEntryKind, OperationId};
use json_codec::{Value, json};
fn entry(path: &str, version: &str) -> Entry {
    Entry {
        entry_id: format!("entry-{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: version.into(),
        size: 7,
    }
}
fn commit(id: &str, entries: Vec<Entry>) -> Arc<Commit> {
    Arc::new(Commit {
        commit_id: id.into(),
        history_tree_id: "history".into(),
        kind: "open-kind".into(),
        parent_commit_ids: vec![],
        tree_id: "tree".into(),
        sequence: 17,
        operation_id: OperationId::generate(),
        author: "author".into(),
        message: "".into(),
        entries,
        pending_file_versions: Default::default(),
        namespace_deletions: None,
        provenance: None,
    })
}
fn old_changes(left: &Commit, right: &Commit) -> Value {
    let before: BTreeMap<_, _> = left
        .entries
        .iter()
        .map(|entry| (entry.lookup_key(), entry))
        .collect();
    let after: BTreeMap<_, _> = right
        .entries
        .iter()
        .map(|entry| (entry.lookup_key(), entry))
        .collect();
    let mut changes = Vec::new();
    for entry in &left.entries {
        match after.get(entry.lookup_key()) {
            None => changes.push(json!({"path":entry.path(),"change":"removed","before":entry})),
            Some(other) if entry.version_id != other.version_id || entry.path() != other.path() => {
                changes.push(
                    json!({"path":other.path(),"change":"modified","before":entry,"after":other}),
                )
            }
            _ => {}
        }
    }
    for entry in &right.entries {
        if !before.contains_key(entry.lookup_key()) {
            changes.push(json!({"path":entry.path(),"change":"added","after":entry}));
        }
    }
    json!(changes)
}
fn changes(comparison: &CommitComparison) -> Value {
    json!(
        comparison
            .changes()
            .map(|change| match change {
                CommitChange::Removed { before } =>
                    json!({"path":before.path(),"change":"removed","before":before}),
                CommitChange::Modified { before, after } =>
                    json!({"path":after.path(),"change":"modified","before":before,"after":after}),
                CommitChange::Added { after } =>
                    json!({"path":after.path(),"change":"added","after":after}),
            })
            .collect::<Vec<_>>()
    )
}
#[test]
fn comparison_preserves_order_keys_and_legacy_last_match_semantics() {
    let cases = [
        vec![],
        vec![entry("File.txt", "v1"), entry("remove.txt", "v2")],
        vec![entry("file.txt", "v1"), entry("new.txt", "v3")],
        vec![entry("é.txt", "v1"), entry("É.txt", "v2")],
        vec![entry("File.txt", "v1"), entry("file.txt", "v2")],
    ];
    for before in &cases {
        for after in &cases {
            let left = commit("left", before.clone());
            let right = commit("right", after.clone());
            let expected = old_changes(&left, &right);
            let comparison = CommitComparison::between(left, right);
            assert_eq!(changes(&comparison), expected);
            assert_eq!(
                comparison.changes().rev().count(),
                comparison.changes().len()
            );
        }
    }
}
#[test]
fn comparison_shares_entries_and_survives_owner_copy_on_write_and_drop() {
    let mut left = commit(
        "unparsed left id",
        vec![entry("file.txt", "old"), entry("removed.txt", "old")],
    );
    let right = commit(
        "",
        vec![entry("FILE.txt", "new"), entry("added.txt", "new")],
    );
    let comparison = CommitComparison::between(Arc::clone(&left), Arc::clone(&right));
    assert_eq!(Arc::strong_count(&left), 2);
    assert_eq!(Arc::strong_count(&right), 2);
    let mut changes = comparison.changes();
    let CommitChange::Modified { before, after } = changes.next().unwrap() else {
        panic!("expected modified")
    };
    assert!(std::ptr::eq(before, &left.entries[0]));
    assert!(std::ptr::eq(after, &right.entries[0]));
    let CommitChange::Removed { before } = changes.next().unwrap() else {
        panic!("expected removed")
    };
    assert!(std::ptr::eq(before, &left.entries[1]));
    let CommitChange::Added { after } = changes.next().unwrap() else {
        panic!("expected added")
    };
    assert!(std::ptr::eq(after, &right.entries[1]));
    assert_eq!(changes.len(), 0);
    let expected = old_changes(&left, &right);
    Arc::make_mut(&mut left).entries[0].version_id = "later".into();
    drop(left);
    drop(right);
    assert_eq!(comparison.left_id(), "unparsed left id");
    assert_eq!(comparison.right_id(), "");
    assert_eq!(self::changes(&comparison), expected);
}
#[test]
fn comparison_modified_semantics_do_not_expand_to_other_entry_metadata() {
    let left = commit("left", vec![entry("file.txt", "version")]);
    let mut changed = left.entries.clone();
    changed[0].entry_id = "other entry".into();
    changed[0].file_id = "other file".into();
    changed[0].size = 99;
    let right = commit("right", changed);
    assert_eq!(CommitComparison::between(left, right).changes().len(), 0);
}
