//! Full-scan relation oracle, independent of the production dependency updater.
use super::*;
use crate::{Branch, BranchKind, BranchState, FileContext, FileVersion};
use json_codec::json;

#[derive(Default, Debug, PartialEq, Eq)]
struct Membership {
    versions_by_history: BTreeMap<String, BTreeSet<String>>,
    pending_branches: BTreeSet<String>,
    path_nodes: BTreeSet<String>,
    metadata_nodes: BTreeSet<String>,
    merges: BTreeSet<String>,
    contexts: BTreeSet<String>,
    aliases: BTreeSet<String>,
}

fn full_memberships(state: &State) -> BTreeMap<String, Membership> {
    let mut expected = BTreeMap::<String, Membership>::new();
    for (id, version) in state.versions.iter() {
        let file = expected.entry(version.file_id.clone()).or_default();
        file.versions_by_history
            .entry(version.history_tree_id.clone())
            .or_default()
            .insert(id.clone());
    }
    for (id, branch) in state.branches.iter() {
        for file in branch.pending_file_versions.keys() {
            expected
                .entry(file.clone())
                .or_default()
                .pending_branches
                .insert(id.clone());
        }
    }
    for (id, node) in state.file_path_nodes.iter() {
        expected
            .entry(node.file_id.clone())
            .or_default()
            .path_nodes
            .insert(id.clone());
    }
    for (id, node) in state.file_metadata_nodes.iter() {
        expected
            .entry(node.file_id.clone())
            .or_default()
            .metadata_nodes
            .insert(id.clone());
    }
    for (id, event) in state.file_branch_merges.iter() {
        expected
            .entry(event.file_id.clone())
            .or_default()
            .merges
            .insert(id.clone());
    }
    for (key, context) in state.file_contexts.iter() {
        if let Some(file) = state
            .branches
            .get(&context.branch_id)
            .and_then(|branch| branch.file_id.as_ref())
        {
            expected
                .entry(file.clone())
                .or_default()
                .contexts
                .insert(key.clone());
        }
    }
    for source in state.file_branch_aliases.keys() {
        if let Some(file) = state
            .branches
            .get(source)
            .and_then(|branch| branch.file_id.as_ref())
        {
            expected
                .entry(file.clone())
                .or_default()
                .aliases
                .insert(source.clone());
        }
    }
    expected
}

fn assert_memberships(state: &State, index: &DerivedOwners) {
    let mut expected = full_memberships(state);
    // Query former owners too, so a retained stale membership cannot hide behind
    // its absence from the authoritative map after the final reference is removed.
    for file in ["file-a", "file-b", "file-c", "file-d", "absent-file"] {
        expected.entry(file.to_owned()).or_default();
    }
    for (file, expected) in expected {
        assert_eq!(
            index.first_version(&file),
            state
                .versions
                .iter()
                .find(|(_, version)| version.file_id == file)
                .map(|(id, _)| id),
            "first version for {file}"
        );
        let mut actual = Membership {
            versions_by_history: index
                .files
                .0
                .get(&file)
                .into_iter()
                .flat_map(|histories| histories.iter())
                .map(|(history, versions)| (history.clone(), versions.keys().cloned().collect()))
                .collect(),
            ..Membership::default()
        };
        if let Some(rows) = index.file_rows(&file) {
            actual.pending_branches = rows.pending_branches.keys().cloned().collect();
            actual.path_nodes = rows.path_nodes.keys().cloned().collect();
            actual.metadata_nodes = rows.metadata_nodes.keys().cloned().collect();
            actual.merges = rows.merges.keys().cloned().collect();
            actual.contexts = rows.contexts.keys().cloned().collect();
            actual.aliases = rows.aliases.keys().cloned().collect();
        }
        assert_eq!(actual, expected, "file membership for {file}");
    }
}

fn record<T: data_serialization::de::DeserializeOwned>(value: Value) -> T {
    json_codec::from_value(value).unwrap()
}

fn version(id: &str, file: &str, history: &str) -> FileVersion {
    record(json!({
        "history_tree_id":history, "version_id":id, "file_id":file,
        "creation_primary_slot_id":"P01", "logical_size":"0", "version_sequence":"1",
        "content_hash":"a".repeat(64), "checksum_chunk_bytes":65536,
        "checksum_manifest_hash":"hash", "creation_name":"file", "storage_name":"file",
        "created_at":"now", "source_version_ids":[], "created_as_full_checkpoint":true
    }))
}

