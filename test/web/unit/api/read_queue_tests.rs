use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Wake;

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn poll(admission: &mut Admission) -> Poll<Result<Permit, AdmissionError>> {
    Pin::new(admission).poll(&mut Context::from_waker(Waker::noop()))
}

fn granted(admission: &mut Admission) -> Permit {
    match poll(admission) {
        Poll::Ready(Ok(permit)) => permit,
        _ => panic!("expected an admitted read"),
    }
}

fn pending(admission: &mut Admission, wakes: &Arc<Wakes>) {
    let waker = Waker::from(Arc::clone(wakes));
    assert!(
        Pin::new(admission)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
}

#[test]
fn all_real_refresh_reads_share_the_queue() {
    for path in [
        "/api/admin/storage/status",
        "/api/admin/storage/discovered-disks",
        "/api/admin/disks",
        "/api/admin/disks/addition",
        "/api/admin/storage/conversion/status",
        "/api/admin/disks/exit",
        "/api/admin/disks/readmission",
        "/api/admin/disks/registrations",
        "/api/admin/disks/P01/details",
    ] {
        assert!(managed("GET", path), "{path}");
        assert!(managed("GET", &format!("{path}?observation=1")));
        assert!(!managed("POST", path));
    }
    for path in [
        "/api/capabilities",
        "/api/workspaces/capacity",
        "/api/setup/status",
    ] {
        assert!(managed("GET", path));
    }
}

#[test]
fn mutations_and_unrelated_reads_are_not_queued_or_replayed() {
    for method in ["POST", "PUT", "DELETE", "PATCH", "HEAD"] {
        assert!(!managed(method, "/api/admin/disks/readmission"));
    }
    for path in [
        "/api/admin/disks-other",
        "/api/admin/storage-other/status",
        "/api/auth/me",
        "/api/management/status",
        "/api/file-manager/disks",
        "/api/system/status",
    ] {
        assert!(!managed("GET", path));
    }
}

#[test]
fn seven_refresh_requests_run_one_at_a_time_in_fifo_order() {
    let queue = Queue::default();
    let mut requests = (0..7).map(|_| queue.acquire().unwrap()).collect::<Vec<_>>();
    let mut active = None;
    for current in 0..7 {
        drop(active.take());
        active = Some(granted(&mut requests[current]));
        for later in requests.iter_mut().skip(current + 1).rev() {
            assert!(poll(later).is_pending());
        }
    }
    drop(active);
    assert!(!queue.0.borrow().active);
    assert!(queue.0.borrow().waiting.is_empty());
}

#[test]
fn polling_a_later_request_first_does_not_skip_fifo() {
    let queue = Queue::default();
    let mut first = queue.acquire().unwrap();
    let mut later = queue.acquire().unwrap();
    assert!(poll(&mut later).is_pending());
    let permit = granted(&mut first);
    assert!(poll(&mut later).is_pending());
    drop(permit);
    drop(granted(&mut later));
}

#[test]
fn body_consumption_or_failure_must_drop_the_held_permit_before_next_read() {
    let queue = Queue::default();
    let mut first = queue.acquire().unwrap();
    let response_permit = granted(&mut first);
    drop(first);
    let mut next = queue.acquire().unwrap();
    assert!(
        poll(&mut next).is_pending(),
        "fetch headers do not release occupancy"
    );
    assert!(
        poll(&mut next).is_pending(),
        "waiting on a body keeps occupancy"
    );
    drop(response_permit);
    drop(granted(&mut next));
}

#[test]
fn cancelling_a_middle_waiter_never_releases_an_active_request() {
    let queue = Queue::default();
    let active = granted(&mut queue.acquire().unwrap());
    let mut first = queue.acquire().unwrap();
    let cancelled = queue.acquire().unwrap();
    let mut last = queue.acquire().unwrap();
    drop(cancelled);
    assert!(poll(&mut first).is_pending());
    assert!(poll(&mut last).is_pending());
    drop(active);
    let first_permit = granted(&mut first);
    assert!(poll(&mut last).is_pending());
    drop(first_permit);
    drop(granted(&mut last));
}

#[test]
fn cancelling_the_first_unpolled_waiter_wakes_the_next() {
    let queue = Queue::default();
    let first = queue.acquire().unwrap();
    let mut next = queue.acquire().unwrap();
    let wakes = Arc::new(Wakes::default());
    pending(&mut next, &wakes);
    drop(first);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    drop(granted(&mut next));
}

#[test]
fn dropping_all_cancelled_queue_entries_leaves_no_later_admission() {
    let queue = Queue::default();
    let active = granted(&mut queue.acquire().unwrap());
    let pending = (0..7).map(|_| queue.acquire().unwrap()).collect::<Vec<_>>();
    drop(pending);
    assert!(queue.0.borrow().waiting.is_empty());
    assert!(queue.0.borrow().active);
    drop(active);
    assert!(!queue.0.borrow().active);
    drop(granted(&mut queue.acquire().unwrap()));
}

#[test]
fn queue_full_is_explicit_and_does_not_drop_or_retry_existing_requests() {
    let queue = Queue::default();
    let active = granted(&mut queue.acquire().unwrap());
    let waiting = (0..MAX_WAITING)
        .map(|_| queue.acquire().unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(queue.acquire(), Err(AdmissionError::Full)));
    assert_eq!(queue.0.borrow().waiting.len(), MAX_WAITING);
    drop(waiting);
    assert!(queue.acquire().is_ok());
    drop(active);
}

#[test]
fn session_invalidation_wakes_every_old_waiter_and_admits_none_of_them() {
    let queue = Queue::default();
    let active = granted(&mut queue.acquire().unwrap());
    let mut waiting = (0..7).map(|_| queue.acquire().unwrap()).collect::<Vec<_>>();
    let wakes = Arc::new(Wakes::default());
    for request in &mut waiting {
        pending(request, &wakes);
    }
    queue.invalidate();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 7);
    for request in &mut waiting {
        assert!(matches!(
            poll(request),
            Poll::Ready(Err(AdmissionError::Invalidated))
        ));
    }
    assert!(queue.0.borrow().active);
    drop(active);
}

