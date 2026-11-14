use crate::container_storage::version_rename::{
    CompletionBinding, Prepared, finish, interrupted_path,
};
use crate::container_storage::{VersionNameError, installed_version_path};
use crate::disk::DiskRoot;
use crate::{Error, FileVersion, State};
use name_policy::Name;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use storage_contract::{ContentHash, FileId, HistoryTreeId, VersionId, VersionStorageId};

fn fixture() -> (tempfile::TempDir, DiskRoot, State, String, String, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let container = temp.path().join("original.txt");
    root.private_dir(&container).unwrap();
    root.private_dir(&container.join("versions")).unwrap();
    let file = FileId::from_random([1; 16]).to_string();
    let history = HistoryTreeId::from_random([2; 16]).to_string();
    let mut state = State::default();
    state.sequence = 9;
    for index in [3u8, 4] {
        let id = VersionId::from_random([index; 16]);
        let bytes = [index; 8];
        let version = FileVersion {
            history_tree_id: history.clone(),
            version_id: id.to_string(),
            file_id: file.clone(),
            creation_primary_slot_id: "P01".into(),
            logical_size: bytes.len() as u64,
            version_sequence: index as u64,
            content_hash: ContentHash::from_digest(blake3::hash(&bytes)),
            checksum_chunk_bytes: 262_144,
            checksum_manifest_hash: blake3::hash(b"fixture metadata").to_hex().to_string(),
            creation_name: "original.txt".into(),
            storage_id: VersionStorageId::from_version(id),
            created_at: "2026-11-13T00:00:00Z".into(),
            source_version_ids: Vec::new(),
            created_as_full_checkpoint: true,
        };
        let path = installed_version_path(&container, &version).unwrap();
        root.write_atomic(&path, &bytes, false).unwrap();
        state.versions.insert(version.version_id.clone(), version);
    }
    (temp, root, state, file, history, container)
}

#[test]
fn decided_version_rename_resumes_partial_leaves_and_keeps_bodies_ids_and_inodes() {
    for stop in ["version-names-intent", "version-name-renamed"] {
        let (temp, root, state, file, history, container) = fixture();
        let state_before = storage_contract::canonical_json(&state).unwrap();
        let target_name = Name::parse("latest.bin").unwrap();
        let before = state.versions.values().map(|version| {
            let path = installed_version_path(&container, version).unwrap();
            (version.version_id.clone(), std::fs::metadata(&path).unwrap().ino(),
                std::fs::read(path).unwrap())
        }).collect::<Vec<_>>();
        let prepared = Prepared::capture(
            &root, &container, &state, &file, &history, &target_name,
        ).unwrap().unwrap();
        assert!(prepared.apply(&root, &container, &mut |point| {
            if point == stop { Err(Error::VersionNames(VersionNameError::Intent)) }
            else { Ok(()) }
        }).is_err());
        drop(root);
        let root = DiskRoot::open(temp.path()).unwrap();
        for version in state.versions.values() {
            let path = interrupted_path(&root, &container, version).unwrap().unwrap();
            assert!(path.is_file());
        }
        let recovered = Prepared::capture(
            &root, &container, &state, &file, &history, &target_name,
        ).unwrap().unwrap();
        recovered.apply(&root, &container, &mut |_| Ok(())).unwrap();
        let target = container.with_file_name(target_name.as_str());
        root.rename_noreplace_synced(&container, &target).unwrap();
        finish(&root, &target, &CompletionBinding {
            file: file.parse().unwrap(),
            history: history.parse().unwrap(),
            sequence: storage_contract::Sequence::new(state.sequence),
            target_name: Name::parse("latest.bin").unwrap(),
        }).unwrap();
        for (id, inode, bytes) in before {
            let version = &state.versions[&id];
            let path = installed_version_path(&target, version).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            assert!(interrupted_path(&root, &target, version).unwrap().is_none());
        }
        assert_eq!(storage_contract::canonical_json(&state).unwrap(), state_before);
    }
}

