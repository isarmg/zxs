//! Management consumers preserve each explicitly selected role and operation.
use super::*;

fn mixed_request(f: &Fixture) -> AdditionRequest {
    let mut request = request(f);
    let root = f.discovery.join("fresh-data");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let stopped = f
        .original
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    // Creating another direct child changes the held parent identity. Bind
    // every selection after the complete set of candidate roots exists.
    for addition in &mut request.additions {
        addition.selection = crate::storage_directories::inspect_fresh_within(
            f.discovery.to_str().unwrap(),
            &InspectRequest {
                mount_path: addition.selection.mount_path.clone(),
                root_path: addition.selection.root_path.clone(),
            },
            &[f.data.clone(), f.backend.inner.management_root.clone()],
            &stopped,
        )
        .unwrap();
    }
    let listing = crate::storage_directories::browse(root.to_str().unwrap()).unwrap();
    let selection = crate::storage_directories::inspect_fresh_within(
        f.discovery.to_str().unwrap(),
        &InspectRequest {
            mount_path: listing.mount_path,
            root_path: root.to_str().unwrap().into(),
        },
        &[f.data.clone(), f.backend.inner.management_root.clone()],
        &stopped,
    )
    .unwrap();
    request.additions.push(DirectoryAddition {
        selection,
        slot_id: "N02".into(),
        level: 1,
        upstream_slot_id: None,
        metadata_member: false,
    });
    request
}

#[test]
fn terminal_mixed_api_preserves_requested_roles_and_published_result() {
    let f = fixture();
    let request = mixed_request(&f);
    let preview: AdditionPreview = json_codec::from_value(
        command(&f, "disk_add_preview", json!(request), &f.identity).unwrap(),
    )
    .unwrap();
    assert_eq!(preview.added_directories, 2);
    assert_eq!(preview.target_metadata_members, 1);
    assert_eq!(preview.request, request);
    assert!(!preview.request.additions[1].metadata_member);
    let body = json!(AdditionCommit {
        preview: preview.clone()
    });
    let completed: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_commit", body.clone(), &f.identity).unwrap())
            .unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert_eq!(
        completed.result.as_ref().unwrap().result["metadata_members"],
        1
    );
    let backend = f.backend.inner.backend().unwrap();
    let config = backend.addition_configuration(&f.identity).unwrap();
    let data = config
        .disks
        .iter()
        .find(|disk| disk.slot_id == "N02")
        .unwrap();
    assert!(!data.metadata_member);
    assert!(
        backend
            .operation(&f.identity, f.historical_operation)
            .is_ok()
    );
    let writer = fs::File::open(data.root.join(".writer.lock")).unwrap();
    assert!(
        rustix::fs::flock(
            &writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .is_err()
    );
    drop(writer);
    drop(backend);
    let stored = read_record(&directory(&f), request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status.preview.request, request);
    assert!(
        !stored
            .target
            .disks
            .iter()
            .find(|disk| disk.slot_id == "N02")
            .unwrap()
            .metadata_member
    );
    let repeated: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_commit", body, &f.identity).unwrap()).unwrap();
    assert_eq!(repeated.result, completed.result);
}

#[test]
fn terminal_mixed_api_cold_reentry_retains_readiness_failure_and_original_handle() {
    let f = fixture();
    let request = mixed_request(&f);
    let saved = cold_pending_request(&f, request.clone());
    let data = saved
        .target
        .disks
        .iter()
        .find(|disk| disk.slot_id == "N02")
        .unwrap();
    let namespace = data.root.join("metadata/membership-pending");
    let operation = fs::read_dir(&namespace)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let readiness = operation.join("data-ready.json");
    let bytes = fs::read(&readiness).unwrap();
    fs::write(&readiness, b"unrelated data readiness").unwrap();
    let body = json!(AdditionResume {
        operation_id: saved.status.operation_id,
        continuation: AdditionContinuation::Add
    });
    let error = command(&f, "disk_add_resume", body.clone(), &f.identity).unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::MetadataResume(_)
    ));
    let writer = fs::File::open(data.root.join(".writer.lock")).unwrap();
    assert!(
        rustix::fs::flock(
            &writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .is_err()
    );
    drop(writer);
    assert_eq!(fs::read(&readiness).unwrap(), b"unrelated data readiness");
    let stored = read_record(&directory(&f), request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status.preview.request, request);
    assert_eq!(stored.status.state, AdditionState::RecoveryRequired);
    assert!(stored.status.result.is_none());
    fs::write(&readiness, bytes).unwrap();
    let completed: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_resume", body, &f.identity).unwrap()).unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert_eq!(completed.operation_id, request.operation_id);
    assert_eq!(completed.preview.request, request);
    assert_eq!(
        completed.result.as_ref().unwrap().result["metadata_members"],
        1
    );
}
