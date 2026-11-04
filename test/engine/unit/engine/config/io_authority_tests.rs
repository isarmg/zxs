use super::*;
use crate::{RetainedWorkspaceLimits, ValidatedConfig};
use std::sync::Arc;

fn configuration() -> Config {
    let mut config = Config::development(
        ["/genesis-primary", "/genesis-data", "/genesis-backup"].map(Into::into),
    )
    .unwrap();
    config.disks[1].metadata_member = false;
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    config.disks[2].metadata_member = false;
    config.disks[2].level = 2;
    config
}

#[test]
fn every_persisted_control_field_participates_in_genesis_context_equality() {
    type ConfigChange = (&'static str, fn(&mut Config));
    let original = ValidatedConfig::try_from(configuration()).unwrap();
    let changes: &[ConfigChange] = &[
        ("pool", |config| {
            config.pool_id = crate::new_id("pool").unwrap()
        }),
        ("mode", |config| config.mode = Mode::Production),
        ("checksum size", |config| {
            config.checksum_chunk_bytes = crate::SUPPORTED_CHECKSUM_CHUNK_BYTES[2]
        }),
        ("file limit", |config| config.max_file_bytes -= 1),
        ("workspace bytes", |config| config.max_workspace_bytes += 1),
        ("workspaces", |config| config.max_workspaces += 1),
        ("retained workspaces", |config| {
            config.max_retained_workspaces += 1
        }),
        ("retained per principal", |config| {
            config.max_retained_workspaces_per_principal -= 1
        }),
        ("retained metadata", |config| {
            config.max_retained_workspace_metadata_bytes += 1
        }),
        ("retained metadata per principal", |config| {
            config.max_retained_workspace_metadata_bytes_per_principal += 1
        }),
        ("entries", |config| config.max_entries += 1),
        ("metadata reserve", |config| {
            config.metadata_reserve_bytes += 1
        }),
        ("workspace TTL", |config| config.workspace_ttl_seconds += 1),
        ("data-only identity", |config| {
            config.disks[2].disk_id = crate::new_id("disk").unwrap()
        }),
        ("data-only slot", |config| {
            config.disks[2].slot_id = "D01".into()
        }),
        ("data-only binding", |config| {
            config.disks[2].binding_generation += 1
        }),
        ("data-only role", |config| {
            config.disks[2].metadata_member = true
        }),
        ("data-only level", |config| {
            config.disks[2].level = 1;
            config.disks[2].upstream_slot_id = None;
        }),
        ("data-only upstream", |config| {
            config.disks[2].upstream_slot_id = Some("P01".into())
        }),
        ("data-only capacity", |config| {
            config.disks[2].capacity_bytes = Some(4096)
        }),
        ("data-only removal", |config| {
            config.disks.pop();
        }),
        ("data-only addition", |config| {
            let mut disk = config.disks[1].clone();
            disk.disk_id = crate::new_id("disk").unwrap();
            disk.slot_id = "D01".into();
            disk.root = "/genesis-added-data".into();
            config.disks.push(disk);
        }),
    ];
    for (description, change) in changes {
        let mut changed = (*original).clone();
        change(&mut changed);
        let changed = ValidatedConfig::try_from(changed).unwrap();
        assert!(
            !original.same_genesis_io_authority(&changed),
            "ignored persisted field: {description}",
        );
        if original.effective_workspace_limits() == changed.effective_workspace_limits() {
            assert!(
                !original.permits_genesis_recovery_projection(&changed),
                "recovery accepted changed control field: {description}",
            );
        }
    }
}

#[test]
fn only_order_roots_and_separate_restart_overrides_are_normalized() {
    let original = ValidatedConfig::try_from(configuration()).unwrap();
    let reopened = ValidatedConfig::try_from((*original).clone()).unwrap();
    assert!(original.same_genesis_io_authority(&reopened));
    let mut reordered = (*original).clone();
    reordered.disks.reverse();
    let reordered = ValidatedConfig::try_from(reordered).unwrap();
    assert!(original.same_genesis_io_authority(&reordered));

    let rebound = original
        .clone()
        .with_disk_roots(
            ["/relocated-primary", "/relocated-data", "/relocated-backup"]
                .map(Into::into)
                .to_vec(),
        )
        .unwrap();
    assert!(original.same_genesis_io_authority(&rebound));
    assert!(Arc::ptr_eq(
        &original.admitted.genesis_io_authority,
        &rebound.admitted.genesis_io_authority,
    ));
    let limits = RetainedWorkspaceLimits {
        max_retained_workspaces: original.max_retained_workspaces + 1,
        max_retained_workspaces_per_principal: original.max_retained_workspaces_per_principal + 1,
        max_retained_workspace_metadata_bytes: original.max_retained_workspace_metadata_bytes + 1,
        max_retained_workspace_metadata_bytes_per_principal: original
            .max_retained_workspace_metadata_bytes_per_principal
            + 1,
    };
    let overridden = rebound.with_workspace_limits(Some(limits)).unwrap();
    assert!(original.same_genesis_io_authority(&overridden));
    assert!(Arc::ptr_eq(
        &original.admitted.genesis_io_authority,
        &overridden.admitted.genesis_io_authority,
    ));
}

#[test]
fn the_cached_projection_does_not_change_persisted_configuration_bytes() {
    let config = configuration();
    let expected = crate::canonical_json(&config).unwrap();
    let admitted = ValidatedConfig::try_from(config).unwrap();
    assert_eq!(crate::canonical_json(&admitted).unwrap(), expected);
}

#[test]
fn genesis_recovery_only_allows_each_retained_limit_to_tighten() {
    let original = ValidatedConfig::try_from(configuration()).unwrap();
    let changes: &[fn(&mut Config)] = &[
        |config| config.max_retained_workspaces -= 1,
        |config| config.max_retained_workspaces_per_principal -= 1,
        |config| config.max_retained_workspace_metadata_bytes -= 1,
        |config| config.max_retained_workspace_metadata_bytes_per_principal -= 1,
    ];
    for change in changes {
        let mut local = (*original).clone();
        change(&mut local);
        let local = ValidatedConfig::try_from(local).unwrap();
        assert!(original.permits_genesis_recovery_projection(&local));
        assert!(!local.permits_genesis_recovery_projection(&original));
        assert!(!original.same_genesis_io_authority(&local));
    }
    let mut mixed = (*original).clone();
    mixed.max_retained_workspaces -= 1;
    mixed.max_retained_workspace_metadata_bytes += 1;
    assert!(
        !original.permits_genesis_recovery_projection(&ValidatedConfig::try_from(mixed).unwrap())
    );
}
