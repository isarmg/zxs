use super::*;
use crate::{Config, Engine};
use std::sync::{
    Barrier, Condvar,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

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
    // Observing worker exit proves the runner handled the task's outcome,
    // rather than racing a signal emitted inside the work callback.
    static WORKER_EXIT: std::cell::RefCell<Option<ExitNotice>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Real held disk handles; `count` tasks reuse them round-robin under
/// distinct task ids `t0`, `t1`, ... The runner only needs per-task identity.
struct Tasks {
    _temp: tempfile::TempDir,
    engine: Engine,
    disks: Vec<DiskConfig>,
}
impl Tasks {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]);
        let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let disks = (0..count)
            .map(|index| {
                let mut disk = engine.config.disks[index % 3].clone();
                disk.disk_id = format!("t{index}");
                disk
            })
            .collect();
        Self {
            _temp: temp,
            engine,
            disks,
        }
    }
    fn stages(&self) -> Vec<MemberStage<'_>> {
        self.disks
            .iter()
            .enumerate()
            .map(|(index, disk)| MemberStage {
                disk,
                handle: self
                    .engine
                    .disk_resources
                    .get(&self.engine.config.disks[index % 3])
                    .unwrap(),
                journal: None,
            })
            .collect()
    }
}

fn index(task: &MemberStage<'_>) -> usize {
    task.disk.disk_id[1..].parse().unwrap()
}

fn done<T>(value: T) -> TaskOutcome<T> {
    TaskOutcome {
        value: Ok(value),
        io: StageIo {
            records_completed: 1,
            logical_bytes_completed: 10,
            ..StageIo::default()
        },
    }
}

fn failed<T>(task: &MemberStage<'_>) -> TaskOutcome<T> {
    TaskOutcome {
        value: Err(Error::new(
            ErrorCode::InjectedFailure,
            format!("task {} failed", task.disk.disk_id),
        )),
        io: StageIo::default(),
    }
}

#[test]
fn results_keep_member_order_when_completion_order_differs() {
    let tasks = Tasks::new(8);
    let later_done = AtomicUsize::new(0);
    let later = Gate::default();
    let run = run_member_stage(StageKind::Accept, Duration::ZERO, tasks.stages(), |task| {
        let index = index(task);
        if index == 0 {
            // Member 0 finishes last: every other member has completed first.
            later.wait();
        } else if later_done.fetch_add(1, Ordering::SeqCst) + 1 == 7 {
            later.release();
        }
        done(index * 10)
    })
    .ok()
    .unwrap();
    assert_eq!(
        run.values,
        (0..8).map(|index| index * 10).collect::<Vec<_>>()
    );
    assert_eq!(run.timing.stage, StageKind::Accept);
    assert_eq!(run.timing.workers, 8);
    assert!(run.timing.parallel);
    assert_eq!(run.timing.members_completed, 8);
    assert_eq!(run.timing.io.records_completed, 8);
    assert_eq!(run.timing.io.logical_bytes_completed, 80);
}

#[test]
fn workers_are_bounded_and_every_task_runs_once() {
    let tasks = Tasks::new(3 * MAX_STAGE_WORKERS);
    let active = AtomicUsize::new(0);
    let maximum = AtomicUsize::new(0);
    let runs: Vec<_> = (0..3 * MAX_STAGE_WORKERS)
        .map(|_| AtomicUsize::new(0))
        .collect();
    // A full-width barrier proves MAX_STAGE_WORKERS tasks really overlap; any
    // extra worker would be observed by the active counter.
    let barrier = Barrier::new(MAX_STAGE_WORKERS);
    let run = run_member_stage(
        StageKind::Finalize,
        Duration::ZERO,
        tasks.stages(),
        |task| {
            runs[index(task)].fetch_add(1, Ordering::SeqCst);
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(now, Ordering::SeqCst);
            barrier.wait();
            active.fetch_sub(1, Ordering::SeqCst);
            barrier.wait();
            done(())
        },
    )
    .ok()
    .unwrap();
    assert_eq!(maximum.load(Ordering::SeqCst), MAX_STAGE_WORKERS);
    assert_eq!(run.timing.workers, MAX_STAGE_WORKERS);
    assert!(runs.iter().all(|count| count.load(Ordering::SeqCst) == 1));
    assert_eq!(run.values.len(), 3 * MAX_STAGE_WORKERS);
}

