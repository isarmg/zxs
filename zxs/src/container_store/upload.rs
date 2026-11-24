//! Held resumable intake, disconnected from the listener until paired review.
mod adoption;
mod journal;
mod material;
mod native;
mod record;

use super::{
    Change, Decision, Id, Mutation, OperationReceipt, State, Store, Target, invalid,
    volume::DataVolume,
};
use crate::{fs::HeldDir, hash::Sha256};
use journal::{MAX_ACTIVE, NAMESPACE, Slot, capacity};
use record::{CHUNK, ChunkUse, Intent, Phase, Record, Scope, chunk_use};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

const MAX_DECLARED: u64 = 4 << 40;
const HEADROOM: u64 = 1 << 20;
const READERS: usize = 4;
type Authorize<'a> = &'a dyn Fn() -> io::Result<()>;
type Gate = Mutex<()>;

#[derive(Debug)]
enum Failure {
    Io(io::Error),
    Absent,
    Conflict,
    OldEpoch,
    InvalidBody,
}
impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug, Eq, PartialEq)]
enum Status {
    Open,
    Complete,
    Committed,
}

#[derive(Debug, Eq, PartialEq)]
struct Progress {
    intent: Intent,
    upload: [u8; 32],
    offset: u64,
    status: Status,
}

#[derive(Clone, Copy)]
struct Chunk {
    mutation: Mutation,
    upload: [u8; 32],
    at: u64,
    length: u64,
    sha256: [u8; 32],
}

#[derive(Default)]
pub(super) struct Shared {
    control: Mutex<Option<Arc<HeldDir>>>,
    gates: Mutex<BTreeMap<Id, Weak<Gate>>>,
    admission: Gate,
    readers: AtomicUsize,
    verifiers: AtomicUsize,
}

struct Uploads<'a> {
    store: &'a Store,
    system: &'a HeldDir,
    scope: Scope, // From the selected local account, not from HTTP fields.
}

impl<'a> Uploads<'a> {
    fn bind(store: &'a Store, system: &'a HeldDir, scope: Scope) -> io::Result<Self> {
        if scope.pool == [0; 16] || scope.epoch == [0; 16] || scope.pool == scope.epoch {
            return Err(invalid("invalid selected upload account scope"));
        }
        let this = Self {
            store,
            system,
            scope,
        };
        this.require_authority()?;
        Ok(this)
    }

    fn require_authority(&self) -> io::Result<()> {
        self.store.routes.require_upload_parent(self.system)?;
        native::require_owner(self.system, c"container-owner", &self.store.system_owner)
    }

    fn require_control(&self, directory: &HeldDir) -> io::Result<()> {
        self.require_authority()?;
        let current = self
            .system
            .child_directory(NAMESPACE)?
            .ok_or_else(|| invalid("original SYSTEM upload namespace is absent"))?;
        if current.entity != directory.entity || !current.same_mount(directory)? {
            return Err(invalid("SYSTEM upload namespace was replaced"));
        }
        directory.require_named()
    }

    fn directory(
        &self,
        create: bool,
        authorize: Authorize<'_>,
    ) -> io::Result<Option<Arc<HeldDir>>> {
        self.directory_with(create, authorize, File::sync_all)
    }

