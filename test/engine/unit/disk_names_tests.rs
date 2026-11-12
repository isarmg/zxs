//! Pure geometry and real held-root observations; no rename or format migration.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

fn disk(index: u8, name: &str, level: u32, upstream: Option<u8>) -> DiskNameObservation {
    DiskNameObservation {
        identity: DiskSourceIdentity {
            pool_id: PoolId::from_random([1; 16]),
            disk_id: DiskId::from_random([index; 16]),
            slot_id: SlotId::parse(&format!("D{index:02}")).unwrap(),
            binding_generation: 1,
        },
        registration_mode: RegistrationMode::Enabled,
        level,
        upstream_slot_id: upstream.map(|index| SlotId::parse(&format!("D{index:02}")).unwrap()),
        observed_name: Some(Name::parse(name).unwrap()),
        component_maximum: Some(255),
    }
}

fn preview(disks: Vec<DiskNameObservation>, requested: u8, name: &str) -> DiskSourceNamePreview {
    DiskSourceNamePreview {
        source: DiskNameObservationSource::ConfiguredRootObservation,
        execution: DiskNamePreviewExecution::PreviewOnly,
        topology_generation: 7,
        registry: None,
        requested_disk_id: DiskId::from_random([requested; 16]),
        proposed_name: Name::parse(name).unwrap(),
        disks,
        slices: Vec::new(),
        rename_roots: Vec::new(),
        blockers: Vec::new(),
    }
}

fn chain() -> Vec<DiskNameObservation> {
    vec![
        disk(1, "A", 1, None),
        disk(2, "B", 2, Some(1)),
        disk(3, "C", 3, Some(2)),
        disk(4, "D", 4, Some(3)),
        disk(5, "Sibling", 3, Some(2)),
    ]
}

fn component(index: u8, name: &str) -> String {
    format!("{name}--{}", format!("{index:02x}").repeat(16))
}

#[test]
fn middle_name_projects_every_source_slice_and_collapses_nested_roots() {
    let mut preview = preview(chain(), 2, "Replacement");
    plan::build(&mut preview).unwrap();
    assert!(preview.blockers.is_empty());
    assert_eq!(preview.slices.len(), 8);
    assert_eq!(preview.rename_roots.len(), 3);
    let c = DiskId::from_random([3; 16]);
    let c_slices: Vec<_> = preview
        .slices
        .iter()
        .filter(|slice| slice.target == c)
        .collect();
    assert_eq!(c_slices.len(), 2);
    assert_eq!(c_slices[0].before.as_ref().unwrap().as_str(), component(2, "B"));
    assert_eq!(
        c_slices[1].after.as_ref().unwrap().as_str(),
        format!("{}/{}", component(2, "Replacement"), component(1, "A")),
    );
    assert!(c_slices.iter().all(|slice| slice.affected));
    assert!(preview
        .slices
        .iter()
        .filter(|slice| slice.target == DiskId::from_random([2; 16]))
        .all(|slice| !slice.affected));
    let d_root = preview
        .rename_roots
        .iter()
        .find(|root| root.target == DiskId::from_random([4; 16]))
        .unwrap();
    assert_eq!(
        d_root.after.as_ref().unwrap().as_str(),
        format!("{}/{}", component(3, "C"), component(2, "Replacement")),
    );
}

#[test]
fn unchanged_spelling_is_empty_but_own_case_change_has_impacts() {
    let mut same = preview(chain(), 2, "B");
    plan::build(&mut same).unwrap();
    assert!(same.rename_roots.is_empty());
    assert!(same.slices.iter().all(|slice| !slice.affected));
    let mut changed = preview(chain(), 2, "b");
    plan::build(&mut changed).unwrap();
    assert!(changed.blockers.is_empty());
    assert_eq!(changed.rename_roots.len(), 3);
}