#[test]
fn a_single_member_runs_on_the_calling_thread() {
    let tasks = Tasks::new(1);
    let caller = std::thread::current().id();
    let run = run_member_stage(StageKind::Prepare, Duration::ZERO, tasks.stages(), |_| {
        done(std::thread::current().id())
    })
    .ok()
    .unwrap();
    assert_eq!(run.values, vec![caller]);
    assert_eq!(run.timing.workers, 1);
    assert!(!run.timing.parallel);
}

#[test]
fn a_reduced_verification_budget_runs_all_members_serially() {
    let tasks = Tasks::new(3);
    let caller = std::thread::current().id();
    let run = run_member_stage_with_limit(
        StageKind::CommitProofs,
        Duration::ZERO,
        tasks.stages(),
        1,
        |_| done(std::thread::current().id()),
    )
    .ok()
    .unwrap();
    assert_eq!(run.values, vec![caller; 3]);
    assert_eq!(run.timing.workers, 1);
    assert_eq!(run.timing.members_completed, 3);
    assert!(!run.timing.parallel);
}

#[test]
fn failed_stage_retains_io_source_timing_and_completed_work() {
    use std::error::Error as _;

    let tasks = Tasks::new(2);
    let offset = Duration::from_millis(17);
    let result =
        run_member_stage_with_limit(StageKind::Prepare, offset, tasks.stages(), 1, |task| {
            if index(task) == 0 {
                done(())
            } else {
                TaskOutcome {
                    value: Err(Error::from(std::io::Error::from_raw_os_error(28)).retryable()),
                    io: StageIo {
                        objects_completed: 1,
                        logical_bytes_completed: 7,
                        ..StageIo::default()
                    },
                }
            }
        });
    let (failure, timing) = *result.err().unwrap();
    assert_eq!(failure.completed, ["t0"]);
    assert_eq!(failure.failed, [("t1".to_owned(), ErrorCode::NoSpace)]);
    assert_eq!(timing.stage, StageKind::Prepare);
    assert_eq!(timing.started_at, offset);
    assert_eq!(timing.workers, 1);
    assert_eq!(timing.members_completed, 1);
    assert_eq!(timing.io.objects_completed, 1);
    assert_eq!(timing.io.records_completed, 1);
    assert_eq!(timing.io.logical_bytes_completed, 17);
    let (primary, summary) = failure.into_parts();
    assert_eq!(primary.code(), ErrorCode::NoSpace);
    assert!(primary.is_retryable());
    assert_eq!(
        primary
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    assert_eq!(
        summary,
        "stage prepare; completed [t0]; failed [t1=NO_SPACE]"
    );
}

#[test]
fn started_tasks_finish_and_the_lowest_failed_index_wins() {
    let tasks = Tasks::new(8);
    let finished = AtomicUsize::new(0);
    // Every member is claimed before any fails, so all eight are started.
    let claimed = Barrier::new(8);
    let fifth_failed = Gate::default();
    let (exit, exited) = mpsc::channel();
    let result = std::thread::scope(|scope| {
        let gate = &fifth_failed;
        scope.spawn(move || {
            exited.recv_timeout(WATCHDOG).unwrap();
            gate.release();
        });
        run_member_stage(StageKind::Accept, Duration::ZERO, tasks.stages(), |task| {
            claimed.wait();
            let index = index(task);
            if index == 5 {
                WORKER_EXIT.with(|notice| *notice.borrow_mut() = Some(ExitNotice(exit.clone())));
                finished.fetch_add(1, Ordering::SeqCst);
                return failed(task);
            }
            // Every other member was already claimed and keeps running after
            // member 5 failed; member 2 fails only after that.
            fifth_failed.wait();
            finished.fetch_add(1, Ordering::SeqCst);
            if index == 2 {
                failed(task)
            } else {
                done(index)
            }
        })
    });
    let (failure, timing) = *result.err().unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 8);
    assert_eq!(failure.stage, StageKind::Accept);
    assert_eq!(failure.primary.message(), "task t2 failed");
    assert_eq!(
        failure.failed,
        vec![
            ("t2".to_owned(), ErrorCode::InjectedFailure),
            ("t5".to_owned(), ErrorCode::InjectedFailure),
        ]
    );
    assert_eq!(failure.completed, ["t0", "t1", "t3", "t4", "t6", "t7"]);
    assert_eq!(timing.members_completed, 6);
    assert_eq!(timing.io.records_completed, 6);
    let (primary, summary) = failure.into_parts();
    assert_eq!(primary.code(), ErrorCode::InjectedFailure);
    assert_eq!(primary.message(), "task t2 failed");
    assert_eq!(
        summary,
        "stage accept; completed [t0,t1,t3,t4,t6,t7]; failed [t2=INJECTED_FAILURE,t5=INJECTED_FAILURE]"
    );
}

