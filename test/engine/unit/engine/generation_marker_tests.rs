use super::*;
use crate::disk::{decode_seal, private_dir, read_file_bytes};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

type Media = BTreeMap<PathBuf, (u64, u64, u32, u64, Vec<u8>)>;

fn media(root: &Path) -> Media {
    fn visit(root: &Path, path: &Path, result: &mut Media) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else if metadata.file_type().is_symlink() {
            fs::read_link(path).unwrap().as_os_str().as_encoded_bytes().to_vec()
        } else {
            Vec::new()
        };
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (metadata.dev(), metadata.ino(), metadata.mode(), metadata.len(), bytes),
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn generation_marker_absence_does_not_hide_unsafe_parents_or_leaves() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("disk");
    private_dir(&path).unwrap();
    let root = DiskRoot::open(&path).unwrap();
    let marker = path.join(GENERATION_FILE);
    let parent = marker.parent().unwrap();
    let metadata = parent.parent().unwrap();
    assert!(open_generation_marker(&root, &path).unwrap().is_none());
    private_dir(metadata).unwrap();
    assert!(open_generation_marker(&root, &path).unwrap().is_none());
    private_dir(parent).unwrap();
    assert!(open_generation_marker(&root, &path).unwrap().is_none());

    // Actual unsafe objects must remain errors, not the optional absence case.
    let outside = temp.path().join("outside");
    private_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"must not be adopted or changed").unwrap();
    let outside_before = media(&outside);
    for component in [metadata, parent] {
        let parked = temp.path().join("parked");
        fs::rename(component, &parked).unwrap();
        for linked in [false, true] {
            if linked {
                symlink(&outside, component).unwrap();
            } else {
                fs::write(component, b"not a directory").unwrap();
            }
            let before = media(&path);
            let error = open_generation_marker(&root, &path).err().unwrap();
            assert_eq!(error.code(), ErrorCode::UnsafePath);
            assert!(std::error::Error::source(&error).is_some());
            assert_eq!(media(&path), before);
            assert_eq!(media(&outside), outside_before);
            fs::remove_file(component).unwrap();
        }
        fs::rename(&parked, component).unwrap();
    }
    symlink(outside.join("missing"), &marker).unwrap();
    let before = media(&path);
    let error = open_generation_marker(&root, &path).err().unwrap();
    assert_eq!(error.code(), ErrorCode::UnsafePath);
    assert_eq!(media(&path), before);
    fs::remove_file(&marker).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    let before = media(&path);
    let error = open_generation_marker(&root, &path).err().unwrap();
    assert_eq!(error.code(), ErrorCode::UnsafePath);
    assert_eq!(media(&path), before);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(open_generation_marker(&root, &path).unwrap().is_none());
}

#[test]
fn generation_marker_observation_stays_on_the_original_held_root() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("disk");
    let marker = path.join(GENERATION_FILE);
    private_dir(marker.parent().unwrap()).unwrap();
    fs::write(&marker, b"original held marker bytes").unwrap();
    let root = DiskRoot::open(&path).unwrap();
    let root_identity = root.directory_identity(&path).unwrap();
    let original = fs::metadata(&marker).unwrap();
    let retained = temp.path().join("retained");
    fs::rename(&path, &retained).unwrap();
    private_dir(marker.parent().unwrap()).unwrap();
    fs::write(&marker, b"replacement named root bytes").unwrap();
    let replacement_before = media(&path);
    let retained_before = media(&retained);

    let mut file = open_generation_marker(&root, &path).unwrap().unwrap();
    let observed = file.metadata().unwrap();
    assert_eq!((observed.dev(), observed.ino()), (original.dev(), original.ino()));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original held marker bytes");
    assert_eq!(root.directory_identity(&path).unwrap(), root_identity);
    assert_eq!(media(&retained), retained_before);
    assert_eq!(media(&path), replacement_before);

    fs::remove_file(retained.join(GENERATION_FILE)).unwrap();
    assert!(open_generation_marker(&root, &path).unwrap().is_none());
    assert_eq!(media(&path), replacement_before);
    let replacement = DiskRoot::open(&path).unwrap();
    assert_eq!(
        read_file_bytes(open_generation_marker(&replacement, &path).unwrap().unwrap(), 128)
            .unwrap(),
        b"replacement named root bytes",
    );
}

#[test]
fn current_restore_requires_its_actual_generation_marker_at_every_absence_depth() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::independent_roots(
        storage_contract::Mode::Development,
        crate::test_disks((0..3).map(|index| temp.path().join(format!("disk-{index}")))),
    )
    .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine
        .create_disk_root("D02", "admin", storage_contract::OperationId::generate().unwrap())
        .unwrap();
    let stopped = engine.config.disks[0].disk_id.clone();
    engine
        .begin_disk_exit(
            &stopped,
            crate::MembershipExitAction::PlannedOffline,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(engine.resume_membership_joint_decision().unwrap().all_certificates_synchronized());
    engine.activate_pending_disk_exit().unwrap();
    let expected = engine.config.generation().unwrap().hash.clone();
    let disk = engine.config.metadata_members().next().unwrap().clone();
    let seal = decode_seal(&fs::read(disk.root.join(layout::DISK_IDENTITY.as_path())).unwrap())
        .unwrap();
    assert!(matches!(&seal.format, DiskSealFormat::MembershipGeneration { authority_hash }
        if authority_hash == &expected));
    drop(engine);
    let admitted = crate::engine::config::read_recovery_anchor(&disk.root, &seal).unwrap();
    assert_eq!(admitted.generation().unwrap().hash, expected);
    let marker = disk.root.join(GENERATION_FILE);
    let parent = marker.parent().unwrap();
    let metadata = parent.parent().unwrap();
    for missing in [marker.as_path(), parent, metadata] {
        let parked = temp.path().join("parked-authority");
        fs::rename(missing, &parked).unwrap();
        let before = media(temp.path());
        let error = crate::engine::config::read_recovery_anchor(&disk.root, &seal)
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert_eq!(media(temp.path()), before);
        fs::rename(&parked, missing).unwrap();
        let admitted = crate::engine::config::read_recovery_anchor(&disk.root, &seal).unwrap();
        assert_eq!(admitted.generation().unwrap().hash, expected);
    }
    let valid = fs::read(&marker).unwrap();
    fs::write(&marker, b"{malformed generation authority").unwrap();
    let before = media(temp.path());
    assert!(crate::engine::config::read_recovery_anchor(&disk.root, &seal).is_err());
    assert_eq!(media(temp.path()), before);
    fs::write(&marker, valid).unwrap();
    let admitted = crate::engine::config::read_recovery_anchor(&disk.root, &seal).unwrap();
    assert_eq!(admitted.generation().unwrap().hash, expected);
}
