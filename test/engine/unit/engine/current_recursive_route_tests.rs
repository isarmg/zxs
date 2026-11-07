//! Pure required-route collection, independent of visibility or body availability.
use super::*;
use crate::{ContentHash, ContentState, FileVersion, Placement};

fn version(state: &mut State, file: &str, id: &str, route: &[&str]) {
    state.versions.insert(
        id.into(),
        FileVersion {
            history_tree_id: "history".into(),
            version_id: id.into(),
            file_id: file.into(),
            creation_primary_slot_id: route[0].into(),
            logical_size: 4,
            version_sequence: 1,
            content_hash: ContentHash::from_digest(blake3::hash(b"body")),
            checksum_chunk_bytes: 1024,
            checksum_manifest_hash: "manifest".into(),
            creation_name: "file".into(),
            storage_name: "version".into(),
            created_at: "2026-11-07T00:00:00Z".into(),
            source_version_ids: Vec::new(),
            created_as_full_checkpoint: true,
        },
    );
    state.placements.insert(
        id.into(),
        Placement {
            version_id: id.into(),
            primary_slot_id: route[0].into(),
            target_copies: route.len() as u32,
            route_slot_ids: route.iter().map(|slot| (*slot).into()).collect(),
            topology_generation: 1,
            binding_generations: route.iter().map(|slot| ((*slot).into(), 1)).collect(),
        },
    );
}

#[test]
fn routed_follow_collects_every_retained_route_without_availability_or_head_filter() {
    let mut state = State::default();
    version(&mut state, "file", "old", &["D04", "D05", "D06"]);
    version(&mut state, "file", "new", &["P01", "B01", "C01"]);
    version(&mut state, "unrelated", "other", &["unused"]);
    state
        .content_states
        .insert("old".into(), ContentState::Purged);
    let routes = collect(&state, &BTreeSet::from(["file".into()])).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(
        routes["file"]
            .bindings
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["B01", "C01", "D04", "D05", "D06", "P01"]
    );
    assert_eq!(
        routes["file"].backups,
        BTreeSet::from(["B01".into(), "C01".into(), "D05".into(), "D06".into()])
    );
}

#[test]
fn routed_follow_rejects_missing_or_conflicting_retained_placement_bindings() {
    let mut state = State::default();
    version(&mut state, "file", "old", &["P01", "B01"]);
    version(&mut state, "file", "new", &["P01", "B01"]);
    let files = BTreeSet::from(["file".into()]);
    let mut missing = state.clone();
    missing.placements.remove("old");
    assert!(collect(&missing, &files).is_err());
    let mut missing_binding = state.clone();
    missing_binding
        .placements
        .get_mut("old")
        .unwrap()
        .binding_generations
        .remove("B01");
    assert!(collect(&missing_binding, &files).is_err());
    state
        .placements
        .get_mut("old")
        .unwrap()
        .binding_generations
        .insert("B01".into(), 2);
    assert!(collect(&state, &files).is_err());
}