#[test]
fn a_failure_stops_claiming_unstarted_members() {
    let tasks = Tasks::new(MAX_STAGE_WORKERS + 2);
    let started = AtomicUsize::new(0);
    // The first MAX_STAGE_WORKERS members are claimed before member 0 fails.
    let claimed = Barrier::new(MAX_STAGE_WORKERS);
    let first_failed = Gate::default();
    let (exit, exited) = mpsc::channel();
    let result = std::thread::scope(|scope| {
        let gate = &first_failed;
        scope.spawn(move || {
            exited.recv_timeout(WATCHDOG).unwrap();
            gate.release();
        });
        run_member_stage(
            StageKind::Publication,
            Duration::ZERO,
            tasks.stages(),
            |task| {
                started.fetch_add(1, Ordering::SeqCst);
                claimed.wait();
                if index(task) == 0 {
                    WORKER_EXIT
                        .with(|notice| *notice.borrow_mut() = Some(ExitNotice(exit.clone())));
                    return failed(task);
                }
                // The failing worker has exited, so admission is closed before
                // any claimed member finishes and could claim another.
                first_failed.wait();
                done(())
            },
        )
    });
    let (failure, _) = *result.err().unwrap();
    assert_eq!(started.load(Ordering::SeqCst), MAX_STAGE_WORKERS);
    assert_eq!(failure.completed.len(), MAX_STAGE_WORKERS - 1);
    assert_eq!(failure.failed.len(), 1);
    assert!(
        !failure
            .completed
            .iter()
            .any(|id| *id == format!("t{MAX_STAGE_WORKERS}")
                || *id == format!("t{}", MAX_STAGE_WORKERS + 1))
    );
}

#[test]
fn a_worker_panic_is_resumed_after_every_started_task_finished() {
    let tasks = Tasks::new(4);
    let finished = AtomicUsize::new(0);
    let claimed = Barrier::new(4);
    let panicked = Gate::default();
    let (exit, exited) = mpsc::channel();
    let caught = std::thread::scope(|scope| {
        let gate = &panicked;
        scope.spawn(move || {
            exited.recv_timeout(WATCHDOG).unwrap();
            gate.release();
        });
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_member_stage(StageKind::Seals, Duration::ZERO, tasks.stages(), |task| {
                claimed.wait();
                if index(task) == 1 {
                    WORKER_EXIT
                        .with(|notice| *notice.borrow_mut() = Some(ExitNotice(exit.clone())));
                    panic!("member task panicked");
                }
                panicked.wait();
                finished.fetch_add(1, Ordering::SeqCst);
                done(())
            })
            .ok()
        }))
    });
    let payload = caught.err().unwrap();
    assert_eq!(
        payload.downcast_ref::<&str>().copied(),
        Some("member task panicked")
    );
    assert_eq!(finished.load(Ordering::SeqCst), 3);
}
