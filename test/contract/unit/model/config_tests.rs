use super::*;

#[test]
fn fresh_storage_roots_do_not_infer_backup_edges_from_discovery_order() {
    for mode in [Mode::Development, Mode::Production] {
        let paths = (0..6)
            .map(|index| PathBuf::from(format!("/storage/disk-{index}")))
            .collect::<Vec<_>>();
        let disks = paths.iter().cloned().enumerate().map(|(index, path)| {
            (name_policy::Name::parse(&format!("Storage {index}")).unwrap(), path)
        });
        let config = Config::independent_roots(mode.clone(), disks).unwrap();
        assert_eq!(config.mode, mode);
        assert_eq!(config.disks.len(), paths.len());
        let mut ids = std::collections::BTreeSet::new();
        for (index, disk) in config.disks.iter().enumerate() {
            assert_eq!(disk.root, paths[index]);
            assert_eq!(disk.name.as_str(), format!("Storage {index}"));
            assert_eq!(disk.slot_id, format!("D{:02}", index + 1));
            assert_eq!(disk.level, 1);
            assert_eq!(disk.upstream_slot_id, None);
            assert!(disk.metadata_member);
            assert_eq!(disk.binding_generation, 1);
            assert!(ids.insert(&disk.disk_id));
        }
    }
}

#[test]
fn development_supports_one_disk_and_optional_backup_tiers() {
    for count in 1..=6 {
        let config = Config::development((0..count).map(|i| {
            (name_policy::Name::parse(&format!("Tier {i}")).unwrap(),
                PathBuf::from(format!("/disk/{i}")))
        })).unwrap();
        assert_eq!(config.disks.len(), count);
        assert_eq!(config.metadata_quorum(), count / 2 + 1);
        for (index, disk) in config.disks.iter().enumerate() {
            assert_eq!(disk.level, index as u32 + 1);
            assert!(disk.metadata_member);
            assert_eq!(
                disk.upstream_slot_id.as_ref(),
                index.checked_sub(1).map(|i| &config.disks[i].slot_id)
            );
        }
    }
}

#[test]
fn recovery_quorum_uses_configured_metadata_members_not_data_disk_count() {
    let mut config = Config::development((0..5).map(|i| {
        (name_policy::Name::parse(&format!("Tier {i}")).unwrap(),
            PathBuf::from(format!("/disk/{i}")))
    })).unwrap();
    config.disks[3].metadata_member = false;
    config.disks[4].metadata_member = false;
    assert_eq!(config.metadata_quorum(), 2);
    // Missing roots do not change the configured recovery threshold.
    for disk in &mut config.disks {
        disk.root = PathBuf::from("/nonexistent/disk");
    }
    assert_eq!(config.metadata_quorum(), 2);
}

#[test]
fn pool_semantic_format_is_required_and_only_accepts_the_exact_token() {
    let config = Config::development([(
        name_policy::Name::parse("Primary").unwrap(),
        PathBuf::from("/disk/primary"),
    )])
    .unwrap();
    let explicit = json_codec::to_vec(&config).unwrap();
    let decoded: Config = json_codec::from_slice(&explicit).unwrap();
    assert_eq!(decoded.pool_semantic_format, config.pool_semantic_format);

    let mut explicit: json_codec::Value = json_codec::from_slice(&explicit).unwrap();
    assert_eq!(explicit["pool_semantic_format"], "manual-directory-v1");
    let mut missing = explicit.clone();
    missing.as_object_mut().unwrap().remove("pool_semantic_format");
    assert!(json_codec::from_value::<Config>(missing).is_err());
    for invalid in [
        json_codec::json!(null),
        json_codec::json!(1),
        json_codec::json!("manual-directory-v2"),
        json_codec::json!({"manual-directory-v1": null}),
    ] {
        explicit["pool_semantic_format"] = invalid;
        assert!(json_codec::from_value::<Config>(explicit.clone()).is_err());
    }
}
