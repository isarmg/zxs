// Public synchronization::api ownership regressions through spin's lock_api adapter.
use spin::lock_api::{RwLock as ApiRwLock, RwLockUpgradableReadGuard};
use std::mem::{ManuallyDrop, forget};

#[test]
fn raw_try_upgrade_failure_preserves_public_guard() {
    let lock = ApiRwLock::new(());
    let reader = ManuallyDrop::new(
        lock.try_read()
            .expect("invariant: fresh lock permits a reader"),
    );
    let upgradable = lock
        .try_upgradable_read()
        .expect("invariant: one upgradable holder may coexist with a reader");

    // The baseline can return a guard whose raw ownership was already released.
    let held = match RwLockUpgradableReadGuard::try_upgrade(upgradable) {
        Err(guard) => ManuallyDrop::new(guard),
        Ok(guard) => {
            forget(guard);
            panic!("upgrade succeeded while a reader was held");
        }
    };

    let another_upgradable = lock.try_upgradable_read();
    let original_is_exclusive = another_upgradable.is_none();
    forget(another_upgradable);
    assert!(
        original_is_exclusive,
        "failed upgrade released the original upgradable ownership"
    );

    drop(ManuallyDrop::into_inner(reader));
    let competing_writer = lock.try_write();
    let writer_is_blocked = competing_writer.is_none();
    forget(competing_writer);
    assert!(
        writer_is_blocked,
        "the original upgradable holder must continue blocking writers"
    );

    drop(ManuallyDrop::into_inner(held));
    drop(
        lock.try_write()
            .expect("invariant: dropping the original holder releases the lock"),
    );
}

#[test]
fn raw_try_upgrade_success_retains_public_writer() {
    let lock = ApiRwLock::new(());
    let upgradable = lock
        .try_upgradable_read()
        .expect("invariant: fresh lock permits one upgradable holder");
    let held = match RwLockUpgradableReadGuard::try_upgrade(upgradable) {
        Ok(guard) => ManuallyDrop::new(guard),
        Err(guard) => {
            forget(guard);
            panic!("a sole upgradable holder could not upgrade");
        }
    };

    let competing_reader = lock.try_read();
    let reader_is_blocked = competing_reader.is_none();
    forget(competing_reader);
    assert!(
        reader_is_blocked,
        "the public writer retains exclusive ownership"
    );

    let competing_writer = lock.try_write();
    let writer_is_blocked = competing_writer.is_none();
    forget(competing_writer);
    assert!(
        writer_is_blocked,
        "upgrading must not release the writer lock"
    );

    drop(ManuallyDrop::into_inner(held));
    drop(
        lock.try_upgradable_read()
            .expect("invariant: the writer releases the lock exactly once"),
    );
}
