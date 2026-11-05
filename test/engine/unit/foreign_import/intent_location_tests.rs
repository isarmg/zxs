use super::*;

#[test]
fn held_ancestry_detects_both_containment_directions_and_separate_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let nested = source.join("nested");
    let separate = temp.path().join("separate");
    private_dir(&nested).unwrap();
    private_dir(&separate).unwrap();
    let source = DiskRoot::open(&source).unwrap();
    let nested = DiskRoot::open(&nested).unwrap();
    let separate = DiskRoot::open(&separate).unwrap();
    let source_location = Location::held(&source).unwrap();
    let nested_location = Location::held(&nested).unwrap();
    let separate_location = Location::held(&separate).unwrap();
    assert!(nested_location.descends_from(&source_location).unwrap());
    assert!(!source_location.descends_from(&nested_location).unwrap());
    assert!(!separate_location.descends_from(&source_location).unwrap());
    assert!(reject_overlap(&source, &nested).is_err());
    assert!(reject_overlap(&nested, &source).is_err());
    reject_overlap(&source, &separate).unwrap();
}

#[test]
fn different_mount_observations_fail_closed_without_simulating_mount_success() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("held-root");
    private_dir(&path).unwrap();
    let root = DiskRoot::open(&path).unwrap();
    let location = Location::held(&root).unwrap();
    let mut other_view = Location::held(&root).unwrap();
    other_view.mount_id += 1;
    assert!(location.descends_from(&other_view).is_err());
    // This is a typed boundary negative, not a real bind-mount integration test.
}

#[test]
fn held_ancestry_keeps_real_managed_io_active_until_its_descriptors_drop() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let root = &engine
        .disk_resources
        .get(engine.disk("P01").unwrap())
        .unwrap()
        .root;
    let binding = root.io_binding().unwrap();
    let before = binding.status().unwrap().ordinary;
    let held = Location::held(root).unwrap();
    assert_eq!(binding.status().unwrap().ordinary, before + 1);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(!binding.status().unwrap().drained());
    assert!(held.directory.metadata().unwrap().is_dir());
    assert!(Location::held(root).is_err());
    drop(held);
    assert!(binding.status().unwrap().ordinary <= before);
}
