//! The SMB share-mode table is not a catalog, a credential, or a FILE writer.
//! One coordinator must span all connections/trees for the service. Both pending
//! and published opens vote; no mutex is held while acquiring or rechecking a
//! real storage open. There is deliberately no production authenticator or
//! storage implementation in this slice. These votes alone do not implement
//! CREATE. Before admitting directories, the adapter must also order the
//! MS-FSA 2.1.5.1.2 first-directory-open rule with real directory rights and
//! other opens; a lock-free guess is not evidence. MS-SMB2 2.2.13 requires
//! ignoring the wire FILE_DISALLOW_EXCLUSIVE option, not forwarding it as a local flag.
pub(in crate::smb) mod path;
pub(in crate::smb) mod tree_connect;
mod query;
mod io;
pub(super) use io::{CloseCompletion, FlushCompletion, ReadCompletion, ReadFault, WriteCompletion, WriteFault};
use crate::{DirectoryId, FileId, OperationId, PoolId, VolumeId};
use std::{collections::HashMap, num::NonZeroUsize, sync::Mutex};

/// These are original domain identities, not SMB2_FILEIDs, host paths or
/// file numbers returned by directory enumeration. A move never changes File.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Object {
    Namespace(PoolId),
    OrdinaryRoot(PoolId, VolumeId),
    Directory(PoolId, DirectoryId),
    RecycleRoot(PoolId),
    DeleteRound(PoolId, OperationId),
    File(PoolId, FileId),
}

const R: u8 = 1;
const W: u8 = 2;
const D: u8 = 4;
const ALL: u8 = R | W | D;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Share(u8);

impl Share {
    fn from_bits(bits: u32) -> Option<Self> {
        (bits & !u32::from(ALL) == 0).then_some(Self(bits as u8))
    }
}

/// Access already expanded and granted for this object; not an access check.
/// Keep the whole grant for the final comparison, not just the three axes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Grant(u32);

impl Grant {
    fn from_effective(mask: u32) -> Option<Self> {
        const GENERIC_OR_MAXIMUM: u32 = 0xf000_0000 | 0x0200_0000;
        (mask & GENERIC_OR_MAXIMUM == 0).then_some(Self(mask))
    }
    fn axes(self) -> u8 {
        let mut axes = 0;
        if self.0 & (0x0001 | 0x0020) != 0 { axes |= R; } // READ_DATA | EXECUTE
        if self.0 & (0x0002 | 0x0004) != 0 { axes |= W; } // WRITE_DATA | APPEND_DATA
        if self.0 & 0x0001_0000 != 0 { axes |= D; } // DELETE (not DELETE_CHILD)
        axes
    }
}

/// MS-FSA 2.1.5.1.2.2: a caller without FILE_WRITE_DATA on the parent
/// cannot deny other readers. This is a fact from the actual parent access
/// check, not inferred from the child's desired access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParentWrite { Granted, Denied }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Facts {
    object: Object,
    grant: Grant,
    parent_write: ParentWrite,
}

#[derive(Clone, Copy)]
struct Vote { needs: u8, denies: u8 }

impl Vote {
    fn new(facts: Facts, share: Share) -> Self {
        let needs = facts.grant.axes();
        let allowed = share.0 | if facts.parent_write == ParentWrite::Denied { R } else { 0 };
        // An attribute-only handle neither blocks nor is blocked by these
        // three share axes; it is still a real open and consumes a slot.
        Self { needs, denies: if needs == 0 { 0 } else { ALL & !allowed } }
    }
}

#[derive(Default)]
struct Counts { live: usize, needs: [usize; 3], denies: [usize; 3] }

impl Counts {
    fn conflicts(&self, vote: Vote) -> bool {
        (0..3).any(|i|
            (vote.needs & (1 << i) != 0 && self.denies[i] != 0) ||
            (vote.denies & (1 << i) != 0 && self.needs[i] != 0))
    }
    fn add(&mut self, vote: Vote) {
        self.live += 1;
        for i in 0..3 {
            self.needs[i] += usize::from(vote.needs & (1 << i) != 0);
            self.denies[i] += usize::from(vote.denies & (1 << i) != 0);
        }
    }
    fn remove(&mut self, vote: Vote) {
        self.live -= 1;
        for i in 0..3 {
            self.needs[i] -= usize::from(vote.needs & (1 << i) != 0);
            self.denies[i] -= usize::from(vote.denies & (1 << i) != 0);
        }
    }
}

