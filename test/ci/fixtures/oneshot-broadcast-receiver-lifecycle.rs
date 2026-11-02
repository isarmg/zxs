// Public oneshot-broadcast endpoint and owned-future lifecycle regressions.
use futures_intrusive::channel::ChannelSendError;
use futures_intrusive::channel::shared::oneshot_broadcast_channel;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn counted_waker() -> (Arc<WakeCount>, Waker) {
    let count = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&count));
    (count, waker)
}

fn poll_once<F: Future>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

#[test]
fn dropping_one_receiver_clone_keeps_survivor_open() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    let survivor = receiver.clone();
    drop(receiver);

    assert_eq!(sender.send(41), Ok(()));
    let (_, waker) = counted_waker();
    let mut receive = Box::pin(survivor.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(Some(41)));
}

#[test]
fn last_receiver_drop_closes_with_owned_pending_receive() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    let (wakes, waker) = counted_waker();
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Pending);

    drop(receiver);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(sender.send(43), Err(ChannelSendError(43)));
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(None));
}

#[test]
fn sender_drop_wakes_pending_receive_as_closed() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    let (wakes, waker) = counted_waker();
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Pending);

    drop(sender);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(None));
}

#[test]
fn broadcast_value_survives_sender_drop_and_fresh_receives() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    assert_eq!(sender.send(47), Ok(()));
    assert_eq!(sender.send(53), Err(ChannelSendError(53)));
    drop(sender);

    let (_, waker) = counted_waker();
    let mut first = Box::pin(receiver.receive());
    let mut second = Box::pin(receiver.receive());
    assert_eq!(poll_once(first.as_mut(), &waker), Poll::Ready(Some(47)));
    assert_eq!(poll_once(second.as_mut(), &waker), Poll::Ready(Some(47)));
}

#[test]
fn canceled_pending_receive_does_not_close_endpoint() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    let (wakes, waker) = counted_waker();
    let mut canceled = Box::pin(receiver.receive());
    assert_eq!(poll_once(canceled.as_mut(), &waker), Poll::Pending);
    drop(canceled);

    assert_eq!(sender.send(59), Ok(()));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(Some(59)));
}

#[test]
fn owned_receive_reads_fulfilled_value_after_endpoint_drop() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(sender.send(61), Ok(()));
    drop(receiver);
    drop(sender);

    let (_, waker) = counted_waker();
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(Some(61)));
}

#[test]
fn parallel_receiver_clone_drops_keep_original_open() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(4);
        for _ in 0..4 {
            workers.push(scope.spawn(|| {
                for _ in 0..16 {
                    drop(receiver.clone());
                }
            }));
        }
        for worker in workers {
            worker
                .join()
                .expect("invariant: bounded clone/drop worker completes");
        }
    });

    assert_eq!(sender.send(71), Ok(()));
    let (_, waker) = counted_waker();
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(Some(71)));
}

#[test]
#[should_panic(expected = "polled ChannelReceiveFuture after completion")]
fn completed_receive_future_repolls_panic() {
    let (sender, receiver) = oneshot_broadcast_channel::<u32>();
    assert_eq!(sender.send(67), Ok(()));
    let (_, waker) = counted_waker();
    let mut receive = Box::pin(receiver.receive());
    assert_eq!(poll_once(receive.as_mut(), &waker), Poll::Ready(Some(67)));
    let _ = poll_once(receive.as_mut(), &waker);
}