#[test]
fn disabled_registration_keeps_name_reservations_and_descendant_slices() {
    let mut disks = chain();
    disks[4].registration_mode = RegistrationMode::Disabled;
    let mut preview = preview(disks, 2, "sIBLING");
    plan::build(&mut preview).unwrap();
    assert_eq!(preview.slices.len(), 8);
    assert_eq!(preview.rename_roots.len(), 3);
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        DiskNamePreviewBlocker::NameConflict {
            phase: DiskNamePreviewPhase::Proposed,
            ..
        }
    )));
}

#[test]
fn unicode_casefold_conflicts_are_reported_without_truncating_names() {
    let disks = vec![disk(1, "Straße", 1, None), disk(2, "Other", 2, Some(1))];
    let mut preview = preview(disks, 2, "STRASSE");
    plan::build(&mut preview).unwrap();
    assert!(preview
        .blockers
        .iter()
        .any(|blocker| matches!(blocker, DiskNamePreviewBlocker::NameConflict { .. })));
    assert_eq!(preview.proposed_name.as_str(), "STRASSE");
}

#[test]
fn multibyte_names_charge_uuid_and_reserve_against_each_actual_target() {
    let name = "界".repeat(69);
    let mut preview = preview(chain(), 2, &name);
    preview.disks[4].component_maximum = Some(512);
    plan::build(&mut preview).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        DiskNamePreviewBlocker::ComponentBudget {
            bytes: 241,
            maximum: 255,
            ..
        }
    )));
    let sibling = DiskId::from_random([5; 16]);
    let accepted = preview
        .slices
        .iter()
        .find(|slice| slice.target == sibling)
        .unwrap();
    assert_eq!(accepted.after.as_ref().unwrap().as_str(), component(2, &name));
    assert!(accepted.after.as_ref().unwrap().as_str().len() > 240);
}

#[test]
fn full_source_slice_paths_have_a_separate_byte_and_depth_budget() {
    let disks = (1..=18)
        .map(|index| {
            disk(
                index,
                &format!("{index:02}{}", "a".repeat(203)),
                index as u32,
                (index > 1).then_some(index - 1),
            )
        })
        .collect();
    let mut preview = preview(disks, 1, "Changed");
    plan::build(&mut preview).unwrap();
    assert!(preview
        .blockers
        .iter()
        .any(|blocker| matches!(blocker, DiskNamePreviewBlocker::ProjectedPathBudget { .. })));
}

#[test]
fn missing_upstream_preserves_the_target_and_marks_incomplete_topology() {
    let mut preview = preview(vec![disk(2, "B", 2, Some(1))], 2, "Changed");
    plan::build(&mut preview).unwrap();
    assert_eq!(preview.disks.len(), 1);
    assert!(matches!(
        &preview.blockers[0],
        DiskNamePreviewBlocker::MissingUpstream { .. }
    ));
}

#[test]
fn complete_preview_rejects_registration_or_ancestry_budget_exhaustion() {
    let mut disks = chain();
    while disks.len() <= MAX_DISKS {
        disks.push(disk(1, "A", 1, None));
    }
    let mut oversized = preview(disks, 1, "Changed");
    assert_eq!(
        plan::build(&mut oversized).unwrap_err().code(),
        ErrorCode::ResourceLimit,
    );
    let disks = (1..=40)
        .map(|index| {
            disk(
                index,
                &format!("N{index}"),
                index as u32,
                (index > 1).then_some(index - 1),
            )
        })
        .collect();
    let mut ancestry = preview(disks, 1, "Changed");
    assert_eq!(
        plan::build(&mut ancestry).unwrap_err().code(),
        ErrorCode::ResourceLimit,
    );
}

fn engine_fixture() -> (tempfile::TempDir, Engine) {
    let temporary = tempfile::tempdir().unwrap();
    let config =
        crate::Config::development(["A", "B", "C"].map(|name| temporary.path().join(name)))
            .unwrap();
    let engine = Engine::initialize(&temporary.path().join("system"), config).unwrap();
    (temporary, engine)
}

fn files(path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files(&entry.path(), result);
        } else {
            result.insert(entry.path(), fs::read(entry.path()).unwrap());
        }
    }
}

