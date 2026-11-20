//! In-memory checks of the sharing table, not an authenticated SMB harness.
use super::*;
use std::{cell::Cell, sync::mpsc};

fn pool(n: u8) -> PoolId { PoolId([n; 16]) }
fn file(n: u8) -> Object { Object::File(pool(1), FileId([n; 16])) }
fn facts(object: Object, bits: u32) -> Facts {
    Facts { object, grant: Grant::from_effective(bits).unwrap(), parent_write: ParentWrite::Granted }
}
fn table(n: usize) -> OpenCoordinator { OpenCoordinator::new(NonZeroUsize::new(n).unwrap()) }

struct Attempt(u8);
struct Original { attempt: u8, actual: Facts }

impl OriginalRequest for Attempt {
    type Held = Original;
    type Error = &'static str;
    fn recheck(&self, held: &Original) -> Result<Facts, Self::Error> {
        if self.0 != held.attempt { return Err("different or revoked request"); }
        Ok(held.actual)
    }
}
fn effective(axes: u8) -> u32 {
    u32::from(axes & R != 0) | (u32::from(axes & W != 0) << 1) |
        (u32::from(axes & D != 0) << 16)
}

#[test]
fn all_ordinary_share_masks_are_checked_in_both_directions() {
    let t = table(2);
    let req = Attempt(1);
    for old_uses in 0..=ALL {
        for old_share in 0..=ALL {
            let old = t.reserve(&req, facts(file(1), effective(old_uses)), Share(old_share)).unwrap();
            for uses in 0..=ALL {
                for share in 0..=ALL {
                    let conflict = old_uses != 0 && uses != 0 &&
                        (uses & !old_share != 0 || old_uses & !share != 0);
                    let admission = t.reserve(&req, facts(file(1), effective(uses)), Share(share));
                    assert_eq!(admission.err(), conflict.then_some(ReserveError::SharingViolation));
                }
            }
            drop(old);
        }
    }
    assert!(t.table.lock().unwrap().objects.is_empty());
}

#[test]
fn data_execute_append_delete_and_attributes_are_distinct() {
    assert_eq!(Grant::from_effective(0x20).unwrap().axes(), R);
    assert_eq!(Grant::from_effective(0x4).unwrap().axes(), W);
    assert_eq!(Grant::from_effective(0x10000).unwrap().axes(), D);
    assert_eq!(Grant::from_effective(0x1001d8).unwrap().axes(), 0); // attrs, EA, delete-child, synchronize
    assert!(Grant::from_effective(0x0200_0000).is_none());
    assert!(Grant::from_effective(0x8000_0000).is_none());
    assert!(Share::from_bits(8).is_none());
    assert_eq!(Share::from_bits(7), Some(Share(ALL)));

    let t = table(4); let req = Attempt(1);
    let info = t.reserve(&req, facts(file(1), 0x80), Share(0)).unwrap();
    let mut plan = facts(file(1), 0x4);
    plan.parent_write = ParentWrite::Denied;
    let writing = t.reserve(&req, plan, Share(0)).unwrap();
    let reading = t.reserve(&req, facts(file(1), 0x20), Share(ALL)).unwrap();
    assert_eq!(t.reserve(&req, facts(file(1), 0x4), Share(ALL)).err(),
               Some(ReserveError::SharingViolation));
    drop((info, writing, reading));
}

#[test]
fn same_file_across_sessions_conflicts_but_other_files_and_pools_do_not() {
    let t = table(5); let a = Attempt(1); let b = Attempt(2);
    let f = facts(file(1), 2);
    let p = t.reserve(&a, f, Share(R)).unwrap();
    assert_eq!(t.reserve(&b, f, Share(ALL)).err(), Some(ReserveError::SharingViolation));
    let h = p.bind(Original { attempt: 1, actual: f }).unwrap();
    assert_eq!(h.original.actual, f);
    assert_eq!(t.reserve(&b, f, Share(ALL)).err(), Some(ReserveError::SharingViolation));
    let _r = t.reserve(&b, facts(file(1), 1), Share(ALL)).unwrap();
    let _other_file = t.reserve(&b, facts(file(2), 2), Share(0)).unwrap();
    let _other_pool = t.reserve(&b, facts(Object::File(pool(2), FileId([1;16])), 2), Share(0)).unwrap();
    let _other_kind = t.reserve(&b, facts(Object::Directory(pool(1), DirectoryId([1;16])), 2), Share(0)).unwrap();
    drop(h);
    assert!(t.reserve(&b, f, Share(ALL)).is_ok());
}

