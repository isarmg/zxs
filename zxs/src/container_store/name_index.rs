//! Bounded projection of the authoritative active names. No on-disk index.
mod fast;
use super::{
    Change, Id, Receipt, State, Store, Target, codec, invalid, ledger_limit,
    operation::require_archived_ledger, parse_container_id, read_confirmed, recycle::IndexRoles,
    recycle_receipt::RecycleReceipt, replay_with, unenrolled_data,
};
use crate::{
    fs::{Entity, HeldDir, ReadAccess, local_uid},
    hash::Sha256,
};
pub(super) use fast::{Background, ChangeCut, Clock, Names, Ticket};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    sync::Arc,
};

const ACCESS: ReadAccess = ReadAccess::Ordinary;
const MAX_ROLES: usize = 64;

struct Limits {
    rows: usize,
    labels: usize,
    ledger_rows: usize,
    ledger_bytes: usize,
    ledger_tags: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: 1 << 17,
            labels: 32 << 20,
            ledger_rows: 1 << 16,
            ledger_bytes: 32 << 20,
            ledger_tags: 1 << 18,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Live([u64; 3]);

impl Live {
    fn capture(at: &HeldDir) -> io::Result<Self> {
        let value = Self(at.live_identity()?);
        let meta = at.file.metadata()?;
        if meta.uid() != local_uid()
            || meta.mode() & 0o7777 != 0o700
            || (meta.dev(), meta.ino()) != (value.0[0], value.0[1])
            || value.0[2] == 0
        {
            return Err(invalid("name view lost a private held directory"));
        }
        Ok(value)
    }

