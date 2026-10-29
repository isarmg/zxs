use super::*;

#[test]
fn public_validation_checks_paths_without_creating_them() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    ValidatedConfig::try_from(config.clone()).unwrap();
    for path in ["relative", "/draft/../p"] {
        config.disks[0].root = path.into();
        assert!(ValidatedConfig::try_from(config.clone()).is_err());
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
    assert!(ValidatedConfig::try_from(config.clone()).is_err());
}

#[test]
fn one_disk_and_optional_backup_chains_are_valid_without_creating_paths() {
    let root = tempfile::tempdir().unwrap();
    for count in 1..=6 {
        let config = Config::development((0..count).map(|i| root.path().join(format!("disk{i}"))));
        ValidatedConfig::try_from(config.clone()).unwrap();
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn empty_membership_is_rejected_even_with_data_disks() {
    let mut config = Config::development(Vec::new());
    assert!(ValidatedConfig::try_from(config.clone()).is_err());
    config = Config::development(["/draft/only".into()]);
    config.disks[0].metadata_member = false;
    assert!(ValidatedConfig::try_from(config.clone()).is_err());
}

#[test]
fn optional_backup_membership_does_not_require_three_voters() {
    let mut config = Config::development(["/draft/p".into(), "/draft/b".into()]);
    config.disks[1].metadata_member = false;
    ValidatedConfig::try_from(config.clone()).unwrap();
    assert_eq!(config.metadata_quorum(), 1);
    config.disks[1].upstream_slot_id = None;
    assert!(ValidatedConfig::try_from(config.clone()).is_err());
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
            ValidatedConfig::try_from(invalid).unwrap_err().code(),
            storage_contract::ErrorCode::ConfigInvalid
        );
    }
}

#[test]
fn validated_configuration_serialization_preserves_the_disk_contract() {
    let config = Config::development(["/draft/p".into(), "/draft/b".into()]);
    let encoded = json_codec::to_vec(&config).unwrap();
    let validated: ValidatedConfig = decode_strict(&encoded).unwrap();
    assert_eq!(json_codec::to_vec(&validated).unwrap(), encoded);

    let mut invalid = json_codec::to_value(&config).unwrap();
    invalid["max_file_bytes"] = json_codec::json!("0");
    assert!(json_codec::from_value::<ValidatedConfig>(invalid).is_err());
    let mut unknown = json_codec::to_value(&config).unwrap();
    unknown["unknown_option"] = json_codec::json!(true);
    assert!(json_codec::from_value::<ValidatedConfig>(unknown).is_err());
}

#[test]
fn validated_configuration_rejects_external_limits_and_identities() {
    let config = Config::development(["/draft/p".into(), "/draft/b".into()]);
    let invalid: &[fn(&mut Config)] = &[
        |c| c.pool_id = "bad".into(),
        |c| c.max_file_bytes = 0,
        |c| c.max_file_bytes = u64::MAX,
        |c| {
            c.max_file_bytes = u64::MAX;
            c.max_workspace_bytes = u64::MAX;
        },
        |c| c.max_workspace_bytes = 0,
        |c| c.max_workspace_bytes = c.max_file_bytes - 1,
        |c| c.max_workspaces = 0,
        |c| c.max_entries = 0,
        |c| c.checksum_chunk_bytes = 0,
        |c| c.checksum_chunk_bytes = 1,
        |c| c.disks[0].slot_id = "bad/slot".into(),
        |c| c.disks[1].slot_id = "p01".into(),
        |c| c.disks[0].disk_id = "bad".into(),
        |c| c.disks[1].disk_id = c.disks[0].disk_id.clone(),
        |c| c.disks[0].binding_generation = 0,
        |c| c.disks[0].level = 0,
        |c| c.disks[0].upstream_slot_id = Some("B01".into()),
        |c| c.disks[1].upstream_slot_id = None,
        |c| c.disks[1].upstream_slot_id = Some("unknown".into()),
        |c| c.disks[1].level = 3,
    ];
    for mutate in invalid {
        let mut candidate = config.clone();
        mutate(&mut candidate);
        assert!(ValidatedConfig::try_from(candidate).is_err());
    }
    ValidatedConfig::try_from(config).unwrap();
}

#[test]
fn root_rebinding_preserves_admitted_topology_and_only_replaces_roots() {
    let config = Config::development(["/draft/p".into(), "/draft/b".into()]);
    let mut expected = config.clone();
    expected.disks[0].root = "/moved/p".into();
    expected.disks[1].root = "/moved/b".into();
    let moved = ValidatedConfig::try_from(config)
        .unwrap()
        .with_disk_roots(vec!["/moved/p".into(), "/moved/b".into()])
        .unwrap();
    assert_eq!(
        json_codec::to_vec(&moved).unwrap(),
        json_codec::to_vec(&expected).unwrap()
    );
}

