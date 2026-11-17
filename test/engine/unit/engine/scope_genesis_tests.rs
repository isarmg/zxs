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
    let fixed = config.fixed_genesis().unwrap();
    let selected: schema::Closure = storage_contract::decode_strict(&raw).unwrap();
    let commitment = MaterialCommitment::from_digest(
        storage_contract::canonical_object_digest("scope-closure", &selected).unwrap(),
    );
    assert_eq!(fixed.closure, commitment);
    let original = readback::Original::new(config, fixed.operation, fixed.closure).unwrap();
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
        let actual = Anchor::read(held).unwrap();
        assert_eq!(actual.fixed_genesis().unwrap().bytes(), fixed.bytes());
        assert!(Anchor::decode(&storage_contract::canonical_json(config).unwrap()).is_err());
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

#[test]
fn damaged_original_system_scope_is_not_reconstructed_on_normal_open() {
    let tmp = tempfile::tempdir().unwrap();
    let system = tmp.path().join("system");
    let input = Config::development(crate::test_disks([tmp.path().join("member")])).unwrap();
    let engine = Engine::initialize(&system, input).unwrap();
    let anchor = engine.config.fixed_genesis().unwrap().bytes().to_vec();
    let disk = engine.config.disks[0].root.clone();
    drop(engine);
    let image = files(&system.join("metadata/objects/scope-sqlite-image"))
        .pop()
        .unwrap();
    let mut bad = fs::read(&image).unwrap();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    fs::write(&image, &bad).unwrap();
    let writer = system.join(layout::CONTROL_WRITER_LOCK.as_path());
    fs::remove_file(&writer).unwrap();
    assert!(Engine::open(&system).is_err());
    assert!(!writer.exists());
    assert_eq!(fs::read(&image).unwrap(), bad);
    assert_eq!(
        fs::read(disk.join(layout::GENESIS_CONFIGURATION.as_path())).unwrap(),
        anchor
    );
}

#[test]
fn original_anchor_rejects_noncanonical_unknown_missing_and_cross_occurrence_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let input = Config::development(crate::test_disks([tmp.path().join("primary")])).unwrap();
    let engine = Engine::initialize(&tmp.path().join("system"), input).unwrap();
    let bytes = engine.config.fixed_genesis().unwrap().bytes();
    let raw: json_codec::Value = storage_contract::decode_strict(bytes).unwrap();
    let mut padded = bytes.to_vec();
    padded.push(b' ');
    assert!(Anchor::decode(&padded).is_err());
    for key in ["scope_closure", "initialization_operation_id", "config"] {
        let mut changed = raw.clone();
        changed.as_object_mut().unwrap().remove(key);
        assert!(
            Anchor::decode(&storage_contract::canonical_json_value(&changed).unwrap()).is_err()
        );
    }
    let mut changed = raw.clone();
    changed["unknown"] = json_codec::Value::Null;
    assert!(Anchor::decode(&storage_contract::canonical_json_value(&changed).unwrap()).is_err());
    let mut changed = raw;
    changed["initialization_operation_id"] =
        json_codec::to_value(storage_contract::OperationId::generate().unwrap()).unwrap();
    assert!(Anchor::decode(&storage_contract::canonical_json_value(&changed).unwrap()).is_err());
}

#[test]
fn previously_admitted_frontier_reads_only_the_selected_original_disk_without_writers() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = [tmp.path().join("primary"), tmp.path().join("peer")];
    let input =
        ValidatedConfig::try_from(Config::development(crate::test_disks(paths.clone())).unwrap())
            .unwrap();
    let system = tmp.path().join("system");
    let engine = Engine::initialize_validated(&system, input.clone()).unwrap();
    let frontier = engine.initial_storage_observation(&input).unwrap();
    let fixed = engine.config.fixed_genesis().unwrap().closure;
    let local = engine.config.disks[0].disk_id.parse().unwrap();
    let peer = engine.config.disks[1].disk_id.parse().unwrap();
    drop(engine);
    fs::rename(&system, tmp.path().join("offline-system")).unwrap();
    fs::rename(&paths[1], tmp.path().join("offline-peer")).unwrap();
    let held = || fs::File::open(&paths[0]).unwrap();
    assert_eq!(
        frontier
            .verify_initial_disk_scope(local, &paths[0], held())
            .unwrap(),
        fixed
    );
    assert!(
        frontier
            .verify_initial_disk_scope(peer, &paths[0], held())
            .is_err()
    );
    let mut manifests = files(&paths[0].join("metadata/objects/disk-scope-manifest"));
    let manifest = manifests.pop().unwrap();
    fs::rename(&manifest, manifest.with_extension("preserved")).unwrap();
    assert!(
        frontier
            .verify_initial_disk_scope(local, &paths[0], held())
            .is_err()
    );
    assert!(!manifest.exists());
    assert!(!paths[1].exists() && !system.exists());
}
