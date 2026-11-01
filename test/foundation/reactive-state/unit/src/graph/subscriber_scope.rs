mod observer_scope_regressions {
    use crate::{
        computed::ArcMemo,
        graph::{untrack, AnySubscriber, ToAnySubscriber, WithObserver},
        owner::Owner,
        signal::ArcRwSignal,
        traits::{Get, Set},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn assert_nested_scope_preserves_untracked(nested: impl FnOnce(&AnySubscriber)) {
        let owner = Owner::new();
        owner.with(|| {
            let signal = ArcRwSignal::new(0);
            let runs = Arc::new(AtomicUsize::new(0));
            let memo = ArcMemo::new({
                let runs = Arc::clone(&runs);
                move |_| {
                    runs.fetch_add(1, Ordering::Relaxed);
                    42
                }
            });
            assert_eq!(memo.get(), 42);
            let subscriber = memo.to_any_subscriber();
            let inner = ArcMemo::new(|_| 0);

            subscriber.with_observer_untracked(|| {
                assert_eq!(signal.get(), 0);
                nested(&inner.to_any_subscriber());
                // This read must remain untracked after the nested scope exits.
                assert_eq!(signal.get(), 0);
            });

            signal.set(1);
            assert_eq!(memo.get(), 42);
            assert_eq!(runs.load(Ordering::Relaxed), 1);
        });
    }

    #[test]
    fn nested_untrack_does_not_enable_outer_tracking() {
        assert_nested_scope_preserves_untracked(|_| untrack(|| {}));
    }

    #[test]
    fn nested_observer_does_not_enable_outer_tracking() {
        assert_nested_scope_preserves_untracked(|inner| inner.with_observer(|| {}));
    }

    #[test]
    fn nested_untracked_observer_does_not_enable_outer_tracking() {
        assert_nested_scope_preserves_untracked(|inner| inner.with_observer_untracked(|| {}));
    }

    #[test]
    fn unwinding_nested_untrack_does_not_enable_outer_tracking() {
        assert_nested_scope_preserves_untracked(|_| {
            assert!(std::panic::catch_unwind(|| untrack(|| panic!("nested callback"))).is_err());
        });
    }
}