#[test]
fn provisional_and_bound_holds_count_toward_one_limit_and_failure_releases() {
    let t = table(1); let req = Attempt(4); let f = facts(file(9), 2);
    let a = t.reserve(&req, f, Share(ALL)).unwrap();
    assert_eq!(t.reserve(&req, facts(file(2), 0x80), Share(ALL)).err(), Some(ReserveError::Resources));
    assert_eq!(a.bind(Original { attempt: 5, actual: f }).err(),
               Some(BindError::Original("different or revoked request")));
    for changed in [
        facts(file(7),2),
        facts(file(9),0x102), // same write share axis, different full granted access
        Facts { parent_write: ParentWrite::Denied, ..f }
    ] {
        let p = t.reserve(&req, f, Share(ALL)).unwrap();
        assert_eq!(p.bind(Original { attempt: 4, actual: changed }).err(), Some(BindError::Changed));
    }
    let p = t.reserve(&req, f, Share(ALL)).unwrap();
    let held = p.bind(Original { attempt: 4, actual: f }).unwrap();
    assert_eq!(t.reserve(&req, f, Share(ALL)).err(), Some(ReserveError::Resources));
    drop(held);
    assert_eq!(t.table.lock().unwrap().live, 0);
}

#[test]
fn share_write_is_not_the_business_writer_lease_and_recheck_is_unlocked() {
    let t = table(3); let f = facts(file(1), 2);
    let req = Attempt(1);
    let p1 = t.reserve(&req, f, Share(ALL)).unwrap();
    // Both protocol votes can coexist. Only the storage adapter may supply a
    // real FILE writer. A failed core acquisition drops its provisional vote.
    let p2 = t.reserve(&req, f, Share(ALL)).unwrap();
    drop(p2);
    assert_eq!(t.table.try_lock().unwrap().live, 1);
    drop(p1);

    struct Checked<'a>(&'a OpenCoordinator, &'a Cell<bool>);
    struct Handle<'a>(&'a OpenCoordinator, &'a Cell<bool>, Facts);
    impl Drop for Handle<'_> {
        fn drop(&mut self) {
            self.1.set(self.0.table.try_lock().is_ok_and(|t| t.live == 1));
        }
    }
    impl<'a> OriginalRequest for Checked<'a> {
        type Held = Handle<'a>;
        type Error = ();
        fn recheck(&self, held: &Handle<'a>) -> Result<Facts, ()> {
            self.1.set(self.0.table.try_lock().is_ok());
            Ok(held.2)
        }
    }
    let (observed_check, observed_drop) = (Cell::new(false), Cell::new(false));
    let original = Checked(&t, &observed_check);
    let open = t.reserve(&original, f, Share(R)).unwrap()
        .bind(Handle(&t, &observed_drop, f)).unwrap();
    assert!(observed_check.get());
    drop(open);
    assert!(observed_drop.get());
    assert_eq!(t.table.lock().unwrap().live, 0);
}

#[test]
fn a_pending_open_votes_across_threads_and_leaves_no_stale_denial() {
    let t = table(2); let req = Attempt(1); let f = facts(file(5), 1);
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let (leave_tx, leave_rx) = mpsc::sync_channel(0);
    let coord = &t;
    std::thread::scope(|s| {
        let worker = s.spawn(move || {
            let req = Attempt(2);
            let hold = coord.reserve(&req, f, Share(R)).unwrap();
            ready_tx.send(()).unwrap();
            leave_rx.recv().unwrap();
            drop(hold);
        });
        ready_rx.recv().unwrap();
        let answer = t.reserve(&req, facts(file(5),2), Share(ALL)).err();
        leave_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(answer, Some(ReserveError::SharingViolation));
    });
    assert!(t.reserve(&req, facts(file(5),2), Share(ALL)).is_ok());
}