    fn directory_with(
        &self,
        create: bool,
        authorize: Authorize<'_>,
        sync_system: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Option<Arc<HeldDir>>> {
        let mut control = self
            .store
            .uploads
            .control
            .lock()
            .map_err(|_| unavailable())?;
        if let Some(directory) = &*control {
            self.require_control(directory)?;
            return Ok(Some(Arc::clone(directory)));
        }
        let directory = match self.system.child_directory(NAMESPACE)? {
            Some(directory) => directory,
            None if create => {
                self.require_authority()?;
                authorize()?;
                self.system.new_child_directory(NAMESPACE)?
            }
            None => return Ok(None),
        };
        self.require_control(&directory)?;
        authorize()?;
        directory.file.sync_all()?;
        // A first mkdir may already be visible after a failed parent flush.
        // No cache/use of even an existing name until its own parent settles.
        self.require_control(&directory)?;
        authorize()?;
        sync_system(&self.system.file)?;
        self.require_control(&directory)?;
        authorize()?;
        let directory = Arc::new(directory);
        *control = Some(Arc::clone(&directory));
        Ok(Some(directory))
    }

    fn gate(&self, mutation: Mutation, authorize: Authorize<'_>) -> Result<Arc<Gate>> {
        authorize()?;
        if mutation.container == [0; 16] || mutation.request == [0; 16] {
            return Err(Failure::Conflict);
        }
        let mut gates = self.store.uploads.gates.lock().map_err(|_| unavailable())?;
        gates.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = gates.get(&mutation.container).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(mutation.container, Arc::downgrade(&gate));
        Ok(gate)
    }

    fn under_object<T>(
        &self,
        mutation: Mutation,
        authorize: Authorize<'_>,
        action: impl FnOnce(&mut Option<State>, Option<OperationReceipt>) -> Result<T>,
    ) -> Result<T> {
        let object = self.store.object(mutation.container)?;
        let mut memory = object.lock().map_err(|_| unavailable())?;
        authorize()?;
        self.require_authority()?;
        let official = self.store.receipt_under_gate(mutation, &mut memory)?;
        action(&mut memory, official)
    }

    fn begin(&self, intent: &Intent, authorize: Authorize<'_>) -> Result<Progress> {
        let gate = self.gate(intent.mutation, authorize)?;
        if intent.scope != self.scope {
            return Err(Failure::OldEpoch);
        }
        intent.upload_id()?;
        let _serial = gate.lock().map_err(|_| unavailable())?;
        let _admit = self
            .store
            .uploads
            .admission
            .lock()
            .map_err(|_| unavailable())?;
        self.under_object(intent.mutation, authorize, |memory, official| {
            let directory = self.directory(false, authorize)?;
            let prior = lookup(directory.as_deref(), intent.mutation.container)?;
            if let Some(slot) = prior {
                let directory = directory.ok_or_else(unavailable)?;
                self.require_scope(&slot, intent.mutation, None, official.is_some())?;
                if slot.record.intent != *intent {
                    return Err(Failure::Conflict);
                }
                if let Some(receipt) = official {
                    return self.committed(&directory, &slot, &receipt, authorize);
                }
                return match slot.record.phase {
                    Phase::Preparing => self.ready(&directory, &slot, authorize),
                    Phase::Ready { .. } => self.progress(&directory, &slot, authorize),
                };
            }
            if official.is_some() {
                return Err(Failure::Conflict);
            }
            self.register(directory.as_deref(), intent, memory, authorize)
        })
    }

    fn register(
        &self,
        existing: Option<&HeldDir>,
        intent: &Intent,
        memory: &mut Option<State>,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let id = intent.mutation.container;
        let route = self.active_route(intent)?;
        let rooted = route.map(|token| self.enrolled(token)).transpose()?;
        let mut first = State::default();
        let state = match rooted {
            Some(volume) => volume.load(id, memory)?,
            None if memory.is_none() => &mut first,
            None => return Err(invalid("loaded container lost its exact DATA route").into()),
        };
        if !matches!(
            state.plan(intent.mutation, change(intent)),
            Ok(Decision::New(_))
        ) {
            return Err(Failure::Conflict);
        }
        let ledger = journal::Ledger::read(existing)?;
        if ledger
            .records
            .iter()
            .any(|row| row.intent.scope.pool != self.scope.pool)
        {
            return Err(invalid("upload inventory includes another pool").into());
        }
        let volume = select_volume(&self.store.volumes, &ledger, route, intent.body.length)?;
        let record = Record {
            data_token: volume.token,
            intent: intent.clone(),
            phase: Phase::Preparing,
        };
        // No authority to use an orphan, including the same U on another DATA.
        for available in &self.store.volumes {
            material::require_unused(available, &record)?;
        }
        let directory = self.directory(true, authorize)?.ok_or_else(unavailable)?;
        let prove = || {
            self.require_control(&directory)?;
            material::custody(volume)
        };
        let cut = || {
            prove()?;
            authorize()
        };
        let pending = journal::prepare(&directory, None, record, cut)?;
        pending.choose(prove, authorize)?;
        let selected = pending.confirm()?;
        self.ready(&directory, &selected, authorize)
    }

    fn ready(
        &self,
        directory: &HeldDir,
        preparing: &Slot,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let volume = self.original_volume(&preparing.record)?;
        let before = || {
            self.require_control(directory)?;
            preparing.require(directory)?;
            material::custody(volume)?;
            authorize()
        };
        preparing.settle(directory)?;
        before()?;
        let stage = material::Stage::prepare(volume, &preparing.record, before)?;
        let mut next = preparing.record.clone();
        next.phase = stage.phase(0);
        let prove = || {
            self.require_control(directory)?;
            stage.require()
        };
        let cut = || {
            prove()?;
            authorize()
        };
        let writer = journal::prepare(directory, Some(preparing), next, cut)?;
        writer.choose(prove, authorize)?;
        let selected = writer.confirm()?;
        self.require_control(directory)?;
        stage.require()?;
        selected.require(directory)?;
        authorize()?;
        make_progress(&selected.record, false)
    }

    fn status(
        &self,
        mutation: Mutation,
        upload: Option<[u8; 32]>,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let gate = self.gate(mutation, authorize)?;
        let _serial = gate.lock().map_err(|_| unavailable())?;
        self.under_object(mutation, authorize, |_, official| {
            let directory = self.directory(false, authorize)?;
            let prior = lookup(directory.as_deref(), mutation.container)?;
            let Some(slot) = prior.filter(|row| row.record.intent.mutation == mutation) else {
                self.require_authority()?;
                if let Some(directory) = &directory {
                    directory.require_named()?;
                } else {
                    self.system.confirm_absence(NAMESPACE)?;
                }
                authorize()?;
                return Err(if official.is_some() {
                    Failure::Conflict
                } else {
                    Failure::Absent
                });
            };
            let directory = directory.ok_or_else(unavailable)?;
            self.require_scope(&slot, mutation, upload, official.is_some())?;
            match official {
                Some(receipt) => self.committed(&directory, &slot, &receipt, authorize),
                None => self.progress(&directory, &slot, authorize),
            }
        })
    }

    fn committed(
        &self,
        directory: &HeldDir,
        slot: &Slot,
        official: &OperationReceipt,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let intent = &slot.record.intent;
        if !matches!(slot.record.phase, Phase::Ready { .. })
            || !matches!(official, OperationReceipt::Mutation(receipt)
                if receipt.mutation == intent.mutation && receipt.change == change(intent))
        {
            return Err(Failure::Conflict);
        }
        authorize()?;
        slot.settle(directory)?;
        self.require_control(directory)?;
        slot.require(directory)?;
        authorize()?;
        make_progress(&slot.record, true)
    }

    fn progress(
        &self,
        directory: &HeldDir,
        slot: &Slot,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let stage = material::Stage::open(self.original_volume(&slot.record)?, &slot.record)?;
        authorize()?;
        slot.settle(directory)?;
        self.require_control(directory)?;
        stage.require()?;
        slot.require(directory)?;
        authorize()?;
        make_progress(&slot.record, false)
    }

    fn put(
        &self,
        chunk: Chunk,
        input: &mut impl Read,
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        let gate = self.gate(chunk.mutation, authorize)?;
        let (admitted, original) = {
            let _serial = gate.lock().map_err(|_| unavailable())?;
            self.under_chunk(chunk, authorize, |_, slot, stage, _| {
                Ok((slot.record.clone(), stage.retain()))
            })?
        };
        // Never hold upload or selected-container locks over network reads.
        let _read = Reader::take(&self.store.uploads.readers, READERS)?;
        let bytes = read_block(chunk, input, authorize)?;
        let _serial = gate.lock().map_err(|_| unavailable())?;
        self.under_chunk(chunk, authorize, |directory, slot, mut stage, usage| {
            if !same_material(&admitted, &slot.record) {
                return Err(Failure::Conflict);
            }
            original.require(&stage)?;
            self.apply_chunk(
                directory,
                slot,
                &mut stage,
                chunk,
                (&bytes, usage),
                authorize,
            )
        })
    }

    fn under_chunk<T>(
        &self,
        chunk: Chunk,
        authorize: Authorize<'_>,
        action: impl FnOnce(&HeldDir, &Slot, material::Stage<'_>, ChunkUse) -> Result<T>,
    ) -> Result<T> {
        self.under_object(chunk.mutation, authorize, |_, official| {
            if official.is_some() {
                return Err(Failure::Conflict);
            }
            let directory = self.directory(false, authorize)?.ok_or(Failure::Absent)?;
            let slot =
                journal::read(&directory, chunk.mutation.container)?.ok_or(Failure::Absent)?;
            self.require_scope(&slot, chunk.mutation, Some(chunk.upload), false)?;
            let offset = selected_offset(&slot.record)?;
            let usage = chunk_use(
                slot.record.intent.body.length,
                offset,
                chunk.at,
                chunk.length,
            )
            .ok_or(Failure::Conflict)?;
            let stage = material::Stage::open(self.original_volume(&slot.record)?, &slot.record)?;
            authorize()?;
            action(&directory, &slot, stage, usage)
        })
    }

    fn apply_chunk(
        &self,
        directory: &HeldDir,
        prior: &Slot,
        stage: &mut material::Stage<'_>,
        chunk: Chunk,
        (bytes, usage): (&[u8], ChunkUse),
        authorize: Authorize<'_>,
    ) -> Result<Progress> {
        // Even a visible-but-unsynced prior selection has to become durable
        // before another chunk can treat its offset as accepted.
        authorize()?;
        prior.settle(directory)?;
        let ChunkUse::AppendTo(end) = usage else {
            if !stage.same_bytes(chunk.at, bytes, authorize)? {
                return Err(Failure::Conflict);
            }
            self.require_control(directory)?;
            prior.require(directory)?;
            authorize()?;
            return make_progress(&prior.record, false);
        };
        let write_cut = || {
            self.require_control(directory)?;
            prior.require(directory)
        };
        stage.write(chunk.at, end, bytes, write_cut, authorize)?;
        let mut next = prior.record.clone();
        next.phase = stage.phase(end);
        let prove = || {
            self.require_control(directory)?;
            stage.require()
        };
        let cut = || {
            prove()?;
            authorize()
        };
        let writer = journal::prepare(directory, Some(prior), next, cut)?;
        writer.choose(prove, authorize)?;
        let selected = writer.confirm()?;
        self.require_control(directory)?;
        stage.require()?;
        selected.require(directory)?;
        authorize()?;
        make_progress(&selected.record, false)
    }

    fn require_scope(
        &self,
        slot: &Slot,
        mutation: Mutation,
        upload: Option<[u8; 32]>,
        committed: bool,
    ) -> Result<()> {
        let intent = &slot.record.intent;
        if intent.mutation != mutation
            || (match upload {
                Some(expected) => intent.upload_id()? != expected,
                None => false,
            })
        {
            return Err(Failure::Conflict);
        }
        if intent.scope.pool != self.scope.pool {
            return Err(invalid("upload slot belongs to another pool").into());
        }
        if intent.scope.epoch != self.scope.epoch {
            return Err(if committed {
                Failure::Conflict
            } else {
                Failure::OldEpoch
            });
        }
        Ok(())
    }

    fn active_route(&self, intent: &Intent) -> Result<Option<[u8; 32]>> {
        let id = intent.mutation.container;
        if self.store.recycled(id)?.is_some() {
            return Err(Failure::Conflict);
        }
        let route = self.store.routes.get(id)?;
        if route.is_none() && !matches!(intent.target, Target::FirstMain { .. }) {
            return Err(Failure::Conflict);
        }
        Ok(route)
    }

    fn original_volume(&self, record: &Record) -> Result<&DataVolume> {
        if self
            .active_route(&record.intent)?
            .is_some_and(|token| token != record.data_token)
        {
            return Err(Failure::Conflict);
        }
        self.enrolled(record.data_token)
    }

    fn enrolled(&self, token: [u8; 32]) -> Result<&DataVolume> {
        self.store
            .enrolled(token)
            .ok_or_else(|| unavailable().into())
    }
}

fn lookup(directory: Option<&HeldDir>, id: Id) -> io::Result<Option<Slot>> {
    match directory {
        Some(directory) => journal::read(directory, id),
        None => Ok(None),
    }
}

fn selected_offset(record: &Record) -> io::Result<u64> {
    match record.phase {
        Phase::Ready { offset, .. } => Ok(offset),
        Phase::Preparing => Err(unavailable()),
    }
}

fn same_material(old: &Record, now: &Record) -> bool {
    if old.intent != now.intent || old.data_token != now.data_token {
        return false;
    }
    match (old.phase, now.phase) {
        (
            Phase::Ready {
                directory,
                payload,
                offset,
            },
            Phase::Ready {
                directory: new_directory,
                payload: new_payload,
                offset: next,
            },
        ) => directory == new_directory && payload == new_payload && offset <= next,
        _ => false,
    }
}

fn make_progress(record: &Record, committed: bool) -> Result<Progress> {
    let offset = selected_offset(record)?;
    let (status, offset) = if committed {
        (Status::Committed, record.intent.body.length)
    } else if offset == record.intent.body.length {
        (Status::Complete, offset)
    } else {
        (Status::Open, offset)
    };
    Ok(Progress {
        intent: record.intent.clone(),
        upload: record.intent.upload_id()?,
        offset,
        status,
    })
}

fn change(intent: &Intent) -> Change {
    Change::Save {
        target: intent.target.clone(),
        node: intent.node,
        body: intent.body.clone(),
    }
}

fn select_volume<'a>(
    roster: &'a [DataVolume],
    ledger: &journal::Ledger,
    routed: Option<[u8; 32]>,
    length: u64,
) -> io::Result<&'a DataVolume> {
    let mut declared: u64 = 0;
    let mut pending = BTreeMap::<[u8; 32], u64>::new();
    for record in &ledger.records {
        let bytes = record.intent.body.length;
        declared = declared.checked_add(bytes).ok_or_else(capacity)?;
        let offset = if let Phase::Ready { offset, .. } = record.phase {
            offset
        } else {
            0
        };
        let remaining = bytes.checked_sub(offset).ok_or_else(capacity)?;
        let charged = pending.entry(record.data_token).or_default();
        *charged = charged.checked_add(remaining).ok_or_else(capacity)?;
    }
    if ledger.records.len() >= MAX_ACTIVE
        || declared
            .checked_add(length)
            .is_none_or(|n| n > MAX_DECLARED)
    {
        return Err(capacity());
    }
    let needed = length.checked_add(HEADROOM).ok_or_else(capacity)?;
    let mut chosen: Option<(&DataVolume, u64)> = None;
    for volume in roster {
        if routed.is_some_and(|token| token != volume.token) {
            continue;
        }
        let available = match material::available(volume) {
            Ok(bytes) => bytes,
            Err(error) if routed.is_some() => return Err(error),
            Err(_) => continue,
        };
        let available = available.saturating_sub(pending.get(&volume.token).copied().unwrap_or(0));
        if available >= needed
            && chosen.is_none_or(|(best, free)| {
                available > free || available == free && volume.token < best.token
            })
        {
            chosen = Some((volume, available));
        }
    }
    chosen.map(|(volume, _)| volume).ok_or_else(capacity)
}

