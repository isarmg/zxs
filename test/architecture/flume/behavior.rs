//! Channel contracts the SQLite connection worker relies on (project-authored).
//!
//! The worker owns a bounded command channel fed by `send_async`, replies on
//! bounded result channels, drains the command receiver by iteration after
//! every sender is gone and treats a failed send as a vanished requester.
//! The retained upstream async suites need runtimes that are not part of the
//! local sources, so cancellation, wake hand-off, disconnect and ownership
//! return are driven here by explicit polling with counting wakers.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use flume::{RecvError, SendError, TryRecvError, TrySendError};

struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn waker() -> (Waker, Arc<Wakes>) {
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    (Waker::from(Arc::clone(&wakes)), wakes)
}

fn woken(wakes: &Wakes) -> usize {
    wakes.0.load(Ordering::SeqCst)
}

fn poll<F: Future + Unpin>(future: &mut F, waker: &Waker) -> Poll<F::Output> {
    Pin::new(future).poll(&mut Context::from_waker(waker))
}

/// Counts drops so ownership is observed, not inferred.
struct Owned(Arc<AtomicUsize>, u32);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn full_bounded_send_async_waits_and_is_woken_by_the_receiver() {
    let (tx, rx) = flume::bounded(1);
    tx.send(1).unwrap();
    assert!(matches!(tx.try_send(9), Err(TrySendError::Full(9))));
    let (waker, wakes) = waker();
    let mut send = tx.send_async(2);
    assert!(poll(&mut send, &waker).is_pending());
    assert_eq!(woken(&wakes), 0);
    assert_eq!(rx.recv(), Ok(1));
    assert_eq!(woken(&wakes), 1, "making room wakes the queued sender");
    assert!(matches!(poll(&mut send, &waker), Poll::Ready(Ok(()))));
    assert_eq!(rx.recv(), Ok(2));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn cancelled_send_async_never_delivers_and_drops_its_item_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = flume::bounded(1);
    tx.send(Owned(Arc::clone(&drops), 1)).unwrap();
    let (waker, _) = waker();
    let mut send = tx.send_async(Owned(Arc::clone(&drops), 2));
    assert!(poll(&mut send, &waker).is_pending());
    drop(send);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the cancelled item is dropped by its future"
    );
    assert_eq!(rx.recv().unwrap().1, 1);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn queued_send_is_delivered_exactly_once_when_its_future_is_dropped_after_room_is_made() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = flume::bounded(1);
    tx.send(Owned(Arc::clone(&drops), 1)).unwrap();
    let (waker, wakes) = waker();
    let mut send = tx.send_async(Owned(Arc::clone(&drops), 2));
    assert!(poll(&mut send, &waker).is_pending());
    assert_eq!(rx.recv().unwrap().1, 1);
    assert_eq!(woken(&wakes), 1);
    drop(send);
    assert_eq!(
        rx.recv().unwrap().1,
        2,
        "an item taken from the waiting sender stays sent"
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn cancelled_woken_recv_async_passes_the_wake_to_the_next_receiver() {
    let (tx, rx) = flume::bounded::<u32>(4);
    let (first_waker, first_wakes) = waker();
    let (second_waker, second_wakes) = waker();
    let mut first = rx.recv_async();
    let mut second = rx.recv_async();
    assert!(poll(&mut first, &first_waker).is_pending());
    assert!(poll(&mut second, &second_waker).is_pending());
    tx.send(7).unwrap();
    assert_eq!((woken(&first_wakes), woken(&second_wakes)), (1, 0));
    drop(first);
    assert_eq!(
        woken(&second_wakes),
        1,
        "the dropped receiver's wake is not lost"
    );
    assert!(matches!(
        poll(&mut second, &second_waker),
        Poll::Ready(Ok(7))
    ));
}

#[test]
fn cancelled_pending_recv_async_leaves_messages_for_later_receivers() {
    let (tx, rx) = flume::bounded::<u32>(2);
    let (waker, _) = waker();
    let mut pending = rx.recv_async();
    assert!(poll(&mut pending, &waker).is_pending());
    drop(pending);
    tx.send(3).unwrap();
    assert_eq!(rx.try_recv(), Ok(3));
}

#[test]
fn last_sender_drop_drains_queued_items_then_reports_disconnect() {
    let (tx, rx) = flume::bounded(4);
    let extra = tx.clone();
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    drop(tx);
    assert!(
        !rx.is_disconnected(),
        "a remaining clone keeps the channel connected"
    );
    extra.send(3).unwrap();
    drop(extra);
    assert!(rx.is_disconnected());
    assert_eq!(rx.iter().collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));
}

#[test]
fn last_sender_drop_wakes_a_pending_recv_async_with_disconnect() {
    let (tx, rx) = flume::bounded::<u32>(1);
    let (waker, wakes) = waker();
    let mut pending = rx.recv_async();
    assert!(poll(&mut pending, &waker).is_pending());
    drop(tx);
    assert_eq!(woken(&wakes), 1);
    assert!(matches!(
        poll(&mut pending, &waker),
        Poll::Ready(Err(RecvError::Disconnected))
    ));
}

#[test]
fn receiver_drop_returns_items_to_senders_and_the_last_sender_releases_the_queue() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = flume::bounded(1);
    tx.send(Owned(Arc::clone(&drops), 1)).unwrap();
    let (waker, wakes) = waker();
    let mut send = tx.send_async(Owned(Arc::clone(&drops), 2));
    assert!(poll(&mut send, &waker).is_pending());
    drop(rx);
    assert_eq!(woken(&wakes), 1, "receiver loss wakes the queued sender");
    match poll(&mut send, &waker) {
        Poll::Ready(Err(SendError(item))) => assert_eq!(item.1, 2),
        _ => panic!("the waiting item must come back to its sender"),
    }
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "only the returned item 2 is gone"
    );
    match tx.send(Owned(Arc::clone(&drops), 3)) {
        Err(SendError(item)) => assert_eq!(item.1, 3),
        Ok(()) => panic!("a disconnected channel accepted an item"),
    }
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    // The worker relies on this: queued replies stay owned by the channel
    // until the connection owner drops its sender.
    drop(send);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(tx);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        3,
        "the last sender releases queued item 1"
    );
}

#[test]
fn zero_capacity_hands_over_only_to_a_receiver() {
    let (tx, rx) = flume::bounded(0);
    assert!(matches!(tx.try_send(1), Err(TrySendError::Full(1))));
    let (waker, wakes) = waker();
    let mut send = tx.send_async(2);
    assert!(poll(&mut send, &waker).is_pending());
    assert_eq!(rx.len(), 0, "a rendezvous channel queues nothing");
    assert_eq!(rx.try_recv(), Ok(2));
    assert_eq!(woken(&wakes), 1);
    assert!(matches!(poll(&mut send, &waker), Poll::Ready(Ok(()))));
}

#[test]
fn blocking_iteration_ends_after_a_producer_thread_finishes() {
    let (tx, rx) = flume::bounded(2);
    let producer = thread::spawn(move || {
        for value in 0..64 {
            tx.send(value).unwrap();
        }
    });
    let received = rx.iter().collect::<Vec<_>>();
    producer.join().unwrap();
    assert_eq!(received, (0..64).collect::<Vec<_>>());
}
