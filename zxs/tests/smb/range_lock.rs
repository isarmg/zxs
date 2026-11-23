//! In-memory OPEN/byte-range checks. No Core, driver, socket or disk.
use super::*;
use crate::{
    FileId, PoolId,
    smb::{
        backend::{Facts, Grant, OpenCoordinator, OriginalRequest, ParentWrite, Share},
        wire::lock_fields::tests::{packet, parse},
    },
};
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
    let flags = match change {
        LockChange::Shared => 0x11,
        LockChange::Exclusive => 0x12,
        LockChange::Unlock => 0x04,
    };
    batch(slot, &[(offset, length, flags)])
}

fn batch(slot: &Slot, rows: &[(u64, u64, u32)]) -> Result<(), LockError> {
    let data = packet(rows);
    let request = parse(&data).map_err(LockError::Fields)?;
    slot.change_lock(request)
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

#[test]
fn acquired_elements_keep_the_same_open_and_rollback_only_the_failed_suffix() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let owner = slot_of(&owner);
    let peer = slot_of(&peer);

    change(owner, 10, 4, LockChange::Shared).unwrap();
    change(peer, 40, 2, LockChange::Exclusive).unwrap();
    assert!(matches!(
        batch(owner, &[(10, 4, 0x11), (20, 2, 0x12), (41, 1, 0x12), (50, 1, 0x12)]),
        Err(LockError::LockNotGranted)
    ));
    assert!(peer.range_io(20, 2, Intent::Read).is_ok());
    assert!(peer.range_io(50, 1, Intent::Write).is_ok());
    assert_eq!(owner.range_io(41, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    change(owner, 10, 4, LockChange::Unlock).unwrap();
    assert!(matches!(change(owner, 10, 4, LockChange::Unlock), Err(LockError::RangeNotLocked)));

    batch(owner, &[(20, 2, 0x12), (10, 4, 0x11)]).unwrap();
    assert_eq!(peer.range_io(20, 2, Intent::Read).err(), Some(RangeIoError::Conflict));
    assert_eq!(owner.range_io(10, 4, Intent::Write).err(), Some(RangeIoError::Conflict));
    batch(owner, &[(20, 2, 0x04), (10, 4, 0x04)]).unwrap();
    assert!(peer.range_io(20, 2, Intent::Read).is_ok());
    assert!(owner.range_io(10, 4, Intent::Write).is_ok());
}

#[test]
fn batch_conflicts_with_its_own_earlier_element_and_with_in_flight_io() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let owner = slot_of(&owner);
    let peer = slot_of(&peer);

    assert!(matches!(
        batch(owner, &[(10, 4, 0x11), (11, 1, 0x12)]),
        Err(LockError::LockNotGranted)
    ));
    assert!(peer.range_io(10, 4, Intent::Write).is_ok());

    let active = peer.range_io(30, 2, Intent::Write).unwrap();
    assert!(matches!(
        batch(owner, &[(10, 4, 0x12), (31, 1, 0x11)]),
        Err(LockError::LockNotGranted)
    ));
    drop(active);
    assert!(peer.range_io(10, 4, Intent::Read).is_ok());
}

#[test]
fn quota_failure_does_not_leave_part_of_an_acquisition() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let owner = slot_of(&owner);
    let peer = slot_of(&peer);

    for offset in 0..LOCKS_PER_OPEN - 1 {
        change(owner, u64::try_from(offset).unwrap(), 1, LockChange::Exclusive).unwrap();
    }
    assert!(matches!(batch(owner, &[(100, 1, 0x12), (101, 1, 0x12)]), Err(LockError::Resources)));
    assert!(peer.range_io(100, 2, Intent::Read).is_ok());
    assert_eq!(peer.range_io(0, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    change(owner, 101, 1, LockChange::Exclusive).unwrap();
}

#[test]
fn unlock_stops_in_wire_order_and_keeps_the_released_prefix() {
    for (flags, length) in [(0x04, 1), (0x11, 0)] {
        let shared = coordinator();
        let owner = ordinary(&shared, FILE, false);
        let peer = ordinary(&shared, FILE, false);
        let owner = slot_of(&owner);
        let peer = slot_of(&peer);
        batch(owner, &[(10, 1, 0x12), (30, 1, 0x12)]).unwrap();

        let result = batch(owner, &[(10, 1, 0x14), (20, length, flags), (30, 1, 0x04)]);
        if flags == 0x04 {
            assert!(matches!(result, Err(LockError::RangeNotLocked)));
        } else {
            assert!(matches!(result, Err(LockError::Fields(LockFieldError::InvalidParameter))));
        }
        assert!(peer.range_io(10, 1, Intent::Read).is_ok());
        assert_eq!(peer.range_io(30, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    }
}

#[test]
fn mixed_acquisition_retains_a_completed_prefix_only_if_processing_started() {
    let shared = coordinator();
    let owner = ordinary(&shared, FILE, false);
    let peer = ordinary(&shared, FILE, false);
    let owner = slot_of(&owner);
    let peer = slot_of(&peer);

    assert!(matches!(
        batch(owner, &[(10, 1, 0x12), (20, 1, 0x04)]),
        Err(LockError::Fields(LockFieldError::InvalidParameter))
    ));
    assert!(peer.range_io(10, 1, Intent::Read).is_ok());
    assert!(matches!(
        batch(owner, &[(10, 1, 0x12), (20, 1, 0x14), (30, 1, 0x12)]),
        Err(LockError::Fields(LockFieldError::UnexpectedUnlock))
    ));
    assert_eq!(peer.range_io(10, 1, Intent::Read).err(), Some(RangeIoError::Conflict));
    assert!(peer.range_io(30, 1, Intent::Read).is_ok());
}
