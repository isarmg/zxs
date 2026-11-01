use super::*;

#[test]
fn development_supports_one_disk_and_optional_backup_tiers() {
    for count in 1..=6 {
        let config =
            Config::development((0..count).map(|i| PathBuf::from(format!("/disk/{i}")))).unwrap();
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
    let mut config =
        Config::development((0..5).map(|i| PathBuf::from(format!("/disk/{i}")))).unwrap();
    config.disks[3].metadata_member = false;
    config.disks[4].metadata_member = false;
    assert_eq!(config.metadata_quorum(), 2);
    // Missing roots do not change the configured recovery threshold.
    for disk in &mut config.disks {
        disk.root = PathBuf::from("/nonexistent/disk");
    }
    assert_eq!(config.metadata_quorum(), 2);
}