#[test]
fn old_waiter_drop_cannot_release_new_session_occupancy() {
    let queue = Queue::default();
    let old = queue.acquire().unwrap();
    queue.invalidate();
    let new_permit = granted(&mut queue.acquire().unwrap());
    let mut next = queue.acquire().unwrap();
    drop(old);
    assert!(poll(&mut next).is_pending());
    drop(new_permit);
    drop(granted(&mut next));
}

#[test]
fn active_old_session_keeps_its_slot_until_its_response_finishes() {
    let queue = Queue::default();
    let old_permit = granted(&mut queue.acquire().unwrap());
    queue.invalidate();
    let mut current = queue.acquire().unwrap();
    assert!(poll(&mut current).is_pending());
    drop(old_permit);
    let current_permit = granted(&mut current);
    let mut next = queue.acquire().unwrap();
    assert!(poll(&mut next).is_pending());
    drop(current_permit);
    drop(granted(&mut next));
}

#[test]
fn repeated_invalidation_does_not_resurrect_old_admissions() {
    let queue = Queue::default();
    let mut old = queue.acquire().unwrap();
    queue.invalidate();
    queue.invalidate();
    assert!(matches!(
        poll(&mut old),
        Poll::Ready(Err(AdmissionError::Invalidated))
    ));
    drop(granted(&mut queue.acquire().unwrap()));
}

#[test]
fn latest_waker_is_not_lost_when_the_same_waiter_is_repolled() {
    let queue = Queue::default();
    let active = granted(&mut queue.acquire().unwrap());
    let mut next = queue.acquire().unwrap();
    let old = Arc::new(Wakes::default());
    let latest = Arc::new(Wakes::default());
    pending(&mut next, &old);
    pending(&mut next, &latest);
    drop(active);
    assert_eq!(old.0.load(Ordering::Relaxed), 0);
    assert_eq!(latest.0.load(Ordering::Relaxed), 1);
    drop(granted(&mut next));
}

#[test]
fn failed_read_releases_occupancy_without_enqueueing_a_retry() {
    fn failure(queue: &Queue) -> Result<(), ()> {
        let _request = granted(&mut queue.acquire().unwrap());
        Err(())
    }
    let queue = Queue::default();
    assert_eq!(failure(&queue), Err(()));
    assert!(queue.0.borrow().waiting.is_empty());
    assert!(!queue.0.borrow().active);
    drop(granted(&mut queue.acquire().unwrap()));
}
