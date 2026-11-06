use super::super::model::{Disk, Model};
use super::*;
use crate::files::model::FileOperation;

#[test]
fn disk_roots_preserve_rows_while_loading_and_only_confirm_empty_after_load() {
    for loading_disks in [false, true] {
        for disks_loaded in [false, true] {
            for disks in [vec![], vec![disk("offline", false, false, 0)]] {
                let model = Model {
                    loading_disks,
                    disks_loaded,
                    disks: disks.clone(),
                    ..Model::default()
                };
                let expected = if disks_loaded && disks.is_empty() {
                    DiskRoots::Empty
                } else {
                    DiskRoots::Disks(disks)
                };
                assert_eq!(disk_roots_projection(&model), expected);
            }
        }
    }
}

#[test]
fn disk_read_status_distinguishes_unknown_cached_loading_and_current_snapshots() {
    for (loading, loaded, cached, expected) in [
        (false, false, false, DiskRead::Unconfirmed),
        (false, false, true, DiskRead::UnconfirmedCached),
        (true, false, false, DiskRead::Reading),
        (true, false, true, DiskRead::RefreshingCached),
        (true, true, false, DiskRead::Reading),
        (true, true, true, DiskRead::RefreshingCached),
        (false, true, false, DiskRead::Current),
        (false, true, true, DiskRead::Current),
    ] {
        let model = Model {
            loading_disks: loading,
            disks_loaded: loaded,
            disks: if cached {
                vec![disk("P01", true, true, 7)]
            } else {
                vec![]
            },
            ..Model::default()
        };
        assert_eq!(disk_read_projection(&model), expected);
        assert_eq!(model.disks.len(), usize::from(cached));
        if cached {
            assert!(model.disks[0].online && model.disks[0].writable);
        }
    }
}

#[test]
fn disk_roots_preserve_order_access_capacity_and_ignore_directory_state() {
    let disks = vec![
        disk("offline", false, false, 0),
        disk("read-only", true, false, 123),
        disk("writable", true, true, u64::MAX),
    ];
    let mut model = Model {
        disks: disks.clone(),
        disks_loaded: true,
        ..Model::default()
    };
    model.directory = "unrelated/path".into();
    model.selected = Some(crate::files::test_entries::entry(
        json_codec::json!({"name": "unrelated"}),
    ));
    model.operations = vec![FileOperation::unknown("unrelated")];
    model.checked_entries.insert("unrelated".into());
    assert_eq!(disk_roots_projection(&model), DiskRoots::Disks(disks));
}

fn disk(slot: &str, online: bool, writable: bool, available_bytes: u64) -> Disk {
    Disk {
        name: "disk-directory".into(),
        slot_id: slot.into(),
        online,
        writable,
        available_bytes: Some(available_bytes),
        observation_failure: None,
    }
}

#[test]
fn disk_capacity_unknown_is_not_zero_and_failure_has_a_safe_explanation() {
    assert_eq!(disk_available_space(None), t("不可用", "Unavailable"));
    assert_eq!(disk_available_space(Some(0)), format::bytes(0));
    assert!(disk_observation_message(None).is_none());
    assert_eq!(
        disk_observation_message(Some(storage_contract::DiskObservationIssue::Seal(
            storage_contract::ErrorCode::IoError
        ))),
        Some(t("磁盘身份观测失败", "Disk identity observation failed"))
    );
    let mut unavailable = disk("P01", false, true, 0);
    unavailable.available_bytes = None;
    unavailable.observation_failure = Some(storage_contract::DiskObservationIssue::Seal(
        storage_contract::ErrorCode::IoError,
    ));
    let model = Model {
        disks: vec![unavailable.clone()],
        disks_loaded: true,
        ..Model::default()
    };
    assert_eq!(
        disk_roots_projection(&model),
        DiskRoots::Disks(vec![unavailable])
    );
}
