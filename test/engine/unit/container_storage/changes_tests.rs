//! Independent full-scan oracle retained from the pre-write-set implementation.
use super::*;

fn full_changed_files(previous: &State, next: &State) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for (id, node) in previous
        .file_metadata_nodes
        .iter()
        .chain(&next.file_metadata_nodes)
    {
        if !previous.file_metadata_nodes.contains_key(id)
            || !next.file_metadata_nodes.contains_key(id)
        {
            files.insert(node.file_id.clone());
        }
    }
    for (id, event) in previous
        .file_branch_merges
        .iter()
        .chain(next.file_branch_merges.iter())
    {
        if !previous.file_branch_merges.contains_key(id)
            || !next.file_branch_merges.contains_key(id)
        {
            files.insert(event.file_id.clone());
        }
    }
    for file in previous
        .file_backup_paths
        .keys()
        .chain(next.file_backup_paths.keys())
    {
        if previous.file_backup_paths.get(file) != next.file_backup_paths.get(file) {
            files.insert(file.clone());
        }
    }
    for file in previous
        .file_backup_policies
        .keys()
        .chain(next.file_backup_policies.keys())
    {
        if previous.file_backup_policies.get(file) != next.file_backup_policies.get(file) {
            files.insert(file.clone());
        }
    }
    for (id, node) in previous
        .file_path_nodes
        .iter()
        .chain(next.file_path_nodes.iter())
    {
        if !previous.file_path_nodes.contains_key(id) || !next.file_path_nodes.contains_key(id) {
            files.insert(node.file_id.clone());
        }
    }
    for id in previous
        .file_path_notes
        .keys()
        .chain(next.file_path_notes.keys())
    {
        if previous.file_path_notes.get(id) != next.file_path_notes.get(id) {
            if let Some(node) = next
                .file_path_nodes
                .get(id)
                .or_else(|| previous.file_path_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            } else if let Some(node) = next
                .file_metadata_nodes
                .get(id)
                .or_else(|| previous.file_metadata_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            }
        }
    }
    for id in previous
        .file_node_parents
        .keys()
        .chain(next.file_node_parents.keys())
    {
        if previous.file_node_parents.get(id) != next.file_node_parents.get(id)
            && let Some(version) = next.versions.get(id).or_else(|| previous.versions.get(id))
        {
            files.insert(version.file_id.clone());
        }
    }
    for key in previous
        .file_locations
        .keys()
        .chain(next.file_locations.keys())
    {
        if previous.file_locations.get(key) != next.file_locations.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous.file_names.keys().chain(next.file_names.keys()) {
        if previous.file_names.get(key) != next.file_names.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous.file_notes.keys().chain(next.file_notes.keys()) {
        if previous.file_notes.get(key) != next.file_notes.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous
        .file_selections
        .keys()
        .chain(next.file_selections.keys())
    {
        if previous.file_selections.get(key) != next.file_selections.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous
        .file_contexts
        .keys()
        .chain(next.file_contexts.keys())
    {
        if previous.file_contexts.get(key) != next.file_contexts.get(key) {
            for (state, context) in previous
                .file_contexts
                .get(key)
                .map(|context| (previous, context))
                .into_iter()
                .chain(next.file_contexts.get(key).map(|context| (next, context)))
            {
                if let Some(file) = state
                    .branches
                    .get(&context.branch_id)
                    .and_then(|branch| branch.file_id.as_ref())
                {
                    files.insert(file.clone());
                }
            }
        }
    }
    for key in previous
        .version_notes
        .keys()
        .chain(next.version_notes.keys())
    {
        if previous.version_notes.get(key) != next.version_notes.get(key)
            && let Some(version) = next
                .versions
                .get(key)
                .or_else(|| previous.versions.get(key))
        {
            files.insert(version.file_id.clone());
        }
    }
    for (id, version) in previous.versions.iter().chain(next.versions.iter()) {
        let changed_placement = match (previous.placements.get(id), next.placements.get(id)) {
            (Some(before), Some(after)) => {
                before.route_slot_ids != after.route_slot_ids
                    || before.binding_generations != after.binding_generations
                    || before.primary_slot_id != after.primary_slot_id
                    || before.target_copies != after.target_copies
            }
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
        };
        if !previous.versions.contains_key(id)
            || !next.versions.contains_key(id)
            || changed_placement
            || previous.content_states.get(id) != next.content_states.get(id)
        {
            files.insert(version.file_id.clone());
        }
    }
    // An inherited branch has no new FileVersion. Its reference metadata must
    // still update the cache, including removal after the first owned write.
    for id in previous.branches.keys().chain(next.branches.keys()) {
        let before = previous.branches.get(id);
        let after = next.branches.get(id);
        let unchanged = match (before, after) {
            (Some(before), Some(after)) => {
                before.pending_file_versions == after.pending_file_versions
                    && before.file_node_id == after.file_node_id
                    && before.name == after.name
                    && before.state == after.state
                    && before.history_tree_id == after.history_tree_id
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        if !unchanged {
            for branch in before.into_iter().chain(after) {
                files.extend(branch.pending_file_versions.keys().cloned());
            }
        }
    }
    files
}

use json_codec::json;
use storage_contract::{BranchState, ContentState};

fn record<T: data_serialization::de::DeserializeOwned>(value: json_codec::Value) -> T {
    json_codec::from_value(value).unwrap()
}

// Classification-only records deliberately bypass unrelated disk admission.
// The full oracle specifies behavior even for overlapping IDs and cold values.
fn fixture() -> State {
    let mut state = State::default();
    for (file, version, branch) in [("file-a", "v-a", "b-a"), ("file-b", "v-b", "b-b")] {
        state.versions.insert(
            version.into(),
            record(json!({
                "history_tree_id":"history", "version_id":version, "file_id":file,
                "creation_primary_slot_id":"P01", "logical_size":"0", "version_sequence":"1",
                "content_hash":"a".repeat(64), "checksum_chunk_bytes":65536,
                "checksum_manifest_hash":"hash", "creation_name":"file", "storage_name":"file",
                "created_at":"now", "source_version_ids":[], "created_as_full_checkpoint":true
            })),
        );
        state.placements.insert(version.into(), record(json!({
            "version_id":version, "primary_slot_id":"P01", "target_copies":1,
            "route_slot_ids":["P01"], "topology_generation":"1", "binding_generations":{"P01":"1"}
        })));
        state
            .content_states
            .insert(version.into(), ContentState::Available);
        state.branches.insert(branch.into(), record(json!({
            "branch_id":branch, "history_tree_id":"history", "name":branch,
            "head_commit_id":"commit", "fork_commit_id":"commit", "branch_kind":"file",
            "file_id":file, "generation":"1", "state":"active", "pending_file_versions":{file:version}
        })));
        state.file_names.insert(file.into(), "file".into());
        state
            .file_locations
            .insert(file.into(), "folder/file".into());
        state.file_notes.insert(file.into(), "note".into());
        state.file_selections.insert(file.into(), branch.into());
        state.file_backup_paths.insert(
            file.into(),
            std::collections::BTreeMap::from([("B01".into(), "backup/file".into())]),
        );
        state
            .file_backup_policies
            .insert(file.into(), "policy".into());
        state
            .version_notes
            .insert(version.into(), "version note".into());
        state.file_node_parents.insert(version.into(), vec![]);
    }
    state.file_contexts.insert(
        "context".into(),
        record(json!({"branch_id":"b-a", "inherited_from":null})),
    );
    state.file_path_nodes.insert("path".into(), record(json!({
        "node_id":"path", "history_tree_id":"history", "file_id":"file-a", "version_id":"v-a",
        "parent_node_ids":[], "branch_id":"b-a", "path":"new", "previous_path":"old",
        "operation_id":"op_00000000000000000000000000000000", "author":"author", "sequence":"1", "created_at":"1"
    })));
    state.file_metadata_nodes.insert("meta".into(), record(json!({
        "node_id":"meta", "history_tree_id":"history", "file_id":"file-b", "version_id":"v-b",
        "parent_node_ids":[], "branch_id":"b-b", "kind":"tags", "tags":[],
        "operation_id":"op_00000000000000000000000000000000", "author":"author", "sequence":"1", "created_at":"1"
    })));
    state.file_branch_merges.insert("merge".into(), record(json!({
        "merge_id":"merge", "history_tree_id":"history", "file_id":"file-a", "input_branch_ids":[],
        "input_node_ids":[], "chosen_node_id":"path", "result_node_id":"path", "surviving_branch_id":"b-a",
        "reused_node":true, "parent_node_ids":[], "operation_id":"op_00000000000000000000000000000000", "sequence":"1"
    })));
    state
        .file_path_notes
        .insert("path".into(), "path note".into());
    state
        .file_path_notes
        .insert("meta".into(), "meta note".into());
    state
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

fn compare(before: &State, after: &State) {
    assert_eq!(
        changed_files(before, after),
        full_changed_files(before, after)
    );
}

#[test]
fn every_input_insert_delete_and_same_value_matches_full_scan() {
    let before = fixture();
    macro_rules! exercise {
        ($($field:ident),* $(,)?) => {$(
            for key in before.$field.keys() {
                let mut after = candidate(&before);
                after.$field.remove(key);
                compare(&before, &after);
                let mut restored = candidate(&after);
                restored.$field.insert(key.clone(), before.$field[key].clone());
                compare(&after, &restored);
                let mut same = candidate(&before);
                same.$field.insert(key.clone(), before.$field[key].clone());
                compare(&before, &same);
                assert!(changed_files(&before, &same).is_empty(), stringify!($field));
                let mut reverted = candidate(&before);
                reverted.$field.insert("temporary".into(), before.$field[key].clone());
                reverted.$field.remove("temporary");
                compare(&before, &reverted);
                assert!(changed_files(&before, &reverted).is_empty(), stringify!($field));
            }
        )*};
    }
    exercise!(
        file_metadata_nodes,
        file_branch_merges,
        file_path_nodes,
        file_backup_paths,
        file_backup_policies,
        file_locations,
        file_names,
        file_notes,
        file_selections,
        file_path_notes,
        file_node_parents,
        file_contexts,
        version_notes,
        versions,
        placements,
        content_states,
        branches,
    );
}

fn assert_change(before: &State, expected: &[&str], change: impl FnOnce(&mut State)) {
    let mut after = candidate(before);
    change(&mut after);
    compare(before, &after);
    assert_eq!(
        changed_files(before, &after),
        expected.iter().map(|file| (*file).to_owned()).collect()
    );
}

#[test]
fn value_edits_preserve_file_context_and_pending_projection_semantics() {
    let before = fixture();
    macro_rules! direct {
        ($($field:ident),* $(,)?) => {$(
            assert_change(&before, &["file-a"], |after| {
                after.$field.insert("file-a".into(), "changed".into());
            });
        )*};
    }
    direct!(
        file_names,
        file_locations,
        file_notes,
        file_selections,
        file_backup_policies
    );
    assert_change(&before, &["file-a"], |after| {
        after
            .file_backup_paths
            .get_mut("file-a")
            .unwrap()
            .insert("B01".into(), "new/file".into());
    });
    assert_change(&before, &["file-a", "file-b"], |after| {
        after.file_contexts.get_mut("context").unwrap().branch_id = "b-b".into();
    });
    for (id, file) in [("path", "file-a"), ("meta", "file-b")] {
        assert_change(&before, &[file], |after| {
            after.file_path_notes.insert(id.into(), "changed".into());
        });
    }
    assert_change(&before, &["file-a"], |after| {
        after.version_notes.insert("v-a".into(), "changed".into());
    });
    assert_change(&before, &["file-a"], |after| {
        after
            .file_node_parents
            .insert("v-a".into(), vec!["meta".into()]);
    });
    assert_change(&before, &["file-a"], |after| {
        after.branches.get_mut("b-a").unwrap().name = "renamed".into();
    });
    assert_change(&before, &["file-a"], |after| {
        after.branches.get_mut("b-a").unwrap().file_node_id = Some("path".into());
    });
    assert_change(&before, &["file-a"], |after| {
        after.branches.get_mut("b-a").unwrap().state = BranchState::Merged;
    });
    assert_change(&before, &["file-a"], |after| {
        after.branches.get_mut("b-a").unwrap().history_tree_id = "other".into();
    });
    assert_change(&before, &["file-a", "file-b"], |after| {
        after
            .branches
            .get_mut("b-a")
            .unwrap()
            .pending_file_versions
            .insert("file-b".into(), "v-b".into());
    });
    assert_change(&before, &[], |after| {
        after.branches.get_mut("b-a").unwrap().generation += 1;
    });
}

#[test]
fn version_candidates_include_placement_and_content_writes_without_version_writes() {
    let before = fixture();
    assert_change(&before, &["file-a"], |after| {
        after
            .placements
            .get_mut("v-a")
            .unwrap()
            .route_slot_ids
            .push("B01".into());
    });
    assert_change(&before, &["file-a"], |after| {
        after
            .placements
            .get_mut("v-a")
            .unwrap()
            .binding_generations
            .insert("P01".into(), 2);
    });
    assert_change(&before, &["file-a"], |after| {
        after.placements.get_mut("v-a").unwrap().primary_slot_id = "P02".into();
    });
    assert_change(&before, &["file-a"], |after| {
        after.placements.get_mut("v-a").unwrap().target_copies = 2;
    });
    assert_change(&before, &["file-a"], |after| {
        after
            .content_states
            .insert("v-a".into(), ContentState::Purged);
    });
    assert_change(&before, &[], |after| {
        after.placements.get_mut("v-a").unwrap().topology_generation += 1;
    });
    assert_change(&before, &["file-a", "file-b"], |after| {
        after.versions.get_mut("v-a").unwrap().file_id = "file-b".into();
        after
            .content_states
            .insert("v-a".into(), ContentState::Purged);
    });
    assert_change(&before, &["file-a"], |after| {
        after.versions.remove("v-a");
        after.placements.remove("v-a");
        after.content_states.remove("v-a");
        after.version_notes.remove("v-a");
    });
}

#[test]
fn overlapping_notes_and_joint_removals_keep_original_owner_rules() {
    let mut before = fixture();
    let mut path = before.file_path_nodes["path"].clone();
    path.node_id = "meta".into();
    before.file_path_nodes.insert("meta".into(), path);
    assert_change(&before, &["file-a"], |after| {
        after
            .file_path_notes
            .insert("meta".into(), "changed".into());
    });
    assert_change(&before, &["file-a", "file-b"], |after| {
        after.file_path_nodes.remove("meta");
        after.file_metadata_nodes.remove("meta");
        after.file_path_notes.remove("meta");
    });
    assert_change(&before, &["file-a"], |after| {
        after
            .branches
            .get_mut("b-a")
            .unwrap()
            .pending_file_versions
            .remove("file-a");
    });
    // These existing record fields were not name-projection triggers before
    // this change; exact write candidates must not widen their semantics.
    assert_change(&before, &[], |after| {
        after.file_path_nodes.get_mut("path").unwrap().author = "different".into();
        after
            .file_metadata_nodes
            .get_mut("meta")
            .unwrap()
            .tags
            .insert("tag".into());
        after.file_branch_merges.get_mut("merge").unwrap().sequence += 1;
        after.versions.get_mut("v-a").unwrap().version_sequence += 1;
    });
}

#[test]
fn cold_foreign_bulk_and_overflow_inputs_keep_authoritative_full_comparison() {
    let before = fixture();
    let mut cold: State = json_codec::from_value(json_codec::to_value(&before).unwrap()).unwrap();
    cold.file_notes.insert("file-a".into(), "cold".into());
    compare(&before, &cold);
    assert_eq!(
        changed_files(&before, &cold),
        BTreeSet::from(["file-a".into()])
    );
    let mut foreign = candidate(&before);
    foreign.file_notes = before
        .file_notes
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    foreign.file_notes.insert("file-b".into(), "foreign".into());
    assert!(matches!(
        foreign.file_notes.changes_since(&before.file_notes),
        MapChanges::All
    ));
    compare(&before, &foreign);
    let mut bulk = candidate(&before);
    for placement in bulk.placements.values_mut() {
        placement.target_copies += 1;
    }
    assert!(matches!(
        bulk.placements.changes_since(&before.placements),
        MapChanges::All
    ));
    compare(&before, &bulk);
    let mut overflow = candidate(&before);
    for index in 0..=16_384 {
        overflow
            .file_notes
            .insert(format!("file-{index}"), "new".into());
    }
    assert!(matches!(
        overflow.file_notes.changes_since(&before.file_notes),
        MapChanges::All
    ));
    compare(&before, &overflow);
}

#[test]
fn exact_key_visitor_never_consumes_full_map_iterator() {
    let keys = std::sync::Arc::new(BTreeSet::from(["only".to_owned()]));
    let mut visited = Vec::new();
    visit_changes(
        MapChanges::Keys(keys),
        std::iter::once_with(|| panic!("exact writes must not scan unrelated records")),
        |key| visited.push(key.clone()),
    );
    assert_eq!(visited, ["only"]);
    visit_changes(
        MapChanges::Keys(std::sync::Arc::new(BTreeSet::new())),
        std::iter::once_with(|| panic!("empty writes must not scan unrelated records")),
        |_| panic!("empty write set"),
    );
    let all = ["old".to_owned(), "new".to_owned()];
    visited.clear();
    visit_changes(MapChanges::All, all.iter(), |key| visited.push(key.clone()));
    assert_eq!(visited, all);
}
