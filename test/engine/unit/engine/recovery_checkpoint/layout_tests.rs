use super::*;
use std::{
    fs,
    os::unix::fs::{MetadataExt, symlink},
};

fn journals(engine: &Engine) -> BTreeMap<String, Vec<(Record, String)>> {
    engine
        .members()
        .iter()
        .map(|disk| {
            let root = &engine.disk_resources.get(disk).unwrap().root;
            (disk.disk_id.clone(), scan_at(root, disk).unwrap().records)
        })
        .collect()
}

fn prefixes(
    journals: &BTreeMap<String, Vec<(Record, String)>>,
) -> BTreeMap<String, BTreeSet<String>> {
    journals
        .iter()
        .map(|(disk, records)| {
            (
                disk.clone(),
                records.iter().map(|(_, hash)| hash.clone()).collect(),
            )
        })
        .collect()
}

#[test]
fn checkpoint_publication_preserves_original_bytes_and_atomic_replacement() {
    let (_temp, system, config) = setup();
    let engine = reopen(&system).0;
    let expected_state = canonical_json(engine.state()).unwrap();
    let journals = journals(&engine);
    let mut before = Vec::new();
    for disk in &config.disks {
        let file = engine
            .disk_resources
            .get(disk)
            .unwrap()
            .root
            .open_optional_read_relative(&layout::REPLAY_CHECKPOINT)
            .unwrap()
            .unwrap();
        before.push((disk, fs::read(path(disk)).unwrap(), file));
    }

    save(&engine, &journals).unwrap();
    for (disk, bytes, file) in before {
        let published = fs::metadata(path(disk)).unwrap();
        assert_ne!(published.ino(), file.metadata().unwrap().ino());
        assert_eq!(published.mode() & 0o7777, 0o600);
        assert_eq!(fs::read(path(disk)).unwrap(), bytes);
        assert_eq!(checkpoint_bytes(file, MAX_CHECKPOINT_BYTES).unwrap(), bytes);
    }
    let envelope: Envelope = read_json(&path(&config.disks[0])).unwrap();
    assert_eq!(
        load(&engine, &prefixes(&journals), &Records::new()).unwrap(),
        Some(envelope.checkpoint),
    );
    drop(engine);
    let cold = reopen(&system).0;
    assert_eq!(canonical_json(cold.state()).unwrap(), expected_state);
}

#[test]
fn checkpoint_consumers_reject_links_and_unsafe_write_parents_without_changing_bytes() {
    for hard_link in [false, true] {
        let (temp, system, config) = setup();
        let engine = reopen(&system).0;
        let journals = journals(&engine);
        let checkpoint = path(&config.disks[0]);
        let original = fs::read(&checkpoint).unwrap();
        let outside = temp.path().join("outside-checkpoint");
        fs::write(&outside, &original).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&checkpoint).unwrap();
        if hard_link {
            fs::hard_link(&outside, &checkpoint).unwrap();
        } else {
            symlink(&outside, &checkpoint).unwrap();
        }
        let linked = fs::symlink_metadata(&checkpoint).unwrap();
        assert!(load(&engine, &prefixes(&journals), &Records::new()).is_err());
        assert!(save(&engine, &journals).is_err());
        assert_eq!(fs::symlink_metadata(&checkpoint).unwrap().ino(), linked.ino());
        assert_eq!(fs::read(&outside).unwrap(), original);

        fs::remove_file(&checkpoint).unwrap();
        write_atomic(&checkpoint, &original, false).unwrap();
        let parent = checkpoint.parent().unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(save(&engine, &journals).is_err());
        assert_eq!(fs::metadata(parent).unwrap().mode() & 0o7777, 0o755);
        assert_eq!(fs::read(&checkpoint).unwrap(), original);
        assert_eq!(fs::read(&outside).unwrap(), original);
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn checkpoint_relative_io_preserves_leaf_absence_parent_errors_limits_and_revocation() {
    let (_temp, system, config) = setup();
    let engine = reopen(&system).0;
    let disk = &config.disks[0];
    let root = &engine.disk_resources.get(disk).unwrap().root;
    let checkpoint = path(disk);
    let original = fs::read(&checkpoint).unwrap();
    fs::remove_file(&checkpoint).unwrap();
    assert!(
        root.open_optional_read_relative(&layout::REPLAY_CHECKPOINT)
            .unwrap()
            .is_none()
    );
    let parent = checkpoint.parent().unwrap();
    let moved = disk.root.join("held-metadata");
    fs::rename(parent, &moved).unwrap();
    assert!(root.open_optional_read_relative(&layout::REPLAY_CHECKPOINT).is_err());
    symlink(&moved, parent).unwrap();
    assert!(root.open_optional_read_relative(&layout::REPLAY_CHECKPOINT).is_err());
    fs::remove_file(parent).unwrap();
    fs::rename(&moved, parent).unwrap();
    root.write_relative_atomic(&layout::REPLAY_CHECKPOINT, &original, true)
        .unwrap();
    let open = || {
        root.open_optional_read_relative(&layout::REPLAY_CHECKPOINT)
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        checkpoint_bytes(open(), 1).unwrap_err().code(),
        ErrorCode::ResourceLimit,
    );
    let idle = open();
    let binding = root.io_binding().unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(binding.status().unwrap().drained());
    assert!(checkpoint_bytes(idle, MAX_CHECKPOINT_BYTES).is_err());
    assert!(root.open_optional_read_relative(&layout::REPLAY_CHECKPOINT).is_err());
    assert!(
        root.write_relative_atomic(&layout::REPLAY_CHECKPOINT, b"replacement", true)
            .is_err()
    );
    assert_eq!(fs::read(&checkpoint).unwrap(), original);
}
