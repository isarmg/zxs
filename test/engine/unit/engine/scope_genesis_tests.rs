use super::*;
use crate::Config;
use crate::disk::DiskRoot;
use crate::engine::Engine;
use crate::persist::layout;
use std::fs;
use std::path::{Path, PathBuf};
use storage_contract::working_tree::MaterialCommitment;

fn files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for item in fs::read_dir(path).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn actual_empty_pool_keeps_system_image_on_system_and_two_checked_images_on_each_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let system = tmp.path().join("system");
    let input = Config::development(crate::test_disks(
        (0..3).map(|i| tmp.path().join(format!("disk-{i}"))),
    ))
    .unwrap();
    let engine = Engine::initialize(&system, input).unwrap();
    let config = &engine.config;
    let scope_names = files(&system.join("metadata/objects/scope-closure"));
    assert_eq!(scope_names.len(), 1);
    let raw = fs::read(&scope_names[0]).unwrap();
    let selected: schema::Closure = storage_contract::decode_strict(&raw).unwrap();
    let commitment = MaterialCommitment::from_digest(
        storage_contract::canonical_object_digest("scope-closure", &selected).unwrap(),
    );
    let original =
        readback::Original::new(config, genesis_operation(config).unwrap(), commitment).unwrap();
    let (system_bytes, all_bytes) = original.system(&DiskRoot::open(&system).unwrap()).unwrap();
    assert_eq!(
        files(&system.join("metadata/objects/scope-sqlite-image")).len(),
        1
    );
    let system_name = files(&system.join("metadata/objects/scope-sqlite-image"))
        .pop()
        .unwrap();
    let system_leaf = system_name.strip_prefix(&system).unwrap();
    let mut sum = system_bytes;
    for disk in &config.disks {
        let held = &engine.disk_resources.get(disk).unwrap().root;
        let counts = original.disk(disk, held).unwrap();
        assert_eq!(
            (
                counts.inventory_entries,
                counts.body_entries,
                counts.materials
            ),
            (0, 0, 2)
        );
        sum += counts.canonical_metadata_bytes;
        assert_eq!(
            files(&disk.root.join("metadata/objects/scope-sqlite-image")).len(),
            2
        );
        assert!(!disk.root.join(system_leaf).exists());
        assert!(
            !disk
                .root
                .join(
                    layout::scope_descriptor(objects::Domain::System, selected.system_material)
                        .as_path()
                )
                .exists()
        );
    }
    assert_eq!(sum, all_bytes);
    let disk = &config.disks[0];
    let held = &engine.disk_resources.get(disk).unwrap().root;
    let other = &config.disks[1];
    assert!(original.disk(other, held).is_err());
    let bad_path = files(&disk.root.join("metadata/objects/scope-sqlite-image"))
        .pop()
        .unwrap();
    let mut bad = fs::read(&bad_path).unwrap();
    bad[0] ^= 1;
    fs::write(&bad_path, &bad).unwrap();
    assert!(original.disk(disk, held).is_err());
    assert_eq!(fs::read(&bad_path).unwrap(), bad);
    assert!(
        original
            .disk(other, &engine.disk_resources.get(other).unwrap().root)
            .is_ok()
    );
}
