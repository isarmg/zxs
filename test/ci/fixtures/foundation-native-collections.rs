use std::{
    cell::Cell,
    future::Future,
    hash::BuildHasher,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};

struct CountDrop(Arc<AtomicUsize>);

impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn allocator_reserve_overflow_preserves_values_and_drain_drops_once() {
    use allocator_api2::collections::TryReserveErrorKind;

    let dropped = Arc::new(AtomicUsize::new(0));
    {
        let mut values = allocator_api2::vec::Vec::new();
        for _ in 0..4 {
            values.push(CountDrop(Arc::clone(&dropped)));
        }
        let capacity = values.capacity();
        let error = values.try_reserve(usize::MAX).unwrap_err();
        assert!(matches!(
            error.kind(),
            TryReserveErrorKind::CapacityOverflow
        ));
        assert_eq!(values.len(), 4);
        assert_eq!(values.capacity(), capacity);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(values.drain(1..3));
        assert_eq!(values.len(), 2);
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 4);
}

#[test]
fn equivalent_borrowed_keys_preserve_equality_and_ordering() {
    use equivalent::{Comparable, Equivalent};

    let key = String::from("repository");
    assert!("repository".equivalent(&key));
    assert!(!"Repository".equivalent(&key));
    assert_eq!("repository".compare(&key), std::cmp::Ordering::Equal);
    assert_eq!("repo".compare(&key), std::cmp::Ordering::Less);
    assert_eq!("z".compare(&key), std::cmp::Ordering::Greater);
}

#[test]
fn foldhash_cloned_builder_preserves_map_lookup_after_growth_and_removal() {
    let state = foldhash::fast::FixedState::with_seed(17);
    assert_eq!(
        state.hash_one("repository"),
        state.clone().hash_one("repository")
    );
    let mut map = std::collections::HashMap::with_hasher(state);
    for key in 0..256 {
        assert_eq!(map.insert(key, key * 3), None);
    }
    for key in (0..256).step_by(2) {
        assert_eq!(map.remove(&key), Some(key * 3));
    }
    for key in 0..256 {
        assert_eq!(map.get(&key).copied(), (key % 2 != 0).then_some(key * 3));
    }
}

#[test]
fn litemap_borrowed_sorted_store_checks_search_and_range_boundaries() {
    let store = [(1_u16, "one"), (3, "three"), (5, "five")];
    let map = litemap::LiteMap::from_sorted_store_unchecked(store.as_slice());
    assert_eq!(map.get(&3), Some(&"three"));
    assert_eq!(map.get(&4), None);
    assert_eq!(map.get_by(|key| key.cmp(&5)), Some(&"five"));
    assert_eq!(map.get_indexed(3), None);
    let range = map.get_indexed_range(1..3).unwrap();
    assert_eq!(
        range.values().copied().collect::<Vec<_>>(),
        ["three", "five"]
    );
    assert!(map.get_indexed_range(2..4).is_none());
}

#[test]
fn stable_deref_bounds_keep_box_moves_and_rc_clones_at_the_same_address() {
    fn stable<T: stable_deref_trait::StableDeref>(value: &T) -> *const T::Target {
        &**value
    }
    fn stable_clone<T: stable_deref_trait::CloneStableDeref + Clone>(value: &T) -> T {
        value.clone()
    }

    let boxed = Box::new([1_u8, 2, 3, 4]);
    let address = stable(&boxed);
    let moved = boxed;
    assert!(std::ptr::eq(address, stable(&moved)));
    assert_eq!(*moved, [1, 2, 3, 4]);

    let original: Rc<str> = Rc::from("borrowed payload");
    let address = stable(&original);
    let cloned = stable_clone(&original);
    drop(original);
    assert!(std::ptr::eq(address, stable(&cloned)));
    assert_eq!(&*cloned, "borrowed payload");
}

#[test]
fn yoke_borrowed_cart_preserves_its_backing_storage_when_moved() {
    let storage = String::from("repository payload");
    let yoke = yoke::Yoke::<&'static str, _>::attach_to_cart(storage.as_str(), |text| text);
    assert_eq!(*yoke.get(), storage.as_str());
    assert_eq!(yoke.get().as_ptr(), storage.as_ptr());
    let moved = yoke;
    assert_eq!(moved.get().as_ptr(), storage.as_ptr());
    assert_eq!(*moved.backing_cart(), storage.as_str());
}

#[test]
fn zerofrom_derive_borrows_text_and_slice_without_copying_the_payload() {
    use zerofrom::ZeroFrom;

    #[derive(ZeroFrom)]
    struct Borrowed<'a> {
        text: &'a str,
        bytes: &'a [u8],
    }

    let text = String::from("版本");
    let bytes = [1, 2, 3, 4];
    let source = Borrowed {
        text: &text,
        bytes: &bytes,
    };
    let borrowed = Borrowed::zero_from(&source);
    assert_eq!(borrowed.text, "版本");
    assert_eq!(borrowed.bytes, [1, 2, 3, 4]);
    assert_eq!(borrowed.text.as_ptr(), text.as_ptr());
    assert_eq!(borrowed.bytes.as_ptr(), bytes.as_ptr());
}

#[test]
fn sync_wrapper_requires_exclusive_access_and_polls_a_non_sync_future() {
    fn assert_sync<T: Sync>(_: &T) {}

    let mut wrapper = sync_wrapper::SyncWrapper::new(Cell::new(7));
    assert_sync(&wrapper);
    wrapper.get_mut().set(9);
    assert_eq!(wrapper.into_inner().get(), 9);

    let mut first = true;
    let value = Cell::new(11);
    let future = std::future::poll_fn(move |context| {
        if first {
            first = false;
            value.set(13);
            context.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(value.get())
        }
    });
    let wrapped = sync_wrapper::SyncFuture::new(future);
    assert_sync(&wrapped);
    let mut pinned = std::pin::pin!(wrapped);
    let mut context = Context::from_waker(Waker::noop());
    assert!(pinned.as_mut().poll(&mut context).is_pending());
    assert_eq!(pinned.as_mut().poll(&mut context), Poll::Ready(13));
}

#[test]
fn sharded_slab_removal_keeps_an_existing_guard_alive_until_its_release() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let slab = sharded_slab::Slab::new();
    let key = slab.insert(CountDrop(Arc::clone(&dropped))).unwrap();
    let guard = slab.get(key).unwrap();
    assert!(slab.remove(key));
    assert!(slab.get(key).is_none());
    assert_eq!(guard.0.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(guard);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(!slab.remove(key));
}
