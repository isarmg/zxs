use super::*;
use std::sync::{
    Barrier, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

// Timeouts are deadlock watchdogs only; all tested ordering uses gates/channels.
const WATCHDOG: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn wait(&self) {
        let (open, _) = self
            .changed
            .wait_timeout_while(self.open.lock().unwrap(), WATCHDOG, |open| !*open)
            .unwrap();
        assert!(*open, "test gate was not released");
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

struct ExitNotice(mpsc::Sender<()>);
impl Drop for ExitNotice {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
thread_local! {
    // Observing worker exit proves its failure has been handled, rather than
    // racing a signal emitted inside the work callback immediately before Err.
    static WORKER_EXIT: std::cell::RefCell<Option<ExitNotice>> = const {
        std::cell::RefCell::new(None)
    };
}

#[test]
fn independent_payload_workers_are_bounded_and_file_order_is_serial() {
    let jobs: Vec<_> = (0..8).flat_map(|file| [(file, 0), (file, 1)]).collect();
    let groups: Vec<_> = jobs.chunks(2).map(|jobs| jobs.iter().collect()).collect();
    let progress: Vec<_> = (0..8).map(|_| AtomicUsize::new(0)).collect();
    let active = AtomicUsize::new(0);
    let maximum = AtomicUsize::new(0);
    let barrier = Barrier::new(4);
    run_payload_groups(&groups, |&(file, step)| {
        assert_eq!(progress[file].fetch_add(1, Ordering::SeqCst), step);
        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
        maximum.fetch_max(now, Ordering::SeqCst);
        barrier.wait();
        active.fetch_sub(1, Ordering::SeqCst);
        barrier.wait();
        Ok(())
    })
    .unwrap();
    assert_eq!(maximum.load(Ordering::SeqCst), 4);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(
        progress
            .iter()
            .all(|count| count.load(Ordering::SeqCst) == 2)
    );
}

#[test]
fn empty_and_single_file_groups_keep_exact_serial_execution() {
    run_payload_groups::<usize>(&[], |_| panic!("empty input must not run work")).unwrap();
    let jobs: Vec<_> = (0..7).collect();
    let groups = vec![jobs.iter().collect()];
    let next = AtomicUsize::new(0);
    run_payload_groups(&groups, |&step| {
        assert_eq!(next.fetch_add(1, Ordering::SeqCst), step);
        Ok(())
    })
    .unwrap();
    assert_eq!(next.load(Ordering::SeqCst), jobs.len());
}

#[test]
fn successful_groups_reuse_at_most_four_workers_including_partial_tail() {
    let jobs: Vec<_> = (0..13).collect();
    let groups: Vec<_> = jobs.iter().map(|job| vec![job]).collect();
    let visits: Vec<_> = jobs.iter().map(|_| AtomicUsize::new(0)).collect();
    let threads = Mutex::new(std::collections::HashSet::new());
    run_payload_groups(&groups, |&job| {
        threads.lock().unwrap().insert(std::thread::current().id());
        visits[job].fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    assert!(threads.into_inner().unwrap().len() <= 4);
    assert!(
        visits
            .iter()
            .all(|visits| visits.load(Ordering::SeqCst) == 1)
    );
}

#[test]
fn idle_worker_starts_later_group_before_slow_first_group_finishes() {
    let jobs: Vec<_> = (0..9).collect();
    let groups: Vec<_> = jobs.iter().map(|job| vec![job]).collect();
    let slow = Gate::default();
    let (started, observed) = mpsc::channel();
    std::thread::scope(|scope| {
        let scheduler = scope.spawn(|| {
            run_payload_groups(&groups, |&job| {
                if job == 0 || job == 4 {
                    started.send(job).unwrap();
                }
                if job == 0 {
                    slow.wait();
                }
                Ok(())
            })
        });
        let first = observed.recv_timeout(WATCHDOG);
        let second = observed.recv_timeout(WATCHDOG);
        slow.release();
        scheduler.join().unwrap().unwrap();
        let mut started = vec![first.unwrap(), second.unwrap()];
        started.sort_unstable();
        assert_eq!(started, [0, 4]);
    });
}

fn assert_failure_stops_new_groups_and_joins_claimed_groups(panics: bool) {
    let jobs: Vec<_> = (0..9).flat_map(|file| [(file, 0), (file, 1)]).collect();
    let groups: Vec<_> = jobs.chunks(2).map(|jobs| jobs.iter().collect()).collect();
    let visits: Vec<_> = (0..9).map(|_| AtomicUsize::new(0)).collect();
    let initial = Barrier::new(4);
    let finish = Gate::default();
    let (exited, observed_exit) = mpsc::channel();
    let (finished, observed_finish) = mpsc::channel();
    std::thread::scope(|scope| {
        let scheduler = scope.spawn(|| {
            let result = run_payload_groups(&groups, |&(file, step)| {
                assert_eq!(visits[file].fetch_add(1, Ordering::SeqCst), step);
                if step == 0 && file < 4 {
                    initial.wait();
                    if file == 1 {
                        WORKER_EXIT.with(|notice| {
                            *notice.borrow_mut() = Some(ExitNotice(exited.clone()));
                        });
                        if panics {
                            panic!("injected payload preparation panic");
                        }
                        return Err(Error::new("SOURCE_CHANGED", "injected payload failure"));
                    }
                    finish.wait();
                }
                Ok(())
            });
            finished.send(()).unwrap();
            result
        });
        let failure_observed = observed_exit.recv_timeout(WATCHDOG);
        let waiting_for_claimed =
            matches!(observed_finish.try_recv(), Err(mpsc::TryRecvError::Empty));
        finish.release();
        let error = scheduler.join().unwrap().unwrap_err();
        failure_observed.unwrap();
        assert!(
            waiting_for_claimed,
            "returned before claimed files finished"
        );
        assert_eq!(
            error.code(),
            if panics {
                "INTERNAL_ERROR"
            } else {
                "SOURCE_CHANGED"
            }
        );
    });
    assert_eq!(visits[1].load(Ordering::SeqCst), 1);
    for file in [0, 2, 3] {
        assert_eq!(visits[file].load(Ordering::SeqCst), 2);
    }
    assert!(
        visits[4..]
            .iter()
            .all(|visits| visits.load(Ordering::SeqCst) == 0)
    );
}

#[test]
fn failure_closes_claims_but_finishes_already_claimed_file_versions() {
    assert_failure_stops_new_groups_and_joins_claimed_groups(false);
}

#[test]
fn panic_closes_claims_before_join_and_finishes_already_claimed_file_versions() {
    assert_failure_stops_new_groups_and_joins_claimed_groups(true);
}

#[test]
fn lower_group_error_wins_over_already_observed_higher_error_or_panic() {
    for higher_panics in [false, true] {
        let jobs: Vec<_> = (0..8).collect();
        let groups: Vec<_> = jobs.iter().map(|job| vec![job]).collect();
        let initial = Barrier::new(4);
        let finish = Gate::default();
        let (exited, observed_exit) = mpsc::channel();
        std::thread::scope(|scope| {
            let scheduler = scope.spawn(|| {
                run_payload_groups(&groups, |&file| {
                    assert!(file < 4, "new group started after a known failure");
                    initial.wait();
                    if file == 1 {
                        WORKER_EXIT.with(|notice| {
                            *notice.borrow_mut() = Some(ExitNotice(exited.clone()));
                        });
                        if higher_panics {
                            panic!("injected higher-index panic");
                        }
                        return Err(Error::new("OBJECT_COLLISION", "higher-index failure"));
                    }
                    finish.wait();
                    if file == 0 {
                        Err(Error::new("SOURCE_CHANGED", "lower-index failure").retryable())
                    } else {
                        Ok(())
                    }
                })
            });
            let higher_finished = observed_exit.recv_timeout(WATCHDOG);
            finish.release();
            let error = scheduler.join().unwrap().unwrap_err();
            higher_finished.unwrap();
            assert_eq!(error.code(), "SOURCE_CHANGED");
            assert_eq!(error.message, "lower-index failure");
            assert!(error.retryable);
        });
    }
}
