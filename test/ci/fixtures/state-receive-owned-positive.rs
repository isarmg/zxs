#[path = "state-receive-owned-support.rs"]
mod support;

use futures_intrusive::channel::shared::{
    generic_state_broadcast_channel, state_broadcast_channel,
};
use futures_intrusive::channel::{GenericStateBroadcastChannel, StateId};
use std::future::Future;
use std::task::{Context, Poll, Waker};
use support::{SyncOnlyRaw, raw_drop_count};

fn require_sync<T: Sync>() {}
fn require_send<T: Send>(value: T) -> T {
    value
}

// Every caller publishes its value before polling, so no wait node is queued.
fn published<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("invariant: the fixture published its state before polling"),
    }
}

fn native_owned_is_send() {
    let (sender, receiver) = state_broadcast_channel::<u32>();
    sender
        .send(11)
        .expect("invariant: fresh channel accepts its first state");
    let future = require_send(receiver.receive(StateId::new()));
    drop(sender);
    drop(receiver);

    let output = std::thread::spawn(move || published(future))
        .join()
        .expect("invariant: native owned receive control completes");
    assert_eq!(output.map(|(_, value)| value), Some(11));
}

fn sync_only_owned_stays_local() {
    require_sync::<SyncOnlyRaw>();
    let before = raw_drop_count();
    let (sender, receiver) = generic_state_broadcast_channel::<SyncOnlyRaw, u32>();
    sender
        .send(22)
        .expect("invariant: fresh channel accepts its first state");
    let future = receiver.receive(StateId::new());
    drop(sender);
    drop(receiver);
    assert_eq!(raw_drop_count(), before);

    assert_eq!(published(future).map(|(_, value)| value), Some(22));
    assert_eq!(raw_drop_count(), before + 1);
}

fn borrowed_sync_owner_needs_no_send() {
    let channel = GenericStateBroadcastChannel::<SyncOnlyRaw, u32>::new();
    channel
        .send(33)
        .expect("invariant: fresh channel accepts its first state");
    let future = require_send(channel.receive(StateId::new()));

    let output = std::thread::scope(|scope| {
        scope
            .spawn(move || published(future))
            .join()
            .expect("invariant: borrowed receive control completes")
    });
    assert_eq!(output.map(|(_, value)| value), Some(33));
}

fn main() {
    native_owned_is_send();
    sync_only_owned_stays_local();
    borrowed_sync_owner_needs_no_send();
    println!("STATE_RECEIVE_OWNED_CONTROLS_COMPLETE");
}