#[derive(Default)]
struct Table { live: usize, objects: HashMap<Object, Counts> }

struct OpenCoordinator { limit: NonZeroUsize, table: Mutex<Table> }

#[derive(Debug, Eq, PartialEq)]
enum ReserveError { SharingViolation, Resources, Unavailable }

/// Implement only for a concrete storage-side adapter. The request owns or
/// borrows the exact original authenticated session/tree and opening intent.
/// It must check that this held object is its own, still authorized, on the
/// original medium, with its real FILE writer if the operation needs one.
/// No implementation for (), booleans, usernames, wire IDs or SQL rows.
trait OriginalRequest {
    type Held;
    type Error;
    fn recheck(&self, held: &Self::Held) -> Result<Facts, Self::Error>;
}

impl OpenCoordinator {
    fn new(limit: NonZeroUsize) -> Self { Self { limit, table: Mutex::default() } }

    /// Existing files: reserve before acquiring the actual core open. New
    /// files: reserve inside the core's provisional-identity callback before
    /// its durable mutation. The two cuts refer to the same original request.
    fn reserve<'s, 'q, Q: OriginalRequest>(
        &'s self, request: &'q Q, planned: Facts, share: Share,
    ) -> Result<Pending<'s, 'q, Q>, ReserveError> {
        let vote = Vote::new(planned, share);
        let mut table = self.table.lock().map_err(|_| ReserveError::Unavailable)?;
        if table.objects.get(&planned.object).is_some_and(|row| row.conflicts(vote)) {
            return Err(ReserveError::SharingViolation);
        }
        if table.live >= self.limit.get() { return Err(ReserveError::Resources); }
        if !table.objects.contains_key(&planned.object) {
            table.objects.try_reserve(1).map_err(|_| ReserveError::Resources)?;
        }
        table.objects.entry(planned.object).or_default().add(vote);
        table.live += 1;
        Ok(Pending { request, planned, slot: Slot { owner: self, object: planned.object, vote } })
    }
}

/// A single non-cloneable share vote. It grants no IO and is never a wire ID.
struct Slot<'s> { owner: &'s OpenCoordinator, object: Object, vote: Vote }

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        // A poisoned table already refuses all new opens. Never reset it or
        // let another caller take ownership by removing an unrelated entry.
        let Ok(mut table) = self.owner.table.lock() else { return };
        if let Some(row) = table.objects.get_mut(&self.object) {
            row.remove(self.vote);
            if row.live == 0 { table.objects.remove(&self.object); }
            table.live -= 1;
        }
    }
}

#[must_use = "dropping a pending reservation cancels it"]
struct Pending<'s, 'q, Q: OriginalRequest> {
    request: &'q Q,
    planned: Facts,
    slot: Slot<'s>,
}

#[derive(Debug, Eq, PartialEq)]
enum BindError<E> { Original(E), Changed }

impl<'s, Q: OriginalRequest> Pending<'s, '_, Q> {
    /// The hold can be created only after the matching original request
    /// rechecks its real storage handle. A future wire OpenTable may publish
    /// that hold, never a Pending or just a share vote.
    fn bind(self, held: Q::Held) -> Result<Held<'s, Q::Held>, BindError<Q::Error>> {
        let result = self.request.recheck(&held).map_err(BindError::Original)
            .and_then(|now| (now == self.planned).then_some(()).ok_or(BindError::Changed));
        if let Err(e) = result {
            drop(held); // close the core handle before releasing its vote
            return Err(e);
        }
        Ok(Held { original: held, _slot: self.slot, grant: self.planned.grant, root_position: None })
    }
}

/// Declaration order matters: drop the actual storage handle before the vote.
/// There is no unpair, publish-by-ID, auto-flush, or automatic history commit.
#[must_use = "dropping the held pair closes the original and releases its share vote"]
struct Held<'s, H> {
    original: H,
    _slot: Slot<'s>,
    grant: Grant,
    root_position: Option<query::root::Position>,
}

#[cfg(test)]
#[path = "../../test/smb/backend_open.rs"]
mod tests;