#[test]
fn root_rebinding_rejects_external_count_traversal_overlap_and_aliases() {
    let config =
        ValidatedConfig::try_from(Config::development(["/draft/p".into(), "/draft/b".into()]))
            .unwrap();
    for roots in [
        vec!["/moved/p"],
        vec!["/moved/p", "relative"],
        vec!["/moved/p", "/moved/q/../b"],
        vec!["/moved/p", "/moved/p/nested"],
        vec!["/moved/p/nested", "/moved/p"],
        vec!["/moved/p", "/moved/p"],
    ] {
        assert_eq!(
            config
                .clone()
                .with_disk_roots(roots.into_iter().map(PathBuf::from).collect())
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }
    let root = tempfile::tempdir().unwrap();
    let disk = root.path().join("disk");
    let alias = root.path().join("alias");
    std::fs::create_dir(&disk).unwrap();
    std::os::unix::fs::symlink(&disk, &alias).unwrap();
    assert!(
        config
            .with_disk_roots(vec![
                disk.canonicalize().unwrap(),
                alias.canonicalize().unwrap()
            ])
            .is_err()
    );
}

#[test]
fn admitted_runtime_limits_survive_root_rebinding_but_are_not_persisted() {
    let config = Config::development(["/draft/p".into()]);
    let encoded = json_codec::to_vec(&config).unwrap();
    let config = ValidatedConfig::try_from(config).unwrap();
    let baseline = super::super::RetainedWorkspaceLimits::from_config(&config);
    let mut raised = baseline;
    raised.max_retained_workspaces += 1;
    let admitted = config.with_workspace_limits(Some(raised)).unwrap();
    assert_eq!(admitted.effective_workspace_limits(), raised);
    assert_eq!(json_codec::to_vec(&admitted).unwrap(), encoded);
    let restored: ValidatedConfig = decode_strict(&encoded).unwrap();
    assert_eq!(restored.effective_workspace_limits(), baseline);
    let moved = admitted.with_disk_roots(vec!["/moved/p".into()]).unwrap();
    assert_eq!(moved.effective_workspace_limits(), raised);
    for lower in [0, baseline.max_retained_workspaces - 1] {
        let mut limits = baseline;
        limits.max_retained_workspaces = lower;
        assert!(
            restored
                .clone()
                .with_workspace_limits(Some(limits))
                .is_err()
        );
    }
}

#[test]
fn restore_rebinds_moved_member_and_preserves_the_admitted_pool_baseline() {
    let temporary = tempfile::tempdir().unwrap();
    let original_root = temporary.path().join("disk");
    let mut expected = Config::development([original_root.clone()]);
    expected.max_workspaces = 7;
    drop(Engine::initialize(&temporary.path().join("system"), expected.clone()).unwrap());
    let moved = temporary.path().join("moved-disk");
    std::fs::rename(original_root, &moved).unwrap();
    expected.disks[0].root = moved.clone();
    let system = temporary.path().join("restored-system");
    let restored = Engine::restore_from_disks(&system, &[moved]).unwrap();
    assert_eq!(
        canonical_json(restored.config()).unwrap(),
        canonical_json(&expected).unwrap()
    );
    drop(restored);
    assert_eq!(
        canonical_json(Engine::open(&system).unwrap().config()).unwrap(),
        canonical_json(&expected).unwrap()
    );
}

#[test]
fn restore_rejects_rebound_metadata_root_nested_in_an_unsupplied_data_root() {
    let temporary = tempfile::tempdir().unwrap();
    let primary = temporary.path().join("primary");
    let backup = temporary.path().join("backup");
    let mut config = Config::development([primary.clone(), backup.clone()]);
    config.disks[1].metadata_member = false;
    drop(Engine::initialize(&temporary.path().join("system"), config).unwrap());
    let moved = backup.join("moved-primary");
    std::fs::rename(primary, &moved).unwrap();
    let system = temporary.path().join("restored-system");
    let error = Engine::restore_from_disks(&system, &[moved]).err().unwrap();
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    assert!(!system.exists());
}

#[test]
fn validated_config_shares_large_topology_and_rebinds_roots_without_changing_snapshot() {
    let mut draft = Config::development((0..96).map(|i| PathBuf::from(format!("/draft/disk-{i}"))));
    for disk in &mut draft.disks[1..] {
        disk.metadata_member = false;
    }
    let config = ValidatedConfig::try_from(draft).unwrap();
    let snapshot = config.clone();
    assert!(Arc::ptr_eq(&config.admitted, &snapshot.admitted));
    assert_eq!(&*snapshot.admitted.metadata_members, &[0]);
    let rebound = config
        .with_disk_roots(
            (0..96)
                .map(|i| PathBuf::from(format!("/new/disk-{i}")))
                .collect(),
        )
        .unwrap();
    assert!(!Arc::ptr_eq(&rebound.admitted, &snapshot.admitted));
    assert_eq!(
        rebound.admitted.metadata_members,
        snapshot.admitted.metadata_members
    );
    assert_eq!(
        snapshot.metadata_members().next().unwrap().root,
        Path::new("/draft/disk-0")
    );
    assert_eq!(
        rebound.metadata_members().next().unwrap().root,
        Path::new("/new/disk-0")
    );
    let encoded = json_codec::to_vec(&snapshot).unwrap();
    let decoded: ValidatedConfig = json_codec::from_slice(&encoded).unwrap();
    assert_eq!(&*decoded.admitted.metadata_members, &[0]);
    assert_eq!(encoded, json_codec::to_vec(&*snapshot).unwrap());
}

#[test]
fn restart_limit_rebinding_keeps_the_previous_admitted_snapshot() {
    let original = ValidatedConfig::try_from(Config::development(["/draft/p".into()])).unwrap();
    let baseline = original.effective_workspace_limits();
    let mut raised = baseline;
    raised.max_retained_workspaces += 1;
    let override_config = original
        .clone()
        .with_workspace_limits(Some(raised))
        .unwrap();
    assert!(!Arc::ptr_eq(&original.admitted, &override_config.admitted));
    assert_eq!(original.effective_workspace_limits(), baseline);
    assert_eq!(override_config.effective_workspace_limits(), raised);
    let restored = override_config.clone().with_workspace_limits(None).unwrap();
    assert_eq!(restored.effective_workspace_limits(), baseline);
    assert_eq!(override_config.effective_workspace_limits(), raised);
    assert_eq!(
        json_codec::to_vec(&original).unwrap(),
        json_codec::to_vec(&override_config).unwrap()
    );
}

#[test]
fn admitted_config_serialization_matches_fixed_raw_config_bytes() {
    let wire = concat!(
        r#"{"pool_id":"pool_00000000000000000000000000000001","mode":"development","disks":["#,
        r#"{"slot_id":"P01","disk_id":"disk_00000000000000000000000000000002","root":"/fixed/member","level":1,"upstream_slot_id":null,"metadata_member":true,"binding_generation":"1","capacity_bytes":null}],"#,
        r#""checksum_chunk_bytes":262144,"max_file_bytes":"1024","max_workspace_bytes":"2048","max_workspaces":3,"#,
        r#""max_retained_workspaces":4,"max_retained_workspaces_per_principal":2,"max_retained_workspace_metadata_bytes":"4096","max_retained_workspace_metadata_bytes_per_principal":"2048","#,
        r#""max_entries":5,"metadata_reserve_bytes":"512","workspace_ttl_seconds":"60"}"#,
    );
    let raw: Config = decode_strict(wire.as_bytes()).unwrap();
    assert_eq!(json_codec::to_vec(&raw).unwrap(), wire.as_bytes());
    let admitted = ValidatedConfig::try_from(raw).unwrap();
    assert_eq!(json_codec::to_vec(&admitted).unwrap(), wire.as_bytes());
    let decoded: ValidatedConfig = decode_strict(wire.as_bytes()).unwrap();
    assert_eq!(json_codec::to_vec(&decoded).unwrap(), wire.as_bytes());
}

#[test]
fn payload_proof_budget_is_runtime_only_and_preserves_snapshot_policy() {
    let config = Config::development(["/draft/only".into()]);
    let mut admitted = ValidatedConfig::try_from(config).unwrap();
    assert_eq!(admitted.max_held_payload_files().get(), 128);
    let bytes = canonical_json(&admitted).unwrap();
    let digest = object_hash("publication-config", &admitted).unwrap();
    let snapshot = admitted.clone();
    admitted.set_max_held_payload_files(std::num::NonZeroUsize::new(512).unwrap());
    assert_eq!(admitted.max_held_payload_files().get(), 512);
    assert_eq!(snapshot.max_held_payload_files().get(), 128);
    assert_eq!(canonical_json(&admitted).unwrap(), bytes);
    assert_eq!(
        object_hash("publication-config", &admitted).unwrap(),
        digest
    );
    let reopened: ValidatedConfig = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(reopened.max_held_payload_files().get(), 128);
}
