//! The SMB share-mode table is not a catalog, a credential, or a FILE writer.
//! One coordinator must span all connections/trees for the service. Both pending
//! and published opens vote; no mutex is held while acquiring or rechecking a
//! real storage open. The storage/root adapter is separate. These votes alone do not implement
//! CREATE. For ordinary directories MS-FSA 2.1.5.1.2's first actual bound
//! Open is decided in this same table; a pending seat is not an Open.
//! MS-SMB2 2.2.13 ignores wire FILE_DISALLOW_EXCLUSIVE.
pub(in crate::smb) mod path;
pub(in crate::smb) mod tree_connect;
mod query;
pub(in crate::smb) use query::root::{EndOfRoot,RootCursorEffect,RootPendingToken,RootScanEnd,records::FullRootRecords};
mod io;
mod disposition;
use disposition::DeleteState;
pub(in crate::smb) use disposition::{DispositionError,PreparedDisposition};
mod root_open;
mod file_open;
mod file_access;
pub(in crate::smb) use file_access::PreparedFileAccess;
mod attribute_tag;
pub(in crate::smb) use attribute_tag::{DIRECTORY_ATTRIBUTES,FILE_ATTRIBUTES,PreparedAttributeTag};
pub(crate) use file_open::{FileRenameTarget,OrdinaryMoveAdmission,OrdinaryRenameAdmission};
mod file_birth;
pub(crate) use file_birth::FileBirthVote;
pub(in crate::smb) use file_birth::PreparedBirth;
mod writable_file_open;
mod directory_open;
pub(crate) use directory_open::OrdinaryDirectoryRenameAdmission;
mod directory_birth;
pub(crate) use directory_birth::DirectoryBirthVote;
pub(in crate::smb) use directory_birth::PreparedDirectoryBirth;
mod retained_open;
pub(in crate::smb) use retained_open::{PreparedRetainedQuery,PreparedRetainedRead,PreparedRetainedRestore,PreparedRetainedStandard,RestoreError,RetainedOpen,RetiredRetainedRestore};
pub(in crate::smb) use directory_open::{DirectoryCloseError,DirectoryOpen,DirectoryOpenError,PreparedDirectoryName,PreparedDirectoryRename,PreparedDirectoryStandard,PreparedOrdinaryQuery};
pub(in crate::smb) use file_open::{FileCloseError,FileCloseFault,FileOpen,FileOpenError,FileReadError,FileStandardError,MoveOpenWitness,WorkingMoveGuard,OrdinaryRenameError,PreparedCrossFilesystemMove,PreparedFileName,PreparedFileRead,PreparedFileStandard,PreparedNamedFile,PreparedOrdinaryRename,RetiredOrdinaryRename};
pub(in crate::smb) use writable_file_open::{FileWriteError,PreparedFileEof,PreparedFileFlush,PreparedFileWrite,PreparedWriterName,PreparedWriterRead,PreparedWriterStandard,WritableCloseFault,WritableFileOpen};
pub(in crate::smb) use root_open::{PreparedRootFsAttributes,PreparedRootFsSize,PreparedRootInternal,PreparedRootQuery,RetiredRootFs,RetiredRootFsSize,RootCloseError,RootFsError,RootOpen,RootOpenError,RootQueryError};
pub(super) use io::{CloseCompletion, FlushCompletion, ReadCompletion, ReadFault, WriteCompletion, WriteFault};
use crate::{DirectoryId, FileId, OperationId, PoolId, VolumeId};
use std::{collections::HashMap, num::NonZeroUsize, sync::{Arc,Mutex,Weak}};

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
struct Counts { live: usize, bound: usize, retained:usize, needs: [usize; 3], denies: [usize; 3], deletion: Option<DeleteState>, renaming: Option<Weak<()>> }

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

pub(in crate::smb) struct OpenCoordinator { limit: NonZeroUsize, table: Mutex<Table> }

