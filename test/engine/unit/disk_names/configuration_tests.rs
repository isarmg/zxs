//! Pure configured-name geometry. These fixtures are not admitted storage roots.
use super::*;
use crate::Mode;

fn id(index: u8) -> DiskId {
    DiskId::from_random([index; 16])
}

fn disk(index: u8, name: &str, level: u32, upstream: Option<u8>) -> DiskConfig {
    DiskConfig {
        slot_id: format!("D{index:02}"),
        disk_id: id(index).to_string(),
        name: Name::parse(name).unwrap(),
        // Deliberately unrelated to the explicit user name.
        root: format!("/unopened-mount-{index}").into(),
        level,
        upstream_slot_id: upstream.map(|parent| format!("D{parent:02}")),
        metadata_member: true,
        binding_generation: u64::from(index) + 10,
        capacity_bytes: None,
    }
}

fn chain() -> Config {
    Config {
        pool_id: PoolId::from_random([9; 16]).to_string(),
        storage_roots: BTreeMap::new(),
        mode: Mode::Development,
        disks: vec![disk(1, "Alpha", 1, None), disk(2, "Beta", 2, Some(1)),
            disk(3, "Gamma", 3, Some(2))],
        checksum_chunk_bytes: 262144,
        max_file_bytes: 1 << 40,
        max_workspace_bytes: 1 << 40,
        max_workspaces: 128,
        max_retained_workspaces: 4096,
        max_retained_workspaces_per_principal: 1024,
        max_retained_workspace_metadata_bytes: 128 << 20,
        max_retained_workspace_metadata_bytes_per_principal: 64 << 20,
        max_entries: 100_000,
        metadata_reserve_bytes: 64 << 20,
        workspace_ttl_seconds: 86_400,
    }
}

fn component(index: u8, name: &str) -> String {
    format!("{name}--{}", format!("{index:02x}").repeat(16))
}

#[test]
fn actual_config_names_generate_direct_source_uuid_paths_not_mount_names() {
    let config = chain();
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let primary = names.source_path(id(1)).unwrap();
    assert_eq!(primary.relative_prefix(), None);
    assert_eq!(primary.direct_source(), None);
    assert_eq!(primary.source_ids().len(), 0);
    primary.verify_component_budget(0).unwrap();
    let b = names.source_path(id(2)).unwrap();
    assert_eq!(b.target(), id(2));
    assert_eq!(b.direct_source(), Some(id(1)));
    assert_eq!(b.relative_prefix(), Some(component(1, "Alpha").as_str()));
    let c = names.source_path(id(3)).unwrap();
    assert_eq!(c.direct_source(), Some(id(2)));
    assert_eq!(c.source_ids().collect::<Vec<_>>(), vec![id(2), id(1)]);
    assert_eq!(c.relative_prefix(), Some(format!("{}/{}",
        component(2, "Beta"), component(1, "Alpha")).as_str()));
    assert_eq!(c.target_registration().disk_id, config.disks[2].disk_id);
    let sources = c.source_registrations().collect::<Vec<_>>();
    assert_eq!(sources[0].pool_id, config.pool_id);
    assert_eq!(sources[0].disk_id, config.disks[1].disk_id);
    assert_eq!(sources[0].slot_id, "D02");
    assert_eq!(sources[0].binding_generation, 12);
    assert_eq!(sources[1].disk_id, config.disks[0].disk_id);
    assert_eq!(sources[1].binding_generation, 11);

    let mut relocated = config.clone();
    for disk in &mut relocated.disks {
        disk.root = format!("/some-other-path/{}", disk.disk_id).into();
    }
    let relocated = ConfiguredDiskNames::new(&relocated).unwrap();
    let actual = relocated.source_path(id(3)).unwrap();
    assert_eq!(actual.relative_prefix(), c.relative_prefix());
    assert_eq!(actual.source_registrations().collect::<Vec<_>>(), sources);
}

#[test]
fn explicit_name_changes_preserve_ids_and_direct_source_edges() {
    let mut config = chain();
    let original = ConfiguredDiskNames::new(&config).unwrap().source_path(id(3)).unwrap();
    let original_path = original.relative_prefix().unwrap().to_owned();
    let sources = original.source_registrations().collect::<Vec<_>>();
    config.disks[0].name = Name::parse("Alpha new").unwrap();
    let named = ConfiguredDiskNames::new(&config).unwrap();
    assert_eq!(named.source_path(id(2)).unwrap().relative_prefix(),
        Some(component(1, "Alpha new").as_str()));
    let changed = named.source_path(id(3)).unwrap();
    assert_ne!(changed.relative_prefix(), Some(original_path.as_str()));
    assert_eq!(changed.relative_prefix(), Some(format!("{}/{}",
        component(2, "Beta"), component(1, "Alpha new")).as_str()));
    assert_eq!(changed.source_registrations().collect::<Vec<_>>(), sources);
    config.disks[1].name = Name::parse("Beta new").unwrap();
    let changed = ConfiguredDiskNames::new(&config).unwrap().source_path(id(3)).unwrap();
    assert_eq!(changed.relative_prefix(), Some(format!("{}/{}",
        component(2, "Beta new"), component(1, "Alpha new")).as_str()));
    assert_eq!(changed.direct_source(), Some(id(2)));
    assert_eq!(changed.source_registrations().collect::<Vec<_>>(), sources);
}

