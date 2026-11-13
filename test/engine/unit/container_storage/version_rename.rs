use crate::container_storage::version_rename::{Prepared, finish, interrupted_path};
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
        let (_temp, root, state, file, history, container) = fixture();
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
        finish(&root, &target).unwrap();
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
