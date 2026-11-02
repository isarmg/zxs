// Compile-only: an owned state future must not be Send for a non-Send raw owner.
// Do not run a cross-thread negative case; the phantom is a type-contract witness.
#[path = "state-receive-owned-support.rs"]
mod support;

use futures_intrusive::channel::StateId;
use futures_intrusive::channel::shared::{StateReceiveFuture, generic_state_broadcast_channel};
use support::{SyncOnlyRaw, raw_drop_count};

fn require_sync<T: Sync>() {}
fn require_send<T: Send>(_: T) {}

fn main() {
    require_sync::<SyncOnlyRaw>();
    let before = raw_drop_count();
    let (sender, receiver) = generic_state_broadcast_channel::<SyncOnlyRaw, u32>();
    let future: StateReceiveFuture<SyncOnlyRaw, u32> = receiver.receive(StateId::new());
    drop(sender);
    drop(receiver);
    assert_eq!(raw_drop_count(), before);
    require_send(future);
}
