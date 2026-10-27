use super::super::{HistoryManifest, graph, history_changes_oracle_tests::dependency_fixture};
use super::*;
use crate::{canonical_json, disk::MAX_METADATA, journal::AuthenticatedMetadataObject};
use json_codec::json;

fn cloned<T: Clone>(value: &T) -> T {
    value.clone()
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

fn logical(state: &State, id: &str) -> Value {
    let manifest = HistoryManifest::project_state(state, id).unwrap();
    let mut value = json_codec::to_value(&manifest).unwrap();
    for field in Field::ALL {
        if value.get(field.name()).is_none() {
            value[field.name()] = json!({});
        }
    }
    value
}

// Independent full projection/encoding is intentionally only test code. The
// fixture includes multiple owners disallowed by today's external admission.
fn compare(before: &State, after: &State, index: &DerivedOwners) -> DerivedOwners {
    let mut delta = ProjectionDelta::between(before, after, index)
        .unwrap()
        .unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(after));
    let mut expected_histories = BTreeSet::new();
    for id in before
        .histories
        .keys()
        .chain(after.histories.keys())
        .collect::<BTreeSet<_>>()
    {
        match (
            before.histories.contains_key(id),
            after.histories.contains_key(id),
        ) {
            (true, true) if logical(before, id) != logical(after, id) => {
                expected_histories.insert(id.clone());
            }
            (true, false) | (false, true) => {
                expected_histories.insert(id.clone());
            }
            _ => {}
        }
    }
    assert_eq!(
        delta.histories.keys().cloned().collect::<BTreeSet<_>>(),
        expected_histories
    );
    for id in after.histories.keys() {
        let expected_manifest = HistoryManifest::project_state(after, id).unwrap();
        let (expected_hash, expected_objects) =
            graph::prepare(&expected_manifest, MAX_METADATA).unwrap();
        let (actual_hash, actual_objects) = if before.histories.contains_key(id) {
            let before_manifest = HistoryManifest::project_state(before, id).unwrap();
            let (hash, objects) = graph::prepare(&before_manifest, MAX_METADATA).unwrap();
            if let Some(edits) = delta.histories.remove(id) {
                let mut projected = logical(before, id);
                if edits.history {
                    projected["history"] = json_codec::to_value(&after.histories[id]).unwrap();
                }
                if edits.repository {
                    projected["repository"] = json_codec::to_value(
                        &after.repositories[&after.histories[id].repository_id],
                    )
                    .unwrap();
                }
                for (field, rows) in &edits.maps {
                    let map = projected[field.name()].as_object_mut().unwrap();
                    for (key, value) in rows {
                        match value {
                            Some(value) => {
                                map.insert(key.clone(), value.clone());
                            }
                            None => {
                                map.remove(key);
                            }
                        }
                    }
                }
                assert_eq!(projected, logical(after, id), "logical delta for {id}");
                let objects = objects
                    .into_iter()
                    .map(|object| {
                        let value = crate::decode_strict(object.bytes()).unwrap();
                        AuthenticatedMetadataObject::authenticate(
                            object.kind(),
                            &object.hash,
                            value,
                        )
                        .unwrap()
                    })
                    .collect();
                graph::prepare_update(objects, edits, after, MAX_METADATA).unwrap()
            } else {
                (hash, objects)
            }
        } else {
            graph::prepare_indexed(after, id, &delta.owners, MAX_METADATA).unwrap()
        };
        assert_eq!(actual_hash, expected_hash, "history root for {id}");
        let bytes = |objects: Vec<crate::journal::PreparedMetadataObject>| {
            objects
                .into_iter()
                .map(|object| {
                    (
                        (object.kind(), object.hash.clone()),
                        object.bytes().to_vec(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(
            bytes(actual_objects),
            bytes(expected_objects),
            "every exact graph object for {id}"
        );
    }
    delta.owners
}

#[test]
fn every_projection_map_insert_delete_and_same_value_write_matches_full_oracle() {
    let before = dependency_fixture();
    let index = DerivedOwners::from_state(&before);
    macro_rules! exercise {
        ($($field:ident => $variant:ident, $owner:ident, $name:literal;)*) => { $(
            for key in before.$field.keys() {
                let mut after = candidate(&before);
                after.$field.remove(key);
                let removed_index = compare(&before, &after, &index);
                let mut restored = candidate(&after);
                restored.$field.insert(key.clone(), cloned(&before.$field[key]));
                compare(&after, &restored, &removed_index);
                let mut same = candidate(&before);
                same.$field.insert(key.clone(), cloned(&before.$field[key]));
                assert!(ProjectionDelta::between(&before, &same, &index).unwrap().unwrap().histories.is_empty(), $name);
            }
        )* };
    }
    projection_maps!(exercise);
}

#[test]
fn every_owner_move_propagates_old_and_new_dependent_rows() {
    let before = dependency_fixture();
    let index = DerivedOwners::from_state(&before);
    macro_rules! moved {
        ($field:ident, $key:literal, $member:ident, $value:literal) => {{
            let mut after = candidate(&before);
            after.$field.get_mut($key).unwrap().$member = $value.into();
            compare(&before, &after, &index);
        }};
    }
    moved!(branches, "b1", history_tree_id, "h3");
    moved!(versions, "v1", history_tree_id, "h3");
    moved!(versions, "v1", file_id, "other-file");
    moved!(commits, "c1", history_tree_id, "h3");
    moved!(histories, "h1", repository_id, "r2");
    moved!(file_path_nodes, "node", history_tree_id, "h3");
    moved!(file_metadata_nodes, "node", history_tree_id, "h3");
    moved!(file_branch_merges, "merge", history_tree_id, "h3");
    moved!(merge_links, "link", history_tree_id, "h3");
    moved!(history_boundaries, "boundary", history_tree_id, "h3");
    moved!(directory_branches, "d1", repository_id, "r2");
    moved!(directory_trees, "t1", directory_branch_id, "d3");
    moved!(placement_rules, "rule", repository_id, "r2");
    moved!(protection_policies, "policy", repository_id, "r2");
    for change in 0..7 {
        let mut after = candidate(&before);
        match change {
            0 => {
                after
                    .file_selections
                    .insert("shared-file".into(), "b3".into());
            }
            1 => {
                after.file_branch_aliases.insert("b1".into(), "b2".into());
            }
            2 => {
                after
                    .directory_selections
                    .insert("shared-directory".into(), "d3".into());
            }
            3 => {
                let value = after.file_contexts.remove("r1/path").unwrap();
                after.file_contexts.insert("r2/path".into(), value);
            }
            4 => {
                after.commits.get_mut("c1").unwrap().entries[0].entry_id = "other-directory".into();
            }
            5 => {
                after
                    .directory_selection_generations
                    .insert("shared-directory".into(), u64::MAX);
                after
                    .file_selection_generations
                    .insert("shared-file".into(), u64::MAX);
            }
            6 => {
                after.repositories.get_mut("r1").unwrap().name = "changed repository".into();
            }
            _ => unreachable!(),
        }
        compare(&before, &after, &index);
    }
}

#[test]
fn deleting_one_of_multiple_references_retains_ownership_until_the_last_reference() {
    let mut before = dependency_fixture();
    let mut extra = before.versions["v1"].clone();
    extra.version_id = "v1-extra".into();
    before.versions.insert(extra.version_id.clone(), extra);
    let mut extra = before.commits["c1"].clone();
    extra.commit_id = "c1-extra".into();
    before.commits.insert(extra.commit_id.clone(), extra);
    let mut index = DerivedOwners::from_state(&before);
    for (version, commit) in [("v1", "c1"), ("v1-extra", "c1-extra"), ("v2", "c2")] {
        let mut after = candidate(&before);
        after.versions.remove_discard(version);
        after.commits.remove_discard(commit);
        index = compare(&before, &after, &index);
        before = after;
    }
    assert!(index.files.owners("shared-file").is_empty());
    assert!(index.directories.owners("shared-directory").is_empty());
}

#[test]
fn adding_and_removing_history_and_joint_owner_edits_match_independent_projection() {
    let mut before = dependency_fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    let mut history = before.histories["h1"].clone();
    history.history_tree_id = "h4".into();
    after.histories.insert("h4".into(), history);
    let index = compare(&before, &after, &index);
    before = after;
    let mut after = candidate(&before);
    after.histories.remove("h1");
    after.branches.get_mut("b1").unwrap().history_tree_id = "h4".into();
    after.versions.get_mut("v1").unwrap().history_tree_id = "h4".into();
    after
        .directory_branches
        .get_mut("d1")
        .unwrap()
        .repository_id = "r2".into();
    after.histories.get_mut("h4").unwrap().repository_id = "r2".into();
    let index = compare(&before, &after, &index);
    before = after;
    let mut after = candidate(&before);
    after.histories.remove("h2");
    after.repositories.remove("r1");
    compare(&before, &after, &index);
}

#[test]
fn repeated_precise_generations_preserve_reverse_indices_and_untouched_history_storage() {
    let mut before = dependency_fixture();
    let mut index = DerivedOwners::from_state(&before);
    for step in 0..20 {
        let mut after = candidate(&before);
        after.file_selections.insert(
            "shared-file".into(),
            if step % 2 == 0 { "b3" } else { "b1" }.into(),
        );
        after
            .file_notes
            .insert("shared-file".into(), format!("note-{step}"));
        after.versions.get_mut("v2").unwrap().file_id = if step % 3 == 0 {
            "other-file"
        } else {
            "shared-file"
        }
        .into();
        index = compare(&before, &after, &index);
        before = after;
    }
    let mut after = candidate(&before);
    after.branches.get_mut("b1").unwrap().name = "precise label".into();
    let delta = ProjectionDelta::between(&before, &after, &index)
        .unwrap()
        .unwrap();
    assert_eq!(delta.histories.len(), 1);
    assert_eq!(delta.histories["h1"].maps.len(), 1);
    assert!(
        delta
            .owners
            .history_rows
            .shares_storage_with(&index.history_rows)
    );
    assert!(delta.owners.files.0.shares_storage_with(&index.files.0));
    assert!(
        delta
            .owners
            .directories
            .0
            .shares_storage_with(&index.directories.0)
    );
}

#[test]
fn exact_empty_and_control_writes_are_ignored_but_all_foreign_and_overflow_are_explicit() {
    let before = dependency_fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    after.operations = Default::default();
    after.operation_principals = Default::default();
    after.plans = Default::default();
    after.control_auth = Some(json!({"independent": true}));
    after.events.clear();
    assert!(
        ProjectionDelta::between(&before, &after, &index)
            .unwrap()
            .unwrap()
            .histories
            .is_empty()
    );
    macro_rules! unknown {
        ($($field:ident => $variant:ident, $owner:ident, $name:literal;)*) => { $(
            let mut after = candidate(&before);
            after.$field = Default::default();
            assert!(ProjectionDelta::between(&before, &after, &index).unwrap().is_none(), $name);
        )* };
    }
    projection_maps!(unknown);
    let mut after = candidate(&before);
    after.branches.values_mut().next().unwrap().name = "unknown mutable iterator".into();
    assert!(
        ProjectionDelta::between(&before, &after, &index)
            .unwrap()
            .is_none()
    );
    let decoded = json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap();
    assert!(
        ProjectionDelta::between(&before, &decoded, &index)
            .unwrap()
            .is_none()
    );
    let mut after = candidate(&before);
    for index in 0..16_385 {
        after
            .file_notes
            .insert(format!("unowned-{index}"), "note".into());
    }
    assert!(
        ProjectionDelta::between(&before, &after, &index)
            .unwrap()
            .is_none()
    );
}

#[test]
fn stable_history_and_directory_head_writes_do_not_rebuild_dependency_membership() {
    let before = dependency_fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    after.histories.get_mut("h1").unwrap().generation += 1;
    after.directory_branches.get_mut("d1").unwrap().sequence += 1;
    after
        .protection_policies
        .get_mut("policy")
        .unwrap()
        .generation += 1;
    let delta = ProjectionDelta::between(&before, &after, &index)
        .unwrap()
        .unwrap();
    assert!(
        delta
            .owners
            .repository_rows
            .shares_storage_with(&index.repository_rows)
    );
    assert!(
        delta
            .owners
            .history_rows
            .shares_storage_with(&index.history_rows)
    );
    assert!(
        delta
            .owners
            .directory_trees
            .0
            .shares_storage_with(&index.directory_trees.0)
    );
    assert_eq!(delta.histories["h1"].maps.len(), 2);
    assert_eq!(delta.histories["h2"].maps.len(), 2);
    compare(&before, &after, &index);
}
