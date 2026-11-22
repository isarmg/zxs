//! In-memory OPEN/byte-range checks. No Core, driver, socket or disk.
use super::*;
use crate::{FileId, PoolId, smb::backend::{Facts, Grant, OpenCoordinator, OriginalRequest, ParentWrite, Share}};
use std::{convert::Infallible, num::NonZeroUsize};

const FILE: Object = Object::File(PoolId([1; 16]), FileId([2; 16]));

struct OpenForTest(Facts);
impl OriginalRequest for OpenForTest {
    type Held = ();
    type Error = Infallible;

    fn recheck(&self, _held: &()) -> Result<Facts, Infallible> {
        Ok(self.0)
    }
}

fn ordinary(coordinator: &Arc<OpenCoordinator>, object: Object, retained: bool) -> Held<()> {
    let facts = Facts { object, grant: Grant(7), parent_write: ParentWrite::Granted };
    let original = OpenForTest(facts);
    let pending = coordinator.reserve(&original, facts, Share(7)).unwrap();
    let pending = if retained { pending.as_retained() } else { pending };
    pending.bind(()).unwrap()
}

fn slot_of(held: &Held<()>) -> &Slot {
    let Held { _slot: slot, .. } = held;
    slot
}

fn coordinator() -> Arc<OpenCoordinator> {
    OpenCoordinator::new(NonZeroUsize::new(6).unwrap())
}

pub(in crate::smb::backend) fn change(slot: &Slot, offset: u64, length: u64, change: LockChange) -> Result<(), LockError> {
    slot.change_lock(Span::new(offset, length).unwrap(), change)
}

