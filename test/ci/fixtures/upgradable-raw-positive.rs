#[path = "upgradable-raw-support.rs"]
mod support;

use support::LocalRaw;
use synchronization::api::{RwLock as GenericRwLock, RwLockUpgradableReadGuard};
use synchronization::{RawRwLock, RwLock};

fn require_sync<T: ?Sized + Sync>(_: &T) {}

fn native_borrowed<'a>(text: &'a str) {
    let lock = RwLock::new(text);
    let guard: RwLockUpgradableReadGuard<'_, RawRwLock, &'a str> = lock.upgradable_read();
    require_sync(&guard);
    std::thread::scope(|scope| {
        let shared = &guard;
        scope.spawn(move || {
            assert_eq!(**shared, text);
            assert!(RwLockUpgradableReadGuard::rwlock(shared).is_locked());
        });
    });
}

fn local_owner_stays_usable() {
    let lock = GenericRwLock::<LocalRaw, u32>::new(8);
    let guard = lock.upgradable_read();
    assert_eq!(*guard, 8);
    assert!(RwLockUpgradableReadGuard::rwlock(&guard).is_locked());
    assert!(lock.try_write().is_none());

    let mut writer = RwLockUpgradableReadGuard::upgrade(guard);
    *writer = 9;
    drop(writer);
    assert_eq!(*lock.read(), 9);
    assert!(!lock.is_locked());
}

fn main() {
    let text = String::from("borrowed native data");
    native_borrowed(&text);
    local_owner_stays_usable();
    println!("UPGRADABLE_POSITIVE_CONTROLS_COMPLETE");
}