#[test]
fn names_are_unique_by_unicode_lookup_key_even_with_distinct_uuids() {
    let mut config = chain();
    config.disks[0].name = Name::parse("Straße").unwrap();
    config.disks[1].name = Name::parse("STRASSE").unwrap();
    let Err(error) = ConfiguredDiskNames::new(&config) else {
        panic!("distinct UUIDs must not relax configured name uniqueness");
    };
    assert!(matches!(error, ConfiguredDiskNameError::DuplicateName {
        first, second, ..
    } if first == id(1) && second == id(2)));
    assert_eq!(error.code(), ErrorCode::NameConflict);

    let config = chain();
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let first = id(4).to_string();
    let second = id(5).to_string();
    let available = Name::parse("Delta").unwrap();
    names.require_new_names([(first.as_str(), &available)]).unwrap();
    let duplicate = Name::parse("ALPHA").unwrap();
    assert!(matches!(names.require_new_names([(first.as_str(), &duplicate)]),
        Err(ConfiguredDiskNameError::DuplicateName { first, second, .. })
            if first == id(1) && second == id(4)));
    let duplicate = Name::parse("DELTA").unwrap();
    assert!(matches!(names.require_new_names([
        (first.as_str(), &available), (second.as_str(), &duplicate),
    ]), Err(ConfiguredDiskNameError::DuplicateName { first, second, .. })
        if first == id(4) && second == id(5)));
}

#[test]
fn admitted_configuration_keeps_the_required_name_and_rejects_duplicate_lookup_names() {
    let config = chain();
    let admitted = crate::ValidatedConfig::try_from(config.clone()).unwrap();
    assert_eq!(admitted.disks[0].name, Name::parse("Alpha").unwrap());

    let mut missing = json_codec::to_value(&config).unwrap();
    missing["disks"][0].as_object_mut().unwrap().remove("name");
    assert!(json_codec::from_value::<Config>(missing).is_err());

    let mut duplicate = config;
    duplicate.disks[1].name = Name::parse("ALPHA").unwrap();
    assert!(matches!(crate::ValidatedConfig::try_from(duplicate),
        Err(crate::Error::ConfiguredDiskName(ConfiguredDiskNameError::DuplicateName { .. }))));
}

#[test]
fn utf8_bytes_include_the_complete_uuid_and_actual_target_reserve() {
    let mut config = chain();
    config.disks[0].name = Name::parse(&"界".repeat(68)).unwrap();
    let source = ConfiguredDiskNames::new(&config).unwrap().source_path(id(2)).unwrap();
    assert_eq!(source.relative_prefix().unwrap().len(), 238);
    source.verify_component_budget(255).unwrap();
    config.disks[0].name = Name::parse(&"界".repeat(69)).unwrap();
    let source = ConfiguredDiskNames::new(&config).unwrap().source_path(id(2)).unwrap();
    assert_eq!(source.relative_prefix().unwrap().len(), 241);
    assert!(matches!(source.verify_component_budget(255),
        Err(ConfiguredDiskNameError::ComponentBudget { target, disk_id, .. })
            if target == id(2) && disk_id == id(1)));
    // Generated components do not acquire a second user-Name 240-byte ceiling.
    source.verify_component_budget(512).unwrap();
    assert!(matches!(source.verify_component_budget(16),
        Err(ConfiguredDiskNameError::TargetBudget { target, .. }) if target == id(2)));
}

#[test]
fn sibling_routes_never_become_successive_ancestors() {
    let mut config = chain();
    config.disks.insert(2, disk(4, "Sibling", 3, Some(2)));
    // Pure geometry does not authorize public Config fanout or select a route.
    // The existing native Backup decision remains responsible for that scope.
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let left = names.source_path(id(3)).unwrap();
    let right = names.source_path(id(4)).unwrap();
    assert_eq!(left.relative_prefix(), right.relative_prefix());
    assert_eq!(left.source_ids().collect::<Vec<_>>(), vec![id(2), id(1)]);
    assert_eq!(right.source_ids().collect::<Vec<_>>(), vec![id(2), id(1)]);
    assert_ne!(left.target(), right.target());
}

#[test]
fn broken_direct_chain_or_unknown_uuid_never_produces_a_path() {
    let mut config = chain();
    config.disks[2].upstream_slot_id = Some("D01".into());
    let names = ConfiguredDiskNames::new(&config).unwrap();
    assert!(matches!(names.source_path(id(3)),
        Err(ConfiguredDiskNameError::Topology { .. })));
    assert!(matches!(names.source_path(id(7)),
        Err(ConfiguredDiskNameError::UnknownDisk { disk_id }) if disk_id == id(7)));
    config.disks[2].upstream_slot_id = Some("d02".into());
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let source = names.source_path(id(3)).unwrap();
    assert_eq!(source.direct_source(), Some(id(2)));
    assert_eq!(source.source_registrations().next().unwrap().slot_id, "D02");
    config.disks[1].upstream_slot_id = Some("D03".into());
    let names = ConfiguredDiskNames::new(&config).unwrap();
    assert!(matches!(names.source_path(id(3)),
        Err(ConfiguredDiskNameError::Topology { .. })));
}