#[test]
fn exclusive_range_is_exact_to_the_open_and_other_files_are_unaffected() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let retained = ordinary(&shared, FILE, true);
    let other = ordinary(&shared, Object::File(PoolId([2; 16]), FileId([2; 16])), false);

    change(slot_of(&owner), 10, 8, LockChange::Exclusive).unwrap();
    assert!(matches!(change(slot_of(&owner), 12, 1, LockChange::Exclusive), Err(LockError::LockNotGranted)));
    assert!(matches!(change(slot_of(&peer), 10, 8, LockChange::Unlock), Err(LockError::RangeNotLocked)));
    assert!(matches!(change(slot_of(&owner), 12, 1, LockChange::Unlock), Err(LockError::RangeNotLocked)));
    assert!(matches!(change(slot_of(&retained), 19, 2, LockChange::Shared), Err(LockError::AccessDenied)));
    assert_eq!(slot_of(&retained).range_io(10, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    assert_eq!(slot_of(&peer).range_io(15, 1, Intent::Write).err(), Some(RangeIoError::Conflict));
    assert!(slot_of(&owner).range_io(10, 8, Intent::Write).is_ok());
    assert!(slot_of(&peer).range_io(18, 1, Intent::Read).is_ok());
    assert!(slot_of(&other).range_io(10, 8, Intent::Write).is_ok());
    change(slot_of(&owner), 10, 8, LockChange::Unlock).unwrap();
    assert!(slot_of(&peer).range_io(10, 8, Intent::Read).is_ok());
}

#[test]
fn shared_ranges_include_same_open_writes_and_exact_unlock_prefers_exclusive() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    change(slot_of(&owner), 0, 4, LockChange::Exclusive).unwrap();
    change(slot_of(&owner), 0, 4, LockChange::Shared).unwrap();
    assert_eq!(slot_of(&owner).range_io(2, 1, Intent::Write).err(), Some(RangeIoError::Conflict));
    assert_eq!(slot_of(&peer).range_io(2, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    change(slot_of(&owner), 0, 4, LockChange::Unlock).unwrap();
    assert!(slot_of(&peer).range_io(2, 1, Intent::Read).is_ok());
    change(slot_of(&peer), 3, 2, LockChange::Shared).unwrap();
    assert!(matches!(change(slot_of(&peer), 2, 1, LockChange::Exclusive), Err(LockError::LockNotGranted)));
    change(slot_of(&owner), 0, 4, LockChange::Unlock).unwrap();
    assert_eq!(slot_of(&owner).range_io(3, 1, Intent::Write).err(), Some(RangeIoError::Conflict));
    assert!(slot_of(&owner).range_io(3, 0, Intent::Write).unwrap().is_none());
}

#[test]
fn in_flight_data_turn_and_lock_admission_share_one_short_cut() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let read = slot_of(&owner).range_io(12, 3, Intent::Read).unwrap();
    std::thread::scope(|threads| {
        threads.spawn(|| {
            assert!(matches!(change(slot_of(&peer), 13, 1, LockChange::Exclusive), Err(LockError::LockNotGranted)));
            change(slot_of(&peer), 13, 1, LockChange::Shared).unwrap();
            change(slot_of(&peer), 13, 1, LockChange::Unlock).unwrap();
        }).join().unwrap();
    });
    drop(read);
    let write = slot_of(&owner).range_io(12, 3, Intent::Write).unwrap();
    assert!(matches!(change(slot_of(&peer), 14, 1, LockChange::Shared), Err(LockError::LockNotGranted)));
    change(slot_of(&peer), 15, 1, LockChange::Exclusive).unwrap();
    drop(write);
    change(slot_of(&peer), 14, 1, LockChange::Exclusive).unwrap();
}

#[test]
fn length_changes_check_only_the_interval_the_original_writer_changes() {
    let shared = coordinator();
    let blocker = ordinary(&shared, FILE, false);
    let writer = ordinary(&shared, FILE, false);
    change(slot_of(&blocker), 10, 3, LockChange::Shared).unwrap();
    assert_eq!(slot_of(&writer).resize_permit(15, 10).err(), Some(RangeIoError::Conflict));
    assert_eq!(slot_of(&writer).resize_permit(9, 12).err(), Some(RangeIoError::Conflict));
    assert!(slot_of(&writer).resize_permit(15, 13).is_ok());
    assert!(slot_of(&writer).resize_permit(11, 11).unwrap().is_none());
}

#[test]
fn drop_and_normal_departure_release_only_the_original_owner() {
    let shared = coordinator();
    let dropped = ordinary(&shared, FILE, false);
    let closed = ordinary(&shared, FILE, false);
    let surviving = ordinary(&shared, FILE, false);
    for (held, at) in [(&dropped, 0), (&closed, 1), (&surviving, 2)] {
        change(slot_of(held), at, 1, LockChange::Exclusive).unwrap();
    }
    drop(dropped);
    let Held { _slot: slot, .. } = closed;
    assert!(slot.finish_normal().unwrap().is_none());
    let new_owner = ordinary(&shared, FILE, false);
    assert!(slot_of(&new_owner).range_io(0, 2, Intent::Write).is_ok());
    assert_eq!(slot_of(&new_owner).range_io(2, 1, Intent::Write).err(), Some(RangeIoError::Conflict));
}

#[test]
fn per_open_and_per_file_limits_are_checked_without_eviction() {
    let shared = coordinator();
    let owners: Vec<_> = (0..5).map(|_| ordinary(&shared, FILE, false)).collect();
    for (owner, held) in owners.iter().take(4).enumerate() {
        for lock in 0..LOCKS_PER_OPEN {
            let offset = u64::try_from(owner * LOCKS_PER_OPEN + lock).unwrap();
            change(slot_of(held), offset, 1, LockChange::Exclusive).unwrap();
        }
        assert!(matches!(change(slot_of(held), 80, 1, LockChange::Exclusive), Err(LockError::Resources)));
    }
    assert_eq!(LOCKS_PER_FILE, 4 * LOCKS_PER_OPEN);
    assert!(matches!(change(slot_of(&owners[4]), 80, 1, LockChange::Exclusive), Err(LockError::Resources)));
    assert_eq!(slot_of(&owners[4]).range_io(0, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    change(slot_of(&owners[0]), 0, 1, LockChange::Unlock).unwrap();
    change(slot_of(&owners[4]), 80, 1, LockChange::Exclusive).unwrap();
}