fn branch(id: &str, file: Option<&str>, history: &str) -> Branch {
    Branch {
        branch_id: id.into(),
        history_tree_id: history.into(),
        name: id.into(),
        head_commit_id: "commit".into(),
        fork_commit_id: "commit".into(),
        branch_kind: if file.is_some() {
            BranchKind::File
        } else {
            BranchKind::Full
        },
        file_id: file.map(str::to_owned),
        file_node_id: None,
        pending_file_versions: BTreeMap::new(),
        generation: 1,
        state: BranchState::Active,
    }
}

// Dependency-only records intentionally include one file in two histories. They
// do not weaken RawState's independent rejection of that external snapshot.
fn fixture() -> State {
    let mut state = State::default();
    for (id, file, history) in [
        ("v-a1", "file-a", "h1"),
        ("v-a2", "file-a", "h1"),
        ("v-a3", "file-a", "h2"),
        ("v-b", "file-b", "h3"),
    ] {
        state.versions.insert(id.into(), version(id, file, history));
    }
    for (id, file, history) in [
        ("b-a", Some("file-a"), "h1"),
        ("b-b", Some("file-b"), "h3"),
        ("b-root", None, "h1"),
    ] {
        state.branches.insert(id.into(), branch(id, file, history));
    }
    state.branches.get_mut("b-a").unwrap().pending_file_versions =
        BTreeMap::from([("file-a".into(), "v-a1".into())]);
    state
        .branches
        .get_mut("b-root")
        .unwrap()
        .pending_file_versions = BTreeMap::from([
        ("file-a".into(), "v-a2".into()),
        ("file-b".into(), "v-b".into()),
    ]);
    for (key, branch) in [("r/path-a", "b-a"), ("r/path-b", "b-b")] {
        state.file_contexts.insert(
            key.into(),
            FileContext {
                node_id: None,
                selection_generation: 0,
                branch_id: branch.into(),
                inherited_from: None,
            },
        );
    }
    state.file_branch_aliases.insert("b-a".into(), "b-b".into());
    state.file_path_nodes.insert(
        "path-a".into(),
        record(json!({
            "node_id":"path-a", "history_tree_id":"h1", "file_id":"file-a", "version_id":"v-a1",
            "parent_node_ids":[], "branch_id":"b-a", "path":"new", "previous_path":"old",
            "operation_id":"op_00000000000000000000000000000000", "author":"author",
            "sequence":"1", "created_at":"1"
        })),
    );
    state.file_metadata_nodes.insert(
        "meta-a".into(),
        record(json!({
            "node_id":"meta-a", "history_tree_id":"h2", "file_id":"file-a", "version_id":"v-a3",
            "parent_node_ids":[], "branch_id":"b-a", "kind":"tags", "tags":["tag"],
            "operation_id":"op_00000000000000000000000000000000", "author":"author",
            "sequence":"1", "created_at":"1"
        })),
    );
    state.file_branch_merges.insert(
        "merge-a".into(),
        record(json!({
            "merge_id":"merge-a", "history_tree_id":"h1", "file_id":"file-a",
            "input_branch_ids":[], "input_node_ids":[], "chosen_node_id":"v-a1",
            "result_node_id":"v-a1", "surviving_branch_id":"b-a", "reused_node":true,
            "parent_node_ids":[], "operation_id":"op_00000000000000000000000000000000",
            "sequence":"1"
        })),
    );
    state
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

fn compare(before: &State, after: &State, index: &DerivedOwners) -> DerivedOwners {
    let transition = ProjectionTransition::new(
        before,
        after,
        Some(index),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    assert_memberships(before, transition.previous().owners);
    assert_memberships(after, transition.next().owners);
    assert_memberships(before, index);
    transition.next().owners.clone()
}

#[test]
fn every_container_membership_insert_delete_and_same_value_has_a_full_scan_oracle() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    assert_memberships(&before, &index);
    macro_rules! exercise {
        ($($field:ident),* $(,)?) => {$(
            for key in before.$field.keys() {
                let mut after = candidate(&before);
                after.$field.remove(key);
                let removed = compare(&before, &after, &index);
                let mut restored = candidate(&after);
                restored.$field.insert(key.clone(), before.$field[key].clone());
                compare(&after, &restored, &removed);
                let mut equal = candidate(&before);
                equal.$field.insert(key.clone(), before.$field[key].clone());
                compare(&before, &equal, &index);
            }
        )*};
    }
    exercise!(
        versions,
        branches,
        file_contexts,
        file_branch_aliases,
        file_path_nodes,
        file_metadata_nodes,
        file_branch_merges,
    );
}

#[test]
fn version_membership_keeps_multiple_references_and_removes_only_the_final_owner() {
    let mut before = fixture();
    let mut index =
        DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
            .unwrap();
    for id in ["v-a1", "v-a2", "v-a3"] {
        let mut after = candidate(&before);
        after.versions.remove(id);
        index = compare(&before, &after, &index);
        before = after;
        let expected = match id {
            "v-a1" => BTreeSet::from(["h1".to_owned(), "h2".to_owned()]),
            "v-a2" => BTreeSet::from(["h2".to_owned()]),
            "v-a3" => BTreeSet::new(),
            _ => unreachable!(),
        };
        assert_eq!(
            index
                .file_histories("file-a")
                .cloned()
                .collect::<BTreeSet<_>>(),
            expected
        );
    }
}

#[test]
fn swapping_version_histories_preserves_exact_buckets_through_successive_deletes() {
    let mut before = State::default();
    for (id, history) in [("v-a1", "h1"), ("v-a2", "h2")] {
        before
            .versions
            .insert(id.into(), version(id, "file-a", history));
    }
    let mut index =
        DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
            .unwrap();
    assert_memberships(&before, &index);

    // The flattened version set and history-owner set stay unchanged by this swap.
    let mut swapped = candidate(&before);
    swapped.versions.get_mut("v-a1").unwrap().history_tree_id = "h2".into();
    swapped.versions.get_mut("v-a2").unwrap().history_tree_id = "h1".into();
    index = compare(&before, &swapped, &index);
    before = swapped;

    for id in ["v-a1", "v-a2"] {
        let mut after = candidate(&before);
        after.versions.remove(id);
        index = compare(&before, &after, &index);
        before = after;
    }
    assert!(!index.files.0.contains_key("file-a"));
}

#[test]
fn moving_versions_nodes_and_branches_updates_old_and_new_file_memberships() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut after = candidate(&before);
    let moved = after.versions.get_mut("v-a1").unwrap();
    moved.file_id = "file-c".into();
    moved.history_tree_id = "h3".into();
    after.versions.get_mut("v-a2").unwrap().history_tree_id = "h2".into();
    after.file_path_nodes.get_mut("path-a").unwrap().file_id = "file-c".into();
    after.file_metadata_nodes.get_mut("meta-a").unwrap().file_id = "file-d".into();
    after.file_branch_merges.get_mut("merge-a").unwrap().file_id = "file-b".into();
    after.branches.get_mut("b-a").unwrap().file_id = Some("file-c".into());
    compare(&before, &after, &index);
}