struct Reader<'a>(&'a AtomicUsize);

impl<'a> Reader<'a> {
    fn take(count: &'a AtomicUsize, limit: usize) -> io::Result<Self> {
        count
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < limit).then_some(n + 1)
            })
            .map_err(|_| unavailable())?;
        Ok(Self(count))
    }
}
impl Drop for Reader<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn read_block(chunk: Chunk, input: &mut impl Read, authorize: Authorize<'_>) -> Result<Vec<u8>> {
    if chunk.length == 0 || chunk.length > CHUNK {
        return Err(Failure::InvalidBody);
    }
    let length = usize::try_from(chunk.length).map_err(|_| Failure::InvalidBody)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    bytes.resize(length, 0);
    let mut digest = Sha256::new();
    for window in bytes.chunks_mut(64 * 1024) {
        let mut remaining = window;
        while !remaining.is_empty() {
            let n = read_some(input, remaining, authorize)?;
            if n == 0 {
                return Err(Failure::InvalidBody);
            }
            digest.update(&remaining[..n])?;
            remaining = &mut remaining[n..];
        }
    }
    authorize()?;
    if read_some(input, &mut [0; 1], authorize)? != 0 || digest.finish() != chunk.sha256 {
        return Err(Failure::InvalidBody);
    }
    authorize()?;
    Ok(bytes)
}

fn read_some(
    input: &mut impl Read,
    bytes: &mut [u8],
    authorize: Authorize<'_>,
) -> io::Result<usize> {
    loop {
        authorize()?;
        match input.read(bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

fn unavailable() -> io::Error {
    io::ErrorKind::NotConnected.into()
}

#[cfg(test)]
#[path = "../../tests/unit/resumable_storage.rs"]
mod tests;
