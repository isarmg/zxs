use super::*;

#[test]
fn reservation_drop_preserves_poison_and_other_owners() {
    let (_temp, shared, _backend, _branch) = super::super::tests::fixture();
    let operation_id = OperationId::generate();
    let other = OperationId::generate();
    let reservation = OperationReservation::acquire(&shared, operation_id).unwrap();
    shared.namespace_operations.lock().unwrap().insert(other);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = shared.namespace_operations.lock().unwrap();
        panic!("poison");
    }));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _reservation = reservation;
            panic!("outer unwind");
        }))
        .is_err()
    );
    assert!(shared.namespace_operations.is_poisoned());
    assert_eq!(
        *shared.namespace_operations.lock().unwrap_err().into_inner(),
        BTreeSet::from([operation_id, other])
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            OperationReservation::acquire(&shared, OperationId::generate())
        }))
        .is_err()
    );
}

#[test]
fn reservation_drop_releases_only_its_operation() {
    let (_temp, shared, _backend, _branch) = super::super::tests::fixture();
    let operation_id = OperationId::generate();
    let other = OperationId::generate();
    let reservation = OperationReservation::acquire(&shared, operation_id).unwrap();
    shared.namespace_operations.lock().unwrap().insert(other);
    drop(reservation);
    assert_eq!(
        *shared.namespace_operations.lock().unwrap(),
        BTreeSet::from([other])
    );
}