#[test]
fn version_rename_rejects_an_existing_target_without_overwrite() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    let first = state.versions.values().next().unwrap();
    let leaf = crate::container_storage::installed_version_leaf(first.storage_id, &name).unwrap();
    let collision = container.join("versions").join(leaf.as_path());
    root.write_atomic(&collision, b"unrelated", false).unwrap();
    let source = installed_version_path(&container, first).unwrap();
    let before = std::fs::read(&source).unwrap();
    assert!(Prepared::capture(&root, &container, &state, &file, &history, &name).is_err());
    assert_eq!(std::fs::read(source).unwrap(), before);
    assert_eq!(std::fs::read(collision).unwrap(), b"unrelated");
}

#[test]
fn completed_leaves_resume_before_and_after_outer_container_rename() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap().apply(&root, &container, &mut |_| Ok(())).unwrap();
    // All leaves reached the new name, but the container still has its old name.
    let recovered = Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap();
    for version in state.versions.values() {
        let path = interrupted_path(&root, &container, version).unwrap().unwrap();
        assert_eq!(std::fs::read(path).unwrap().len() as u64, version.logical_size);
    }
    recovered.apply(&root, &container, &mut |_| Ok(())).unwrap();
    let target = container.with_file_name(name.as_str());
    root.rename_noreplace_synced(&container, &target).unwrap();
    // Outer placement is complete, while FILE publication/intent cleanup can resume.
    assert!(Prepared::capture(&root, &target, &state, &file, &history, &name)
        .unwrap().is_none());
    for version in state.versions.values() {
        assert_eq!(interrupted_path(&root, &target, version).unwrap().unwrap(),
            installed_version_path(&target, version).unwrap());
    }
    finish(&root, &target, &CompletionBinding {
            file: file.parse().unwrap(),
            history: history.parse().unwrap(),
            sequence: storage_contract::Sequence::new(state.sequence),
            target_name: Name::parse("latest.bin").unwrap(),
        }).unwrap();
    finish(&root, &target, &CompletionBinding {
            file: file.parse().unwrap(),
            history: history.parse().unwrap(),
            sequence: storage_contract::Sequence::new(state.sequence),
            target_name: Name::parse("latest.bin").unwrap(),
        }).unwrap();
}

#[test]
fn prepared_version_rename_rejects_replaced_source_inode_without_moving_it() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    let prepared = Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap();
    let version = state.versions.values().next().unwrap();
    let source = installed_version_path(&container, version).unwrap();
    let retained = source.with_extension("retained");
    let original = std::fs::read(&source).unwrap();
    root.rename_noreplace_synced(&source, &retained).unwrap();
    root.write_atomic(&source, &original, false).unwrap();
    assert!(prepared.apply(&root, &container, &mut |_| Ok(())).is_err());
    let leaf = crate::container_storage::installed_version_leaf(version.storage_id, &name)
        .unwrap();
    assert!(!container.join("versions").join(leaf.as_path()).exists());
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(std::fs::read(retained).unwrap(), original);
}

#[test]
fn rename_reservation_covers_atomic_intent_and_retained_source_intent() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    let prepared = Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap();
    let fresh = prepared.reservation().unwrap();
    assert!(fresh.0 > 0);
    assert_eq!(fresh.1, 2);
    assert!(prepared.apply(&root, &container, &mut |_| {
        Err(Error::VersionNames(VersionNameError::Intent))
    }).is_err());
    let recovered = Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap();
    let retained = recovered.reservation().unwrap();
    assert_eq!(retained.0, fresh.0 * 2);
    assert_eq!(retained.1, 2);
}

#[test]
fn recovery_rejects_intent_subset_and_target_unbound_to_candidate() {
    use crate::container_storage::version_rename::{Intent, intent_path};
    for subset in [true, false] {
        let (_temp, root, state, file, history, container) = fixture();
        let name = Name::parse("latest.bin").unwrap();
        let prepared = Prepared::capture(&root, &container, &state, &file, &history, &name)
            .unwrap().unwrap();
        assert!(prepared.apply(&root, &container, &mut |_| {
            Err(Error::VersionNames(VersionNameError::Intent))
        }).is_err());
        let path = intent_path(&container);
        let mut intent: Intent = storage_contract::decode_strict(
            &std::fs::read(&path).unwrap(),
        ).unwrap();
        if subset {
            intent.bodies.pop().unwrap();
        } else {
            intent.target_name = Name::parse("unselected.bin").unwrap();
        }
        let changed = storage_contract::canonical_json(&intent).unwrap();
        root.write_atomic(&path, &changed, true).unwrap();
        assert!(Prepared::capture(&root, &container, &state, &file, &history, &name).is_err());
        assert_eq!(std::fs::read(path).unwrap(), changed);
        for version in state.versions.values() {
            assert!(installed_version_path(&container, version).unwrap().is_file());
        }
    }
}

