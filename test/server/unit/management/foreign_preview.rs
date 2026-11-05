use super::super::foreign_disk_fixture::*;
use super::*;
use storage_contract::storage_directories::DiscoveredDiskState;

#[test]
fn readonly_foreign_preview_keeps_multiple_origins_and_missing_bodies() {
    let f = fixture();
    let original = bytes(&f.source);
    let local = bytes(&f.known);
    let preview = inspect(&f, source(&f)).unwrap();
    assert_eq!(preview.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(preview.source_level, 2);
    assert_eq!(preview.histories.len(), 2);
    let first = preview
        .histories
        .iter()
        .find(|history| history.origin.slot_id == "P01")
        .unwrap();
    let second = preview
        .histories
        .iter()
        .find(|history| history.origin.slot_id == "B01")
        .unwrap();
    assert_eq!(first.verified_local_versions, 1);
    assert!(first.temporarily_unavailable_versions.is_empty());
    assert_eq!(second.verified_local_versions, 0);
    assert_eq!(second.temporarily_unavailable_versions.len(), 1);
    assert_ne!(first.origin.disk_id, second.origin.disk_id);
    assert_ne!(first.group_id, second.group_id);
    for history in &preview.histories {
        assert_eq!(
            history.versions,
            history.verified_local_versions
                + history.temporarily_unavailable_versions.len()
                + history.recorded_purged_versions
                + history.recorded_migrated_versions
        );
        assert!(history.branches > 0 && history.commit_records > 0);
    }
    assert_eq!(preview.review_digest.len(), 64);
    assert_eq!(preview, inspect(&f, source(&f)).unwrap());
    assert_eq!(bytes(&f.source), original);
    assert_eq!(bytes(&f.known), local);
}

#[test]
fn foreign_preview_rejects_other_paths_changed_identity_and_lifecycle_races() {
    let f = fixture();
    let selected = source(&f);
    for path in [
        f.known.clone(),
        f.source.join("metadata"),
        f._root.path().join("primary-one"),
    ] {
        let mut request = selected.clone();
        request.path = path.to_str().unwrap().into();
        assert!(inspect(&f, request).is_err());
    }
    let mut changed = selected.clone();
    changed.identity.inode += 1;
    assert!(inspect(&f, changed).is_err());
    let gate = crate::lock(&f.backend.inner.storage_addition);
    assert_eq!(
        inspect(&f, selected).unwrap_err().code,
        storage_contract::ErrorCode::StorageRetryRunning
    );
    drop(gate);
}

#[test]
fn foreign_preview_never_uses_revoked_credentials_or_extra_commands() {
    let f = fixture();
    let selected = source(&f);
    let mut input = json_codec::to_value(ForeignDiskPreviewRequest {
        source: selected.clone(),
    })
    .unwrap();
    input["execute"] = json!(true);
    assert!(
        f.backend
            .setup_operation("foreign_disk_preview", input, None, Some(&f.identity))
            .is_err()
    );
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    let result = inspect(&f, selected).unwrap_err();
    assert_eq!(result.status, http::StatusCode::UNAUTHORIZED);
}

#[test]
fn foreign_preview_can_read_a_relocated_source_without_admitting_it() {
    fn copy(source: &Path, destination: &Path) {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(destination)
            .unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let output = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &output);
            } else {
                fs::copy(entry.path(), output).unwrap();
            }
        }
    }
    let f = fixture();
    let moved = f.source.parent().unwrap().join("moved");
    copy(&f.source, &moved);
    let original = inspect(&f, source(&f)).unwrap();
    let before = bytes(&moved);
    let discovered = f.backend.discovered_disks(&f.identity).unwrap();
    let candidate = discovered
        .candidates
        .into_iter()
        .find(|candidate| candidate.name == "moved")
        .unwrap();
    let DiscoveredDiskState::Unsupported {
        source: Some(source),
        ..
    } = candidate.state
    else {
        panic!("relocation must not relax the ordinary seal registration check")
    };
    let preview = inspect(&f, source).unwrap();
    assert_eq!(preview.identity, original.identity);
    assert_eq!(preview.review_digest, original.review_digest);
    assert_eq!(preview.histories, original.histories);
    assert_ne!(
        preview.source.identity.inode,
        original.source.identity.inode
    );
    assert_eq!(preview.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(before, bytes(&moved));
}
