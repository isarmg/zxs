use super::Shutdown;
use std::sync::{Arc, Barrier};

#[test]
fn stop_before_handoff_prevents_start_and_closes_new_jobs() {
    let shutdown = Shutdown::new();
    let slots = tokio::sync::Semaphore::new(1);
    let held = shutdown.admit(|| slots.try_acquire()).unwrap().unwrap();
    shutdown.stop();
    assert!(!shutdown.queue("late service start"));
    assert!(shutdown.stopping());
    drop(held);
    assert!(shutdown.admit(|| slots.try_acquire()).is_none());
    assert_eq!(shutdown.take_after_drain(), None);
}

#[test]
fn accepted_handoff_survives_drain_notification_and_is_consumed_once() {
    let shutdown = Shutdown::new();
    let observation = shutdown.subscribe();
    assert!(shutdown.queue("confirmed start"));
    assert!(observation.has_changed().unwrap());
    shutdown.stop();
    assert!(!shutdown.queue("replacement"));
    assert_eq!(shutdown.take_after_drain(), Some("confirmed start"));
    assert!(shutdown.stopping());
    assert_eq!(shutdown.take_after_drain(), None);
}

#[test]
fn concurrent_stop_and_handoff_report_the_same_winner_as_the_final_owner() {
    let shutdown = Arc::new(Shutdown::new());
    let barrier = Arc::new(Barrier::new(2));
    let stop = {
        let shutdown = Arc::clone(&shutdown);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            shutdown.stop();
        })
    };
    barrier.wait();
    let accepted = shutdown.queue("one service start");
    stop.join().unwrap();
    assert_eq!(shutdown.take_after_drain(), accepted.then_some("one service start"));
    assert!(shutdown.admit(|| ()).is_none());
}