#[test]
fn live_preview_reads_held_capabilities_without_changing_state_or_files() {
    let (temporary, engine) = engine_fixture();
    let state = crate::canonical_json(engine.state()).unwrap();
    let config = crate::canonical_json(engine.config()).unwrap();
    let mut before = BTreeMap::new();
    files(temporary.path(), &mut before);
    let disk_id = engine.config().disks[1].disk_id.parse().unwrap();
    let preview = engine
        .preview_disk_source_names(disk_id, &Name::parse("Renamed").unwrap())
        .unwrap();
    assert!(preview.blockers.is_empty());
    assert_eq!(preview.slices.len(), 3);
    assert_eq!(preview.rename_roots.len(), 1);
    assert!(preview
        .disks
        .iter()
        .all(|disk| disk.component_maximum.is_some()));
    assert_eq!(crate::canonical_json(engine.state()).unwrap(), state);
    assert_eq!(crate::canonical_json(engine.config()).unwrap(), config);
    let mut after = BTreeMap::new();
    files(temporary.path(), &mut after);
    assert_eq!(before, after);
}

#[test]
fn unavailable_held_root_is_blocked_without_reopening_or_dropping_slices() {
    let (_temporary, mut engine) = engine_fixture();
    let target = engine.config().disks[2].disk_id.clone();
    engine.disk_resources = Arc::new(engine.disk_resources.excluding(&BTreeSet::from([target])));
    let preview = engine
        .preview_disk_source_names(
            engine.config().disks[1].disk_id.parse().unwrap(),
            &Name::parse("Renamed").unwrap(),
        )
        .unwrap();
    assert_eq!(preview.disks.len(), 3);
    assert_eq!(preview.slices.len(), 3);
    assert_eq!(preview.rename_roots.len(), 1);
    assert!(preview.rename_roots[0].after.is_none());
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        DiskNamePreviewBlocker::BudgetUnavailable {
            code: ErrorCode::DiskUnavailable,
            ..
        }
    )));
}

#[test]
fn mismatched_registration_never_borrows_configured_name_or_capacity() {
    let (_temporary, engine) = engine_fixture();
    let mut registered = engine.registered_disks().next().unwrap();
    registered.binding_generation += 1;
    let mut blockers = Vec::new();
    let observation = observe(&engine, registered, &mut blockers).unwrap();
    assert!(observation.observed_name.is_none());
    assert!(observation.component_maximum.is_none());
    assert!(matches!(
        &blockers[0],
        DiskNamePreviewBlocker::ConfigIdentityMismatch { .. }
    ));
}

#[test]
fn an_unregistered_typed_disk_is_a_named_error_not_a_partial_preview() {
    let (_temporary, engine) = engine_fixture();
    let error = engine
        .preview_disk_source_names(
            DiskId::from_random([0; 16]),
            &Name::parse("Renamed").unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::DiskNotFound);
}

#[test]
fn disabled_identity_retains_configured_name_without_borrowing_vote_membership() {
    let (_temporary, engine) = engine_fixture();
    let mut registered = engine.registered_disks().next().unwrap();
    registered.registration_mode = RegistrationMode::Disabled;
    registered.metadata_member = false;
    let mut blockers = Vec::new();
    let observation = observe(&engine, registered, &mut blockers).unwrap();
    assert_eq!(observation.observed_name.as_ref().unwrap().as_str(), "A");
    assert!(observation.component_maximum.is_some());
    assert!(blockers.is_empty());
}

#[test]
fn serialized_preview_explicitly_disclaims_execution_and_omits_host_paths() {
    let (temporary, engine) = engine_fixture();
    let preview = engine
        .preview_disk_source_names(
            engine.config().disks[1].disk_id.parse().unwrap(),
            &Name::parse("Renamed").unwrap(),
        )
        .unwrap();
    let json = String::from_utf8(crate::canonical_json(&preview).unwrap()).unwrap();
    assert!(json.contains("\"source\":\"configured_root_observation\""));
    assert!(json.contains("\"execution\":\"preview_only\""));
    assert!(!json.contains(temporary.path().to_str().unwrap()));
}
