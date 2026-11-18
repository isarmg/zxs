//! Only a whole outer container moves; every real BODY keeps its immutable
//! relative name and must still be the same opened file at completion.
use super::*;
use crate::container_storage::installed_version_path;
use crate::{FileVersion, State};
use crate::state_projection::ProjectionSnapshot;
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
fn one_outer_rename_keeps_each_old_relative_leaf_and_inode_without_writing_an_intent() {
    let (_temporary, root, state, file, history, container) = fixture();
    let projection = ProjectionSnapshot::new(&state);
    let before: Vec<_> = state.versions.values().map(|version| {
        let path = installed_version_path(&container, version).unwrap();
        (version, std::fs::metadata(&path).unwrap().ino(), std::fs::read(&path).unwrap())
    }).collect();
    let held = HeldBodies::capture(&root, &container, projection.view(), &file, &history, None).unwrap();
    let target = container.with_file_name("latest.bin");
    root.rename_noreplace_synced(&container,&target).unwrap();
    held.verify_at(&root,&target).unwrap();
    assert!(!target.join(".zxs-version-names.json").exists());
    for (version, inode, bytes) in before {
        let path = installed_version_path(&target,version).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(),inode);
        assert_eq!(std::fs::read(&path).unwrap(),bytes);
        assert!(!target.join("versions").join(format!("{}-latest.bin",version.storage_id)).exists());
    }
}

#[test]
fn original_name_same_bytes_or_an_extra_name_cannot_replace_the_held_original() {
    for substitute in [false,true] {
        let (temporary, root, state, file, history, container) = fixture();
        let projection = ProjectionSnapshot::new(&state);
        let held = HeldBodies::capture(&root,&container,projection.view(),&file,&history,None).unwrap();
        let version = state.versions.values().next().unwrap();
        let path = installed_version_path(&container,version).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let parked = temporary.path().join("parked-original");
        let foreign = path.with_file_name(format!("{}-later.txt",version.storage_id));
        if substitute {
            root.rename_noreplace_synced(&path,&parked).unwrap();
            root.write_atomic(&path,&bytes,false).unwrap();
        } else {
            root.write_atomic(&foreign,&bytes,false).unwrap();
        }
        assert!(held.verify_at(&root,&container).is_err());
        assert_eq!(std::fs::read(&path).unwrap(),bytes);
        assert_eq!(std::fs::read(if substitute {&parked} else {&foreign}).unwrap(),bytes);
    }
}

#[test]
fn an_available_routed_body_is_missing_even_if_the_current_outer_suffix_is_present() {
    let (_temporary, root, mut state, file, history, container) = fixture();
    let version = state.versions.values().next().unwrap().clone();
    let key = version.version_id.clone();
    state.content_states.insert(key.clone(),crate::ContentState::Available);
    state.placements.insert(key.clone(),crate::Placement {
        version_id:key, primary_slot_id:"P01".into(),target_copies:1,
        route_slot_ids:vec!["P01".into()],topology_generation:1,binding_generations:Default::default(),
    });
    let path = installed_version_path(&container,&version).unwrap();
    let wrong = path.with_file_name(format!("{}-current.txt",version.storage_id));
    root.rename_noreplace_synced(&path,&wrong).unwrap();
    let bytes = std::fs::read(&wrong).unwrap();
    let projection = ProjectionSnapshot::new(&state);
    assert!(HeldBodies::capture(&root,&container,projection.view(),&file,&history,Some("P01")).is_err());
    assert_eq!(std::fs::read(&wrong).unwrap(),bytes);
    assert!(!path.exists());
}
