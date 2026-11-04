use super::*;

#[test]
fn reservation_drop_preserves_poison_and_other_owners() {
    let (_temp, shared, _backend, _branch) = super::super::tests::fixture();
    let operation_id = OperationId::generate().unwrap();
    let other = OperationId::generate().unwrap();
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
            OperationReservation::acquire(&shared, OperationId::generate().unwrap())
        }))
        .is_err()
    );
}

#[test]
fn reservation_drop_releases_only_its_operation() {
    let (_temp, shared, _backend, _branch) = super::super::tests::fixture();
    let operation_id = OperationId::generate().unwrap();
    let other = OperationId::generate().unwrap();
    let reservation = OperationReservation::acquire(&shared, operation_id).unwrap();
    shared.namespace_operations.lock().unwrap().insert(other);
    drop(reservation);
    assert_eq!(
        *shared.namespace_operations.lock().unwrap(),
        BTreeSet::from([other])
    );
}

#[test]
fn namespace_admission_keeps_operation_reserved_until_budget_cleanup_finishes() {
    for unwind in [false, true] {
        let (_temp, shared, _backend, _branch) = super::super::tests::fixture();
        let identity = shared.auth.sole_identity().unwrap();
        let operation_id = OperationId::generate().unwrap();
        let admission = NamespaceAdmission::acquire(&shared, &identity, operation_id).unwrap();
        let record = shared.spool.join(format!("{operation_id}.budget.json"));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                budget::persist::pause_at(budget::persist::Point::BeforeRemove, barrier);
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _admission = admission;
                    if unwind {
                        panic!("unwind namespace admission");
                    }
                }))
            })
        };
        barrier.wait();
        // The real lease is cleaning up its durable record. A concurrent
        // request must still fail at the operation registry, not race into the
        // charged budget source and report SmbSpoolUnavailable.
        let conflict = OperationReservation::acquire(&shared, operation_id)
            .err()
            .map(|error| error.code);
        let retained = record.try_exists();
        // Release cleanup before assertions so a regression cannot strand it.
        barrier.wait();
        let dropped = worker.join().unwrap();
        assert_eq!(dropped.is_err(), unwind);
        assert_eq!(
            conflict,
            Some(storage_contract::ErrorCode::RecoveryRequired)
        );
        assert!(retained.unwrap());
        assert!(!record.try_exists().unwrap());
        let next = NamespaceAdmission::acquire(&shared, &identity, operation_id).unwrap();
        drop(next);
        assert!(!record.try_exists().unwrap());
    }
}