#[test]
fn simultaneous_context_retarget_and_branch_reownership_preserve_source_alias_semantics() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut after = candidate(&before);
    after.branches.get_mut("b-a").unwrap().file_id = Some("file-c".into());
    after.branches.get_mut("b-b").unwrap().file_id = Some("file-d".into());
    after.file_contexts.get_mut("r/path-a").unwrap().branch_id = "b-b".into();
    after.file_contexts.remove("r/path-b");
    after.file_contexts.insert(
        "another/history/path".into(),
        FileContext {
            node_id: None,
            selection_generation: 7,
            branch_id: "b-a".into(),
            inherited_from: Some("b-root".into()),
        },
    );
    let next = compare(&before, &after, &index);
    assert_eq!(
        next.file_rows("file-c")
            .unwrap()
            .aliases
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["b-a"]
    );
    assert!(next.file_rows("file-d").unwrap().aliases.is_empty());
}

#[test]
fn pending_membership_includes_non_file_branches_and_tracks_last_reference_removal() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut after = candidate(&before);
    let pending = &mut after
        .branches
        .get_mut("b-root")
        .unwrap()
        .pending_file_versions;
    pending.remove("file-a");
    pending.insert("file-c".into(), "v-a1".into());
    after
        .branches
        .get_mut("b-a")
        .unwrap()
        .pending_file_versions
        .insert("file-a".into(), "v-a2".into());
    let next = compare(&before, &after, &index);
    assert_eq!(
        next.file_rows("file-c")
            .unwrap()
            .pending_branches
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["b-root"]
    );
    let mut removed = candidate(&after);
    removed.branches.remove("b-root");
    compare(&after, &removed, &next);
}