    fn entity(self) -> Entity {
        Entity {
            device: self.0[0],
            inode: self.0[1],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Directory {
    live: Live,
    nlink: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Directory {
    fn capture(at: &HeldDir) -> io::Result<Self> {
        let live = Live::capture(at)?;
        let meta = at.file.metadata()?;
        Ok(Self {
            live,
            nlink: meta.nlink(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Role {
    root: Live,
    owner: (u64, u64),
}

impl Role {
    fn capture(at: &HeldDir, name: &CStr, original: &File) -> io::Result<Self> {
        let root = Live::capture(at)?;
        let named = at
            .opaque_pin(name)?
            .ok_or_else(|| invalid("name view owner is absent"))?;
        let only = named.file.metadata()?;
        let meta = original.metadata()?;
        let owner = (meta.dev(), meta.ino());
        if only.len() != 0
            || owner != (only.dev(), only.ino())
            || !at.shares_file_mount(original)?
        {
            return Err(invalid("name view owner was replaced"));
        }
        named.require(at, name)?;
        Ok(Self { root, owner })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DataFence {
    token: [u8; 32],
    role: Role,
    data: Directory,
    staging: Live,
}

impl DataFence {
    fn same_scope(&self, other: &Self) -> bool {
        self.token == other.token
            && self.role == other.role
            && self.data.live == other.data.live
            && self.staging == other.staging
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecycleFence {
    pool: Id,
    token: [u8; 32],
    role: Role,
    arrived: Directory,
    staging: Live,
}

impl RecycleFence {
    fn capture(parts: &IndexRoles<'_>, system: Live) -> io::Result<Self> {
        if Live::capture(parts.system)? != system {
            return Err(invalid("name view has a different SYSTEM for recycle"));
        }
        Ok(Self {
            pool: parts.role.pool,
            token: parts.role.token,
            role: Role::capture(parts.root, c"container-owner", parts.owner)?,
            arrived: Directory::capture(parts.arrived)?,
            staging: Live::capture(parts.staging)?,
        })
    }

    fn same_scope(&self, other: &Self) -> bool {
        (self.pool, self.token) == (other.pool, other.token)
            && self.role == other.role
            && self.arrived.live == other.arrived.live
            && self.staging == other.staging
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Fence {
    system: Role,
    routes: Directory,
    retired: Option<Directory>,
    moves: Option<Directory>,
    data: Vec<DataFence>,
    recycle: Option<RecycleFence>,
}

impl Fence {
    fn capture(store: &Store, system: &HeldDir) -> io::Result<Self> {
        Self::sample(store, system, None)
    }

    fn sample(store: &Store, system: &HeldDir, required: Option<&Required>) -> io::Result<Self> {
        if store.volumes.len() + usize::from(store.archive.is_some()) >= MAX_ROLES {
            return Err(capacity());
        }
        store.routes.require_bound_parent(system)?;
        let authority = Role::capture(system, c"container-owner", &store.system_owner)?;
        let routes = Directory::capture(store.routes.name_namespace())?;
        let retired = system
            .child_directory(c"retired-data")?
            .as_ref()
            .map(Directory::capture)
            .transpose()?;
        let data = store
            .volumes
            .iter()
            .filter(|volume| required.is_none_or(|focus| focus.data.contains(&volume.token)))
            .map(|volume| {
                Ok(DataFence {
                    token: volume.token,
                    role: Role::capture(volume.root(), c"owner", volume.bound_owner())?,
                    data: Directory::capture(volume.contents())?,
                    staging: Live::capture(volume.staging())?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let moves = store
            .archive
            .as_ref()
            .map(|archive| Directory::capture(archive.index_moves()?))
            .transpose()?;
        let recycle = store
            .archive
            .as_ref()
            .filter(|_| required.is_none_or(|focus| focus.recycle))
            .map(|archive| RecycleFence::capture(&archive.index_roles()?, authority.root))
            .transpose()?;
        if moves.is_none() && system.child_directory(c"container-moves")?.is_some() {
            return Err(invalid(
                "name view lacks the enrolled recycle selector authority",
            ));
        }
        Ok(Self {
            system: authority,
            routes,
            retired,
            moves,
            data,
            recycle,
        })
    }

    fn same_scope(&self, other: &Self) -> bool {
        let live = |original: Option<&Directory>| original.map(|directory| directory.live);
        self.system == other.system
            && self.routes.live == other.routes.live
            && live(self.retired.as_ref()) == live(other.retired.as_ref())
            && live(self.moves.as_ref()) == live(other.moves.as_ref())
            && self.data.len() == other.data.len()
            && self
                .data
                .iter()
                .zip(&other.data)
                .all(|(left, right)| left.same_scope(right))
            && match (self.recycle.as_ref(), other.recycle.as_ref()) {
                (None, None) => true,
                (Some(left), Some(right)) => left.same_scope(right),
                _ => false,
            }
    }
}

pub(super) fn active_label(receipt: &Receipt) -> Option<&str> {
    match &receipt.change {
        Change::Save {
            target: Target::FirstMain { name },
            ..
        }
        | Change::Rename { name, .. } => Some(name),
        _ => None,
    }
}

#[derive(Default)]
struct Required {
    data: BTreeSet<[u8; 32]>,
    recycle: bool,
}

#[derive(Clone, Copy, Debug)]
struct Anchor {
    sequence: u64,
    operation: Id,
    digest: [u8; 32],
}

impl Anchor {
    fn new(receipt: &Receipt) -> io::Result<Self> {
        Ok(Self {
            sequence: receipt.sequence,
            operation: receipt.mutation.request,
            digest: receipt_digest(&codec::encode(receipt)?)?,
        })
    }

    fn require(self, receipt: &Receipt) -> io::Result<()> {
        if (receipt.sequence, receipt.mutation.request) != (self.sequence, self.operation)
            || receipt_digest(&codec::encode(receipt)?)? != self.digest
        {
            return Err(invalid("the observed durable name receipt changed"));
        }
        Ok(())
    }
}

fn receipt_digest(bytes: &[u8]) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(bytes)?;
    Ok(digest.finish())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ledger {
    sequence: u64,
    digest: [u8; 32],
}

struct Replay {
    tags_left: usize,
    digest: [u8; 32],
    prior: Option<Ledger>,
}

impl Replay {
    fn new(id: Id, prior: Option<Ledger>, tags_left: usize) -> io::Result<Self> {
        let mut digest = Sha256::new();
        digest.update(b"ZXS name ledger v1\0")?;
        digest.update(&id)?;
        Ok(Self {
            tags_left,
            digest: digest.finish(),
            prior,
        })
    }

    fn receipt(&mut self, sequence: u64, raw: &[u8]) -> io::Result<super::Receipt> {
        let mut digest = Sha256::new();
        digest.update(b"ZXS name ledger receipt v1\0")?;
        digest.update(&self.digest)?;
        digest.update(&(raw.len() as u64).to_be_bytes())?;
        digest.update(raw)?;
        self.digest = digest.finish();
        if self
            .prior
            .is_some_and(|old| old.sequence == sequence && old.digest != self.digest)
        {
            return Err(invalid("observed name ledger prefix changed"));
        }
        codec::decode_charging(raw, || {
            self.tags_left = self.tags_left.checked_sub(1).ok_or_else(ledger_limit)?;
            Ok(())
        })
    }

    fn selected(&self, state: &State) -> io::Result<Ledger> {
        if self.prior.is_some_and(|old| state.sequence < old.sequence) {
            return Err(invalid("observed name ledger moved backwards"));
        }
        Ok(Ledger {
            sequence: state.sequence,
            digest: self.digest,
        })
    }
}

fn selected_ledger(
    at: &HeldDir,
    id: Id,
    previous: Option<&Place>,
    limits: &Limits,
) -> io::Result<(State, Ledger, Live)> {
    let before = Directory::capture(at)?;
    let mut replay = Replay::new(id, previous.and_then(Place::ledger), limits.ledger_tags)?;
    let state = read_confirmed(at, || {
        replay_with(
            at,
            id,
            ACCESS,
            (limits.ledger_rows, limits.ledger_bytes),
            |sequence, raw| replay.receipt(sequence, raw),
        )
    })?;
    if Directory::capture(at)? != before {
        return Err(invalid("selected ledger changed during the cold name scan"));
    }
    let witness = replay.selected(&state)?;
    if let Some(anchor) = previous.and_then(Place::anchor) {
        anchor.require(
            state
                .receipts
                .get(&anchor.operation)
                .ok_or_else(|| invalid("the observed durable name receipt disappeared"))?,
        )?;
    }
    Ok((state, witness, before.live))
}

#[derive(Debug)]
enum Place {
    Reserved(Option<Live>),
    Active {
        name: Arc<str>,
        at: Live,
        ledger: Option<Ledger>, // absent for a fallback-only observed rename
        warm: Option<Anchor>,
    },
    Recycled {
        receipt: RecycleReceipt,
        at: Live,
        ledger: Option<Ledger>,
        warm: Option<Anchor>,
    },
}

impl Place {
    fn ledger(&self) -> Option<Ledger> {
        match self {
            Self::Active { ledger, .. } | Self::Recycled { ledger, .. } => *ledger,
            Self::Reserved(_) => None,
        }
    }

    fn anchor(&self) -> Option<Anchor> {
        match self {
            Self::Active { warm, .. } | Self::Recycled { warm, .. } => *warm,
            Self::Reserved(_) => None,
        }
    }

    fn require_progress(&self, old: &Self, token: [u8; 32]) -> io::Result<()> {
        let source = |receipt: RecycleReceipt, at: Live| receipt.require_source(token, at.entity());
        let valid = match (old, self) {
            (Self::Reserved(None), _) => true,
            (Self::Reserved(Some(prior)), Self::Reserved(Some(at)) | Self::Active { at, .. })
            | (Self::Active { at: prior, .. }, Self::Active { at, .. }) => prior == at,
            (
                Self::Reserved(Some(at)) | Self::Active { at, .. },
                Self::Recycled { receipt, .. },
            ) => {
                source(*receipt, *at)?;
                true
            }
            (
                Self::Recycled {
                    receipt: prior,
                    at: old,
                    ..
                },
                Self::Recycled { receipt, at, .. },
            ) => receipt == prior && at == old,
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(invalid(
                "an observed container selection was lost or replaced",
            ))
        }
    }
}

#[derive(Debug)]
struct Row {
    token: [u8; 32],
    place: Place,
}

fn observe(
    store: &Store,
    id: Id,
    token: [u8; 32],
    previous: Option<&Row>,
    limits: &Limits,
) -> io::Result<Place> {
    if previous.is_some_and(|old| old.token != token) {
        return Err(invalid("observed container route changed"));
    }
    let prior = previous.map(|old| &old.place);
    let place = if let Some((receipt, at)) = store.recycled(id)? {
        receipt.require_data(token)?;
        if store.enrolled(token).is_none()
            && !store
                .archive
                .as_ref()
                .ok_or_else(unenrolled_data)?
                .retired_data(token)?
        {
            return Err(unenrolled_data());
        }
        #[cfg(test)]
        store.name_clock.count_ledger();
        let (state, ledger, at) = selected_ledger(&at, id, prior, limits)?;
        require_archived_ledger(&state, &receipt)?;
        Place::Recycled {
            receipt,
            at,
            ledger: Some(ledger),
            warm: None,
        }
    } else {
        let volume = store.enrolled(token).ok_or_else(unenrolled_data)?;
        match volume.held_container(id)? {
            None => Place::Reserved(None),
            Some(at) => {
                #[cfg(test)]
                store.name_clock.count_ledger();
                let (state, ledger, at) = selected_ledger(&at, id, prior, limits)?;
                if let Some(name) = state.name {
                    Place::Active {
                        name: Arc::from(name),
                        at,
                        ledger: Some(ledger),
                        warm: None,
                    }
                } else {
                    Place::Reserved(Some(at))
                }
            }
        }
    };
    if let Some(old) = previous {
        place.require_progress(&old.place, token)?;
    }
    Ok(place)
}

fn capacity() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "bounded cold name view exceeds capacity",
    )
}

fn check_data_membership(
    store: &Store,
    rows: &BTreeMap<Id, Row>,
    mut remaining: usize,
) -> io::Result<()> {
    let mut expected = rows
        .values()
        .filter(|row| matches!(row.place, Place::Active { .. } | Place::Reserved(Some(_))))
        .count();
    for volume in &store.volumes {
        volume.contents().each_name(ACCESS, |raw| {
            remaining = remaining.checked_sub(1).ok_or_else(capacity)?;
            let id = parse_container_id(raw)?;
            let row = rows
                .get(&id)
                .filter(|row| row.token == volume.token)
                .ok_or_else(|| invalid("DATA contains a misplaced or unrouted container"))?;
            let source = volume
                .held_container(id)?
                .ok_or_else(|| invalid("DATA position changed during scan"))?;
            match &row.place {
                Place::Active { at, .. } | Place::Reserved(Some(at))
                    if *at == Live::capture(&source)? =>
                {
                    expected = expected
                        .checked_sub(1)
                        .ok_or_else(|| invalid("DATA member repeated during scan"))?;
                    Ok(())
                }
                Place::Recycled { receipt, .. } => receipt.require_source(row.token, source.entity),
                _ => Err(invalid("DATA position changed during scan")),
            }
        })?;
    }
    if expected == 0 {
        Ok(())
    } else {
        Err(invalid("observed DATA position disappeared"))
    }
}

fn check_selector_membership(
    store: &Store,
    rows: &BTreeMap<Id, Row>,
    max: usize,
) -> io::Result<()> {
    let Some(archive) = store.archive.as_ref() else {
        return Ok(());
    };
    let mut stages = max;
    let mut selected = rows
        .values()
        .filter(|row| matches!(row.place, Place::Recycled { .. }))
        .count();
    archive.index_roles()?.moves.each_name(ACCESS, |raw| {
        let private = raw.len() == 16 && raw.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if private {
            stages = stages.checked_sub(1).ok_or_else(capacity)?;
            return Ok(()); // Old small selector staging is never selected here.
        }
        selected = selected
            .checked_sub(1)
            .ok_or_else(|| invalid("SYSTEM selector lacks its qualified recycled route"))?;
        let id = parse_container_id(raw)?;
        if rows
            .get(&id)
            .is_none_or(|row| !matches!(row.place, Place::Recycled { .. }))
        {
            return Err(invalid(
                "SYSTEM selector lacks its qualified recycled route",
            ));
        }
        Ok(())
    })?;
    if selected == 0 {
        Ok(())
    } else {
        Err(invalid("observed SYSTEM selector disappeared"))
    }
}

#[derive(Debug)]
struct View {
    fence: Option<Fence>,
    rows: BTreeMap<Id, Row>,
    names: BTreeMap<Arc<str>, Id>,
    label_bytes: usize,
}

impl View {
    fn partial() -> Self {
        Self {
            fence: None,
            rows: BTreeMap::new(),
            names: BTreeMap::new(),
            label_bytes: 0,
        }
    }

    #[cfg(test)]
    fn build(
        store: &Store,
        system: &HeldDir,
        previous: Option<&Self>,
        limits: &Limits,
        before_confirm: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        // The only write-lock ordering here is names then short SYSTEM/R
        // cuts. Never enter another container's object lock under names.
        let _names = store
            .names
            .lock()
            .map_err(|_| invalid("container name selection unavailable"))?;
        Self::scan(store, system, previous, limits, before_confirm)
    }

    // Caller holds names and never enters another object's lock.
    fn scan(
        store: &Store,
        system: &HeldDir,
        previous: Option<&Self>,
        limits: &Limits,
        before_confirm: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        let fence = Fence::capture(store, system)?;
        if previous
            .and_then(|old| old.fence.as_ref())
            .is_some_and(|old| !fence.same_scope(old))
        {
            return Err(invalid("name view belongs to a different held authority"));
        }
        let mut rows = BTreeMap::new();
        let mut names = BTreeMap::new();
        let mut labels = limits.labels;
        #[cfg(test)]
        store.name_clock.count_routes();
        for id in store.routes.index_ids(limits.rows)? {
            let token = store
                .routes
                .get(id)?
                .ok_or_else(|| invalid("route vanished during name scan"))?;
            let place = observe(
                store,
                id,
                token,
                previous.and_then(|old| old.rows.get(&id)),
                limits,
            )?;
            if let Place::Active { name, .. } = &place {
                labels = labels.checked_sub(name.len()).ok_or_else(capacity)?;
                if names.insert(Arc::clone(name), id).is_some() {
                    return Err(invalid("duplicate active name in selected catalogue"));
                }
            }
            if rows.insert(id, Row { token, place }).is_some() {
                return Err(invalid("route repeated during name scan"));
            }
        }
        if previous.is_some_and(|old| old.rows.keys().any(|id| !rows.contains_key(id))) {
            return Err(invalid("an observed route disappeared from SYSTEM"));
        }
        check_data_membership(store, &rows, limits.rows)?;
        check_selector_membership(store, &rows, limits.rows)?;
        before_confirm()?;
        if fence != Fence::capture(store, system)? {
            return Err(invalid(
                "a common namespace changed during the cold name scan",
            ));
        }
        Ok(Self {
            fence: Some(fence),
            rows,
            names,
            label_bytes: limits.labels - labels,
        })
    }

    /// Pure projection. The caller, not this map, proves current authority.
    fn vacant(&self, except: Id, name: &str) -> bool {
        self.names
            .get(name)
            .is_none_or(|selected| *selected == except)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/name_index.rs"]
mod tests;