#[derive(Debug, Eq, PartialEq)]
enum ReserveError { SharingViolation, DeletePending, Resources, Unavailable }

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
    pub(in crate::smb) fn new(limit: NonZeroUsize) -> Arc<Self> { Arc::new(Self { limit, table: Mutex::default() }) }

    /// Existing files: reserve before acquiring the actual core open. New
    /// files: reserve inside the core's provisional-identity callback before
    /// its durable mutation. The two cuts refer to the same original request.
    fn reserve<'q, Q: OriginalRequest>(
        self:&Arc<Self>, request: &'q Q, planned: Facts, share: Share,
    ) -> Result<Pending<'q, Q>, ReserveError> {
        let vote = Vote::new(planned, share);
        let mut table = self.table.lock().map_err(|_| ReserveError::Unavailable)?;
        if let Some(row)=table.objects.get(&planned.object){
            if row.deletion.is_some(){return Err(ReserveError::DeletePending);}
            if row.renaming.is_some(){return Err(ReserveError::SharingViolation);}
            if row.conflicts(vote){return Err(ReserveError::SharingViolation);}
        }
        if table.live >= self.limit.get() { return Err(ReserveError::Resources); }
        if !table.objects.contains_key(&planned.object) {
            table.objects.try_reserve(1).map_err(|_| ReserveError::Resources)?;
        }
        table.objects.entry(planned.object).or_default().add(vote);
        table.live += 1;
        Ok(Pending { request, planned, slot: Slot { owner: Arc::clone(self), object: planned.object,
            vote, bound: false, retained:false, released: false, identity: Arc::new(()) } })
    }
}

/// A single non-cloneable share vote. It grants no IO and is never a wire ID.
struct Slot {
    owner: Arc<OpenCoordinator>, object: Object, vote: Vote, bound: bool,
    retained:bool, released: bool, identity: Arc<()>,
}

impl Slot {
    fn mark_bound(&mut self)->bool{
        let Ok(mut table)=self.owner.table.lock() else{return false;};
        let Some(row)=table.objects.get_mut(&self.object) else{return false;};
        if self.bound||self.released||row.bound>=row.live||row.deletion.is_some(){return false;}
        let mut effective=self.vote;
        // MS-FSA 2.1.5.1.2, after the sharing checks. Our ordinary
        // directories lack full FILE_GENERIC_WRITE, independent of actual
        // parent FILE_WRITE_DATA (already reflected in the pending vote).
        // Only this actual binding learns if earlier real Opens exist. It
        // may relax its own reservation; never guess from provisional seats.
        if row.bound==0 && effective.denies&R!=0 &&
            matches!(self.object,Object::OrdinaryRoot(..)|Object::Directory(..)){
            row.denies[0]-=1;
            effective.denies&=!R;
        }
        row.bound+=1;
        if self.retained{row.retained+=1;}
        drop(table);
        self.vote=effective;
        self.bound=true;
        true
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.released{return;}
        // A poisoned table already refuses all new opens. Never reset it or
        // let another caller take ownership by removing an unrelated entry.
        let Ok(mut table) = self.owner.table.lock() else { return };
        if let Some(row) = table.objects.get_mut(&self.object) {
            // Abort/disconnect never arms a mark or starts FILE movement.
            if row.deletion.as_ref().is_some_and(|mark|mark.is_unarmed_on(&self.identity)){
                row.deletion=None;
            }
            row.remove(self.vote);
            if self.bound{row.bound-=1;if self.retained{row.retained-=1;}}
            if row.live == 0 { table.objects.remove(&self.object); }
            table.live -= 1;
        }
    }
}

#[must_use = "dropping a pending reservation cancels it"]
struct Pending<'q, Q: OriginalRequest> {
    request: &'q Q,
    planned: Facts,
    slot: Slot,
}

#[derive(Debug, Eq, PartialEq)]
enum BindError<E> { Original(E), Changed }

impl<Q: OriginalRequest> Pending<'_, Q> {
    fn as_retained(mut self)->Self{self.slot.retained=true;self}
    /// The hold can be created only after the matching original request
    /// rechecks its real storage handle. A future wire OpenTable may publish
    /// that hold, never a Pending or just a share vote.
    fn bind(mut self, held: Q::Held) -> Result<Held<Q::Held>, BindError<Q::Error>> {
        let result = self.request.recheck(&held).map_err(BindError::Original)
            .and_then(|now| (now == self.planned).then_some(()).ok_or(BindError::Changed))
            .and_then(|()|self.slot.mark_bound().then_some(()).ok_or(BindError::Changed));
        if let Err(e) = result {
            drop(held); // close the core handle before releasing its vote
            return Err(e);
        }
        Ok(Held { original: held, _slot: self.slot, grant: self.planned.grant, root_search: query::root::Search::default() })
    }
}

/// Declaration order matters: drop the actual storage handle before the vote.
/// There is no unpair, publish-by-ID, auto-flush, or automatic history commit.
#[must_use = "dropping the held pair closes the original and releases its share vote"]
struct Held<H> {
    original: H,
    _slot: Slot,
    grant: Grant,
    root_search: query::root::Search,
}

#[cfg(test)]
#[path = "../../../tests/smb/backend_open.rs"]
mod tests;