#[test]
fn cold_foreign_all_and_overflow_rebuild_memberships_from_the_authoritative_states() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut cold: State = json_codec::from_value(json_codec::to_value(&before).unwrap()).unwrap();
    cold.versions.get_mut("v-a1").unwrap().file_id = "file-c".into();
    let transition = ProjectionTransition::new(
        &before,
        &cold,
        None,
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    assert_memberships(&cold, transition.next().owners);
    let mut foreign = candidate(&before);
    foreign.versions = before
        .versions
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    foreign.versions.remove("v-a3");
    compare(&before, &foreign, &index);
    let mut all = candidate(&before);
    for branch in all.branches.values_mut() {
        branch.file_id = Some("file-d".into());
    }
    compare(&before, &all, &index);
    let mut overflow = candidate(&before);
    for number in 0..=16_384 {
        let key = format!("overflow/{number}");
        overflow.file_contexts.insert(
            key,
            FileContext {
                node_id: None,
                selection_generation: 0,
                branch_id: "b-a".into(),
                inherited_from: None,
            },
        );
    }
    compare(&before, &overflow, &index);
}

fn sharing_fixture() -> State {
    let mut state = fixture();
    state.histories.insert(
        "h1".into(),
        record(json!({
            "history_tree_id":"h1", "repository_id":"r1", "root_commit_id":"commit",
            "main_branch_id":"b-a", "generation":"1", "state":"active", "provenance":null
        })),
    );
    state.repositories.insert(
        "r1".into(),
        record(json!({
            "repository_id":"r1", "name":"repository", "history_tree_id":"h1",
            "main_branch_id":"b-a", "primary_slot_id":"P01", "target_copies":1,
            "name_policy":"portable", "unicode_version":"15.1", "smb_enabled":false,
            "smb_binding_generation":"1"
        })),
    );
    state.directory_branches.insert("directory-branch".into(), record(json!({
        "directory_branch_id":"directory-branch", "repository_id":"r1", "directory_id":"directory",
        "path":"directory", "name":"directory", "parent_branch_id":null, "head_tree_id":"tree",
        "commit_id":"commit", "entries":[], "file_branches":{}, "materialized":true, "sequence":"1"
    })));
    state.directory_trees.insert("tree".into(), record(json!({
        "tree_id":"tree", "directory_branch_id":"directory-branch", "parent_tree_id":null,
        "commit_id":"commit", "path":"directory", "entries":[], "file_branches":{}, "sequence":"1"
    })));
    state
}

#[test]
fn branch_label_only_changes_preserve_untouched_membership_backing() {
    let before = sharing_fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut after = candidate(&before);
    after.branches.get_mut("b-a").unwrap().name = "new branch label".into();
    let transition = ProjectionTransition::new(
        &before,
        &after,
        Some(&index),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let next = transition.next().owners;
    assert!(index.history_rows.shares_storage_with(&next.history_rows));
    assert!(index.files.0.shares_storage_with(&next.files.0));
    assert!(index.directories.0.shares_storage_with(&next.directories.0));
    assert_memberships(&after, next);
}

#[test]
fn control_and_unowned_rows_do_not_rebuild_unchanged_relations() {
    let before = sharing_fixture();
    let index = DerivedOwners::from_state(&before, &crate::namespace::fixture_store::FixtureSource)
        .unwrap();
    let mut after = candidate(&before);
    after.control_auth = Some(json!({"control":"changed"}));
    after.sequence += 1;
    after
        .file_notes
        .insert("absent-file".into(), "unowned note".into());
    let transition = ProjectionTransition::new(
        &before,
        &after,
        Some(&index),
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    let next = transition.next().owners;
    assert!(
        index
            .repository_rows
            .shares_storage_with(&next.repository_rows)
    );
    assert!(index.history_rows.shares_storage_with(&next.history_rows));
    assert!(
        index
            .directory_trees
            .0
            .shares_storage_with(&next.directory_trees.0)
    );
    assert_memberships(&after, next);
}

#[test]
fn first_version_uses_version_order_not_history_order() {
    let mut owners = DerivedOwners::default();
    owners.files.add("file", "history-a", "version-z");
    owners.files.add("file", "history-a", "version-m");
    owners.files.add("file", "history-z", "version-a");
    owners.files.add("unrelated", "history-0", "version-0");
    assert_eq!(
        owners.first_version("file").map(String::as_str),
        Some("version-a")
    );
    assert_eq!(owners.first_version("missing"), None);
    owners.files.remove("file", "history-z", "version-a");
    assert_eq!(
        owners.first_version("file").map(String::as_str),
        Some("version-m")
    );
    owners.files.remove("file", "history-a", "version-m");
    owners.files.remove("file", "history-a", "version-z");
    assert_eq!(owners.first_version("file"), None);
}
