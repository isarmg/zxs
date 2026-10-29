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
                        return Err(Error::new(
                            storage_contract::ErrorCode::SourceChanged,
                            "injected payload failure",
                        ));
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
        let outcome = scheduler.join();
        failure_observed.unwrap();
        assert!(
            waiting_for_claimed,
            "returned before claimed files finished"
        );
        if panics {
            assert!(
                outcome.is_err(),
                "internal worker panic must reach its caller"
            );
        } else {
            assert_eq!(
                outcome.unwrap().unwrap_err().code(),
                storage_contract::ErrorCode::SourceChanged
            );
        }
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
fn lower_group_error_wins_over_higher_error_but_never_hides_a_panic() {
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
                        return Err(Error::new(
                            storage_contract::ErrorCode::ObjectCollision,
                            "higher-index failure",
                        ));
                    }
                    finish.wait();
                    if file == 0 {
                        Err(Error::new(
                            storage_contract::ErrorCode::SourceChanged,
                            "lower-index failure",
                        )
                        .retryable())
                    } else {
                        Ok(())
                    }
                })
            });
            let higher_finished = observed_exit.recv_timeout(WATCHDOG);
            finish.release();
            let outcome = scheduler.join();
            higher_finished.unwrap();
            if higher_panics {
                assert!(
                    outcome.is_err(),
                    "ordinary errors must not hide worker panics"
                );
            } else {
                let error = outcome.unwrap().unwrap_err();
                assert_eq!(error.code(), storage_contract::ErrorCode::SourceChanged);
                assert_eq!(error.message(), "lower-index failure");
                assert!(error.is_retryable());
            }
        });
    }
}

#[test]
fn routes_outside_fan_out_envelope_keep_serial_copy_and_replay_capability() {
    for copies in [3, 9] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config =
            Config::development((0..copies).map(|i| temp.path().join(format!("disk-{i}"))));
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("resource-envelope").unwrap(),
                "P01",
                copies,
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        let bytes = b"independent copies beyond the optimized route envelope";
        engine
            .stage_bytes(&workspace.workspace_id, "body.bin", bytes)
            .unwrap();
        let request = CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "bounded fan-out or original serial copy".into(),
            request_digest: None,
        };
        let result = engine.commit_workspace(request.clone()).unwrap();
        let report = engine.last_publication_stages().unwrap();
        let payload = report
            .stages
            .iter()
            .find(|stage| stage.stage == StageKind::PayloadCopy)
            .unwrap();
        assert_eq!(
            payload.io.logical_bytes_completed,
            bytes.len() as u64 * copies as u64
        );
        assert_eq!(
            payload.io.payload_source_bytes_completed,
            bytes.len() as u64 * if copies == 3 { 1 } else { copies as u64 }
        );
        let version = engine.state.versions.values().next().unwrap();
        for disk in &engine.config.disks {
            let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
            engine
                .disk_resources
                .get(disk)
                .unwrap()
                .verify_payload(&directory, version)
                .unwrap();
        }
        drop(engine);
        let mut reopened = Engine::open(&root).unwrap();
        assert_eq!(reopened.commit_workspace(request).unwrap(), result);
    }
}
