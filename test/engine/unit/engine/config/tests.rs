use super::*;

#[test]
fn public_validation_checks_paths_without_creating_them() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    Engine::validate_configuration(&config).unwrap();
    for path in ["relative", "/draft/../p"] {
        config.disks[0].root = path.into();
        assert!(Engine::validate_configuration(&config).is_err());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn impossible_upstream_level_is_rejected_without_overflow() {
    let mut config = Config::development(["/draft/p".into(), "/draft/b".into(), "/draft/c".into()]);
    config.disks[0].level = 2;
    config.disks[0].upstream_slot_id = Some("B01".into());
    config.disks[1].level = u32::MAX;
    config.disks[1].upstream_slot_id = Some("C01".into());
    assert!(validate_config(&config).is_err());
}

#[test]
fn one_disk_and_optional_backup_chains_are_valid_without_creating_paths() {
    let root = tempfile::tempdir().unwrap();
    for count in 1..=6 {
        let config = Config::development((0..count).map(|i| root.path().join(format!("disk{i}"))));
        Engine::validate_configuration(&config).unwrap();
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn empty_membership_is_rejected_even_with_data_disks() {
    let mut config = Config::development(Vec::new());
    assert!(validate_config(&config).is_err());
    config = Config::development(["/draft/only".into()]);
    config.disks[0].metadata_member = false;
    assert!(validate_config(&config).is_err());
}

#[test]
fn optional_backup_membership_does_not_require_three_voters() {
    let mut config = Config::development(["/draft/p".into(), "/draft/b".into()]);
    config.disks[1].metadata_member = false;
    validate_config(&config).unwrap();
    assert_eq!(config.metadata_quorum(), 1);
    config.disks[1].upstream_slot_id = None;
    assert!(validate_config(&config).is_err());
}

#[test]
fn retained_workspace_limits_default_for_legacy_configuration_and_reject_zero() {
    let config = Config::development(["/draft/only".into()]);
    let mut value = json_codec::to_value(&config).unwrap();
    for field in [
        "max_retained_workspaces",
        "max_retained_workspaces_per_principal",
        "max_retained_workspace_metadata_bytes",
        "max_retained_workspace_metadata_bytes_per_principal",
    ] {
        value.as_object_mut().unwrap().remove(field);
    }
    let restored: Config = json_codec::from_value(value).unwrap();
    assert_eq!(restored.max_retained_workspaces, 4096);
    assert_eq!(restored.max_retained_workspaces_per_principal, 1024);
    assert_eq!(restored.max_retained_workspace_metadata_bytes, 128 << 20);
    assert_eq!(
        restored.max_retained_workspace_metadata_bytes_per_principal,
        64 << 20
    );
    for field in 0..4 {
        let mut invalid = restored.clone();
        match field {
            0 => invalid.max_retained_workspaces = 0,
            1 => invalid.max_retained_workspaces_per_principal = 0,
            2 => invalid.max_retained_workspace_metadata_bytes = 0,
            _ => invalid.max_retained_workspace_metadata_bytes_per_principal = 0,
        }
        assert_eq!(
            validate_config(&invalid).unwrap_err().code(),
            storage_contract::ErrorCode::ConfigInvalid
        );
    }
}
