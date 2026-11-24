use super::*;

const A: Token = [1; 32];
const B: Token = [2; 32];
const X: Id = [1; 16];
const Y: Id = [2; 16];

fn reserve<'a>(
    placement: &'a Placement,
    id: Id,
    bytes: u64,
    points: &[Sample],
) -> io::Result<Option<Intake<'a>>> {
    placement.reserve(placement.generation()?, id, bytes, points)
}

#[test]
fn greatest_qualified_available_then_smallest_stable_token() {
    let ledger = Placement::default();
    let first = reserve(&ledger, X, 1, &[(A, HEADROOM + 1), (B, HEADROOM + 2)])
        .unwrap()
        .unwrap();
    assert_eq!(first.token, B);
    drop(first);
    let tie = reserve(&ledger, X, 1, &[(B, HEADROOM + 3), (A, HEADROOM + 3)])
        .unwrap()
        .unwrap();
    assert_eq!(tie.token, A);
}

#[test]
fn in_flight_same_container_stays_on_one_candidate_and_counts_both_bodies() {
    let ledger = Placement::default();
    let first = reserve(&ledger, X, 8, &[(A, HEADROOM + 16), (B, HEADROOM + 22)])
        .unwrap()
        .unwrap();
    let points = [(A, HEADROOM + 12), (B, HEADROOM + 22)];
    let exact_retry = reserve(&ledger, X, 8, &points).unwrap().unwrap();
    assert_eq!(first.token, B);
    assert_eq!(exact_retry.token, B);
    let another = reserve(&ledger, Y, 8, &points).unwrap().unwrap();
    assert_eq!(another.token, A);
    assert!(matches!(
        reserve(&ledger, X, 1, &[(A, HEADROOM + 100)]),
        Err(error) if error.kind() == io::ErrorKind::NotConnected
    ));
}

#[test]
fn release_requires_resampling_and_capacity_errors_do_not_create_a_lease() {
    let ledger = Placement::default();
    let prior_generation = ledger.generation().unwrap();
    let samples = [(A, HEADROOM + 20)];
    let first = reserve(&ledger, X, 14, &samples).unwrap().unwrap();
    assert!(
        ledger
            .reserve(prior_generation, Y, 7, &samples)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        reserve(&ledger, Y, 7, &samples),
        Err(error) if error.kind() == io::ErrorKind::StorageFull
    ));
    let before_release = ledger.generation().unwrap();
    drop(first);
    assert!(
        ledger
            .reserve(before_release, Y, 7, &samples)
            .unwrap()
            .is_none()
    );
    let only = reserve(&ledger, Y, 7, &samples).unwrap().unwrap();
    assert_eq!(only.token, A);
}

#[test]
fn no_qualified_sample_differs_from_full_and_overflow_fails_closed() {
    let ledger = Placement::default();
    let bad = |bytes, points: &[Sample], kind| {
        assert!(matches!(reserve(&ledger, X, bytes, points), Err(error) if error.kind() == kind));
    };
    bad(1, &[], io::ErrorKind::NotConnected);
    bad(1, &[(A, 0)], io::ErrorKind::StorageFull);
    bad(u64::MAX, &[(A, u64::MAX)], io::ErrorKind::StorageFull);
    assert!(ledger.active.lock().unwrap().containers.is_empty());
}

#[test]
fn provisional_slots_remain_bounded_even_for_empty_bodies() {
    let ledger = Placement::default();
    let points = [(A, HEADROOM)];
    let held: Vec<_> = (0..MAX_INTAKES)
        .map(|_| reserve(&ledger, X, 0, &points).unwrap().unwrap())
        .collect();
    assert!(matches!(
        reserve(&ledger, Y, 0, &points),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));
    drop(held);
    assert_eq!(reserve(&ledger, Y, 0, &points).unwrap().unwrap().token, A);
}
