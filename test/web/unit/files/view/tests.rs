use super::super::model::{Disk, Model};
use super::*;
use crate::files::model::FileOperation;

#[test]
fn disk_roots_preserve_loading_empty_and_refresh_states() {
    for loading_disks in [false, true] {
        for disks_loaded in [false, true] {
            for disks in [vec![], vec![disk("offline", false, false, 0)]] {
                let model = Model {
                    loading_disks,
                    disks_loaded,
                    disks: disks.clone(),
                    ..Model::default()
                };
                let expected = if loading_disks && !disks_loaded {
                    DiskRoots::Loading
                } else if disks_loaded && disks.is_empty() {
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
        slot_id: slot.into(),
        online,
        writable,
        available_bytes,
    }
}