#[test]
fn intent_cleanup_requires_the_callers_publication_binding() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap().apply(&root, &container, &mut |_| Ok(())).unwrap();
    let target = container.with_file_name(name.as_str());
    root.rename_noreplace_synced(&container, &target).unwrap();
    let intent_path = crate::container_storage::version_rename::intent_path(&target);
    let before = std::fs::read(&intent_path).unwrap();
    let mut binding = CompletionBinding {
        file: file.parse().unwrap(),
        history: history.parse().unwrap(),
        sequence: storage_contract::Sequence::new(10),
        target_name: name,
    };
    assert!(finish(&root, &target, &binding).is_err());
    assert_eq!(std::fs::read(&intent_path).unwrap(), before);
    binding.sequence = storage_contract::Sequence::new(state.sequence);
    binding.target_name = Name::parse("unselected.bin").unwrap();
    assert!(finish(&root, &target, &binding).is_err());
    assert_eq!(std::fs::read(&intent_path).unwrap(), before);
}

#[test]
fn read_locator_rejects_container_outside_both_intent_endpoints() {
    let (_temp, root, state, file, history, container) = fixture();
    let name = Name::parse("latest.bin").unwrap();
    let prepared = Prepared::capture(&root, &container, &state, &file, &history, &name)
        .unwrap().unwrap();
    assert!(prepared.apply(&root, &container, &mut |_| {
        Err(Error::VersionNames(VersionNameError::Intent))
    }).is_err());
    let unrelated = container.with_file_name("unselected.bin");
    root.rename_noreplace_synced(&container, &unrelated).unwrap();
    for version in state.versions.values() {
        assert!(interrupted_path(&root, &unrelated, version).is_err());
    }
}

#[test]
fn resumed_version_rename_rejects_changed_metadata_at_source_and_completed_target() {
    for stop in ["version-names-intent", "version-name-renamed"] {
        let (temp, root, state, file, history, container) = fixture();
        let name = Name::parse("latest.bin").unwrap();
        let prepared = Prepared::capture(&root, &container, &state, &file, &history, &name)
            .unwrap().unwrap();
        assert!(prepared.apply(&root, &container, &mut |point| {
            if point == stop {
                Err(Error::VersionNames(VersionNameError::Intent))
            } else {
                Ok(())
            }
        }).is_err());
        drop(root);
        let root = DiskRoot::open(temp.path()).unwrap();
        let recovered = Prepared::capture(&root, &container, &state, &file, &history, &name)
            .unwrap().unwrap();
        let retained = state.versions.values().map(|version| {
            let path = interrupted_path(&root, &container, version).unwrap().unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            (version, path, metadata, bytes)
        }).collect::<Vec<_>>();
        let (version, path, before, _) = &retained[0];
        assert_eq!(
            path == &installed_version_path(&container, version).unwrap(),
            stop == "version-names-intent",
        );
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(before.mode() ^ 0o100),
        ).unwrap();
        let changed = std::fs::metadata(path).unwrap();
        assert_eq!((changed.dev(), changed.ino()), (before.dev(), before.ino()));
        assert_eq!(changed.len(), before.len());
        assert_eq!(
            (changed.mtime(), changed.mtime_nsec()),
            (before.mtime(), before.mtime_nsec()),
        );
        assert_ne!(changed.mode(), before.mode());
        assert!(matches!(
            recovered.apply(&root, &container, &mut |_| Ok(())),
            Err(Error::VersionNames(VersionNameError::Intent)),
        ));
        for (version, path, before, bytes) in retained {
            assert_eq!(interrupted_path(&root, &container, version).unwrap(), Some(path.clone()));
            let after = std::fs::metadata(&path).unwrap();
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}
