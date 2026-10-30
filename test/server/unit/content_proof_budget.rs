use super::*;

#[test]
fn concurrent_content_opens_share_capacity_until_the_actual_owner_drops() {
    let budget = Budget(AtomicUsize::new(0));
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _reservation = budget.reserve(LIMIT / 2).unwrap();
            barrier.wait();
            barrier.wait();
        });
        barrier.wait();
        assert!(budget.reserve(1).is_err());
        barrier.wait();
    });
    assert_eq!(budget.0.load(Ordering::Relaxed), 0);
    assert!(budget.reserve(LIMIT / 2).is_ok());
    assert!(budget.reserve(LIMIT / 2 + 1).is_err());
    assert!(budget.reserve(usize::MAX).is_err());
}
