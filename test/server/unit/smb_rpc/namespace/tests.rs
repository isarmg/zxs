use super::*;

#[test]
fn namespace_record_noclobber_preserves_existing_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(directory.path().join("spool")).unwrap();
    let name = EntryName::new("operation.namespace.json").unwrap();
    let path = directory.path().join(name.as_path());
    fs::write(&path, b"earlier recovery evidence").unwrap();
    assert!(write_namespace_record(&directory, &name, &json!({"replacement":true})).is_err());
    assert_eq!(fs::read(path).unwrap(), b"earlier recovery evidence");
}

#[test]
fn namespace_record_rejects_replaced_owner_without_touching_either_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("spool");
    let directory = PrivateDirectory::create(&path).unwrap();
    let moved = temp.path().join("held");
    fs::rename(&path, &moved).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    let name = EntryName::new("operation.namespace.json").unwrap();
    let error = write_namespace_record(&directory, &name, &json!({"intent":true})).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert_eq!(fs::read_dir(moved).unwrap().count(), 0);
    assert_eq!(fs::read_dir(replacement.path()).unwrap().count(), 0);
}

#[test]
fn namespace_record_errors_preserve_io_code_and_original_source() {
    use std::error::Error as _;
    for source in [
        platform_io::fs_safety::Error::Io(io::Error::from_raw_os_error(5)),
        platform_io::fs_safety::Error::PublishedDurabilityUnknown(io::Error::from_raw_os_error(5)),
    ] {
        let error = namespace_record_error(source);
        assert_eq!(error.code, storage_contract::ErrorCode::IoError);
        let mut cause = error.source().unwrap();
        while let Some(next) = cause.source() {
            cause = next;
        }
        assert_eq!(
            cause.downcast_ref::<io::Error>().unwrap().raw_os_error(),
            Some(5)
        );
    }
}

#[cfg(feature = "core")]
#[test]
fn operation_reservation_terminal_release_preserves_poison_without_a_second_panic() {
    let (_fixture, shared, _backend, _branch) = crate::smb_rpc::tests::fixture();
    let operation = OperationId::generate();
    let reservation = OperationReservation::acquire(&shared, operation).unwrap();
    assert!(
        shared
            .namespace_operations
            .lock()
            .unwrap()
            .contains(&operation)
    );
    drop(reservation);
    assert!(shared.namespace_operations.lock().unwrap().is_empty());
    let reservation = OperationReservation::acquire(&shared, operation).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _reservation = reservation;
        let _operations = shared.namespace_operations.lock().unwrap();
        panic!("namespace registry failed while an operation was active");
    }));
    assert!(panic.is_err());
    assert!(shared.namespace_operations.is_poisoned());
    let poisoned = shared.namespace_operations.lock().err().unwrap();
    assert!(poisoned.get_ref().contains(&operation));
}
