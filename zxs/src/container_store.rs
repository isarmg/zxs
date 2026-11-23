//! Complete opaque node saves and metadata selection for individual containers.
mod codec;
mod model;
mod recycle;
mod recycle_material;
mod routes;
mod stage;
mod volume;

#[cfg(test)]
use crate::fs::local_uid;
use crate::{fs::HeldDir, hash::Sha256};
use model::State;
pub(crate) use model::{Body, Change, Conflict, Id, MAIN, Mutation, Node, NodeId, Receipt, Target};
pub(crate) use recycle::RecycleReceipt;
use routes::Routes;
use stage::FlatStage;
use std::{
    collections::BTreeMap,
    ffi::CString,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use volume::DataVolume;

#[cfg(test)]
use std::{
    fs::{DirBuilder, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const TAG_BYTES: usize = 1024 * 1024;
const OBJECT_SLOT_TARGET: usize = 128;

pub(crate) enum Error {
    Io(io::Error),
    Conflict(Conflict),
    WrongLength,
    WrongDigest,
    Uncertain(io::Error),
    CleanupPending {
        receipt: RecycleReceipt,
        error: io::Error,
    },
}

// A local diagnostic can name the disposition and error kind without
// rendering arbitrary I/O details or the selected receipt.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => f.debug_tuple("Io").field(&error.kind()).finish(),
            Self::Conflict(reason) => f.debug_tuple("Conflict").field(reason).finish(),
            Self::WrongLength => f.write_str("WrongLength"),
            Self::WrongDigest => f.write_str("WrongDigest"),
            Self::Uncertain(error) => f.debug_tuple("Uncertain").field(&error.kind()).finish(),
            Self::CleanupPending { error, .. } => f
                .debug_struct("CleanupPending")
                .field("kind", &error.kind())
                .finish(),
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<Conflict> for Error {
    fn from(value: Conflict) -> Self {
        Self::Conflict(value)
    }
}

pub(crate) struct Container {
    pub(crate) id: Id,
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) sequence: u64,
    pub(crate) heads: BTreeMap<String, Option<NodeId>>,
}

pub(crate) const CATALOG_PAGE_ROWS: usize = 256;

pub(crate) struct CatalogRow {
    pub(crate) id: Id,
    pub(crate) sequence: u64,
    pub(crate) name: String,
}

pub(crate) struct CatalogPage {
    pub(crate) items: Vec<CatalogRow>,
    pub(crate) next_after: Option<Id>,
}

fn catalog_rows(
    sorted: impl IntoIterator<Item = Id>,
    after: Option<Id>,
    readable: impl Fn(Id) -> bool,
    mut project: impl FnMut(Id) -> io::Result<Option<CatalogRow>>,
) -> io::Result<CatalogPage> {
    let mut items = Vec::with_capacity(CATALOG_PAGE_ROWS);
    for id in sorted {
        if after.is_some_and(|bound| id <= bound) || !readable(id) {
            continue;
        }
        if let Some(row) = project(id)? {
            items.push(row);
            if items.len() == CATALOG_PAGE_ROWS {
                return Ok(CatalogPage {
                    items,
                    next_after: Some(id),
                });
            }
        }
    }
    Ok(CatalogPage {
        items,
        next_after: None,
    })
}

pub(crate) const HISTORY_PAGE_ROWS: usize = 256;

pub(crate) struct HistoryPage {
    pub(crate) head: NodeId,
    pub(crate) nodes: Vec<(NodeId, Node)>,
    pub(crate) next: Option<NodeId>,
}

fn select_history_page(
    state: &State,
    branch: &str,
    head: NodeId,
    next: Option<NodeId>,
) -> Result<HistoryPage, Error> {
    let current = *state.branches.get(branch).ok_or(Conflict::MissingBranch)?;
    if current != Some(head) {
        return Err(Conflict::HeadChanged(current).into());
    }
    if next == Some(head) {
        return Err(Conflict::MissingNode.into());
    }
    let first = next.unwrap_or(head);
    let mut at = Some(head);
    let mut previous = None;
    let mut emitted = false;
    let mut nodes = Vec::with_capacity(HISTORY_PAGE_ROWS);
    while let Some(id) = at {
        let node = state
            .nodes
            .get(&id)
            .ok_or_else(|| invalid("branch refers to a missing node"))?;
        if node.sequence == 0
            || node.sequence > state.sequence
            || previous.is_some_and(|upper| node.sequence >= upper)
        {
            return Err(invalid("branch ancestry is not strictly descending").into());
        }
        emitted |= id == first;
        if emitted {
            if nodes.len() == HISTORY_PAGE_ROWS {
                return Ok(HistoryPage {
                    head,
                    nodes,
                    next: Some(id),
                });
            }
            nodes.push((id, node.clone()));
        }
        previous = Some(node.sequence);
        at = node.prior;
    }
    if !emitted {
        return Err(Conflict::MissingNode.into());
    }
    Ok(HistoryPage {
        head,
        nodes,
        next: None,
    })
}

pub(crate) const DETAIL_PAGE_ROWS: usize = 256;

pub(crate) struct DetailPage {
    pub(crate) id: Id,
    pub(crate) sequence: u64,
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) heads: Vec<(String, NodeId)>,
    pub(crate) next_after: Option<String>,
}

fn select_detail_page(
    id: Id,
    name: &str,
    state: &State,
    sequence: Option<u64>,
    after: Option<&str>,
) -> Result<DetailPage, Error> {
    use std::ops::Bound::{Excluded, Unbounded};

    if sequence.is_some_and(|sequence| sequence != state.sequence) {
        return Err(Conflict::SequenceChanged(state.sequence).into());
    }
    let mut heads = Vec::with_capacity(DETAIL_PAGE_ROWS);
    for (branch, node) in state
        .branches
        .range::<str, _>((after.map_or(Unbounded, Excluded), Unbounded))
        .take(DETAIL_PAGE_ROWS)
    {
        let node = node.ok_or_else(|| invalid("selected branch has no head"))?;
        heads.push((branch.clone(), node));
    }
    let next_after = heads
        .last()
        .filter(|_| heads.len() == DETAIL_PAGE_ROWS)
        .map(|(branch, _)| branch.clone());
    Ok(DetailPage {
        id,
        sequence: state.sequence,
        name: name.to_owned(),
        tags: state.tags.clone(),
        heads,
        next_after,
    })
}

pub(crate) struct NodeBytes {
    pub(crate) node: Node,
    pub(crate) file: File,
}

pub(crate) struct DataRoot {
    pub(crate) token: [u8; 32],
    pub(crate) root: HeldDir,
}

pub(crate) struct Store {
    volumes: Vec<DataVolume>,
    routes: Routes,
    next_stage: AtomicU64,
    // Catalogue lock selects an independent short per-container lock.
    // No upload reads, hashing, or fsync while acquiring this map.
    objects: Mutex<BTreeMap<Id, Arc<Mutex<Option<State>>>>>,
    // Only bootstrap and rename require a shared display-namespace cut.
    names: Mutex<()>,
    archive: Option<recycle::Recycle>,
    _system_owner: File,
}

impl Store {
    /// Startup supplies the already-qualified original roots.
    pub(crate) fn open_with_recycle(
        system: HeldDir,
        data: Vec<DataRoot>,
        recycled: HeldDir,
        pool: Id,
    ) -> io::Result<Self> {
        recycle::Recycle::require_roles(&system, &recycled, &data)?;
        let owner = system.opaque_owner(c"container-owner")?;
        let mut store = Self::open_roster(&system, data, owner)?;
        store.archive = Some(recycle::Recycle::open(system, recycled, pool)?);
        Ok(store)
    }

    fn open_roster(system: &HeldDir, mut data: Vec<DataRoot>, owner: File) -> io::Result<Self> {
        data.sort_unstable_by_key(|volume| volume.token);
        if data.is_empty()
            || data[0].token == [0; 32]
            || data.windows(2).any(|pair| pair[0].token == pair[1].token)
        {
            return Err(invalid("invalid enrolled DATA roster"));
        }

        let routes = Routes::open_existing(system)?;
        let fresh = match &routes {
            Some(routes) => match routes.ids()?.first() {
                Some(id) => {
                    routes
                        .get(*id)?
                        .ok_or_else(|| invalid("selected route vanished at startup"))?;
                    false
                }
                None => true,
            },
            None => true,
        };
        if fresh && let Some(old) = system.child_directory(c"container-moves")? {
            old.only(None)?;
        }
        // An absent route layout is a new enrollment, not a legacy import.
        // Check every DATA namespace under its own owner before creating it.
        let volumes = data
            .into_iter()
            .map(|root| DataVolume::open(root, fresh))
            .collect::<io::Result<Vec<_>>>()?;
        let routes = match routes {
            Some(routes) => routes,
            None => Routes::open(system)?,
        };
        Ok(Self {
            volumes,
            routes,
            next_stage: AtomicU64::new(0),
            objects: Mutex::new(BTreeMap::new()),
            names: Mutex::new(()),
            archive: None,
            _system_owner: owner,
        })
    }

    fn enrolled(&self, token: [u8; 32]) -> Option<&DataVolume> {
        self.volumes
            .binary_search_by_key(&token, |volume| volume.token)
            .ok()
            .map(|i| &self.volumes[i])
    }

    fn routed(&self, id: Id) -> io::Result<Option<&DataVolume>> {
        self.routes
            .get(id)?
            .map(|token| self.enrolled(token).ok_or_else(unenrolled_data))
            .transpose()
    }

    fn write_volume(
        &self,
        id: Id,
        first: bool,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<&DataVolume, Error> {
        if self.recycled(id)?.is_none() {
            if let Some(volume) = self.routed(id)? {
                return Ok(volume);
            }
            if first {
                // The roster is nonempty and sorted by stable disk token.
                // Only an ID with no route can be assigned an intake root.
                return Ok(&self.volumes[usize::from(id[0]) % self.volumes.len()]);
            }
        }
        authorize()?;
        Err(Conflict::MissingContainer.into())
    }

    fn recycled(&self, id: Id) -> io::Result<Option<(RecycleReceipt, HeldDir)>> {
        self.archive
            .as_ref()
            .map_or(Ok(None), |archive| archive.selected(id))
    }

    fn object(&self, id: Id) -> io::Result<Arc<Mutex<Option<State>>>> {
        if id == [0; 16] {
            return Err(invalid("zero container identity"));
        }
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| invalid("container catalogue unavailable"))?;
        if let Some(active) = objects.get(&id) {
            return Ok(Arc::clone(active));
        }
        // Only the registry can issue a new strong reference. A slot owned
        // solely by the registry has no in-flight reader or writer.
        while objects.len() >= OBJECT_SLOT_TARGET {
            let idle = objects
                .iter()
                .find_map(|(id, slot)| (Arc::strong_count(slot) == 1).then_some(*id));
            let Some(id) = idle else { break };
            objects.remove(&id);
        }
        Ok(objects.entry(id).or_default().clone())
    }
    fn stage<'a>(&self, volume: &'a DataVolume) -> io::Result<FlatStage<'a>> {
        volume.require_named()?;
        self.flat_stage_at(volume.staging())
    }

    fn flat_stage_at<'a>(&self, root: &'a HeldDir) -> io::Result<FlatStage<'a>> {
        loop {
            let n = self
                .next_stage
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| invalid("staging name counter exhausted"))?;
            let name =
                CString::new(format!("{n:016x}")).map_err(|_| invalid("invalid stage name"))?;
            match FlatStage::new(root, name) {
                Ok(stage) => return Ok(stage),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// One data call validates and durably selects one complete node.
    /// Its body reader must end at this request. No payload parsing or second copy.
    pub(crate) fn save(
        &self,
        mutation: Mutation,
        change: Change,
        mut input: impl Read,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let Change::Save {
            body, initial_name, ..
        } = &change
        else {
            return Err(invalid("a node requires a complete save operation").into());
        };
        let volume = self.write_volume(mutation.container, initial_name.is_some(), &authorize)?;
        let mut stage = self.stage(volume)?;
        {
            let file = stage.create(c"payload")?;
            let mut digest = Sha256::new();
            let mut left = body.length;
            let mut block = vec![0; 64 * 1024];
            while left != 0 {
                let take = usize::try_from(left.min(block.len() as u64))
                    .map_err(|_| invalid("invalid node intake window"))?;
                let n = read(&mut input, &mut block[..take])?;
                if n == 0 {
                    return Err(Error::WrongLength);
                }
                digest.update(&block[..n])?;
                file.write_all(&block[..n])?;
                left -= n as u64;
            }
            if read(&mut input, &mut block[..1])? != 0 {
                return Err(Error::WrongLength);
            }
            if digest.finish() != body.sha256 {
                return Err(Error::WrongDigest);
            }
            file.sync_all()?;
        }
        self.publish(mutation, change, volume, &mut stage, authorize)
    }

    pub(crate) fn change(
        &self,
        mutation: Mutation,
        change: Change,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        if matches!(change, Change::Save { .. }) {
            return Err(invalid("a node must carry the original complete body").into());
        }
        let volume = self.write_volume(mutation.container, false, &authorize)?;
        self.publish(
            mutation,
            change,
            volume,
            &mut self.stage(volume)?,
            authorize,
        )
    }

    fn publish(
        &self,
        mutation: Mutation,
        change: Change,
        volume: &DataVolume,
        stage: &mut FlatStage<'_>,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let id = mutation.container;
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            authorize()?;
            return Err(Conflict::MissingContainer.into());
        }
        volume.require_named()?;
        let reserved = match self.routes.get(id)? {
            Some(token) if token == volume.token => true,
            None if memory.is_none() => false,
            _ => return Err(invalid("DATA route changed during intake").into()),
        };
        let mut first = State::default();
        let selected = if reserved {
            volume.load(id, &mut memory)?
        } else {
            &mut first
        };
        match selected.replay(mutation, &change) {
            Ok(None) => {}
            result => {
                authorize()?;
                return result?
                    .cloned()
                    .ok_or_else(|| invalid("missing replay").into());
            }
        }
        let receipt = match selected.next(mutation, change) {
            Ok(receipt) => receipt,
            Err(e) => {
                authorize()?;
                return Err(e.into());
            }
        };
        stage.write(c"receipt", &codec::encode(&receipt)?)?;
        stage.directory.file.sync_all()?;

        let wanted = match &receipt.change {
            Change::Save { initial_name, .. } => initial_name.as_deref(),
            Change::Rename { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let _namespace = wanted
            .map(|_| {
                self.names
                    .lock()
                    .map_err(|_| invalid("container name selection unavailable"))
            })
            .transpose()?;
        if let Some(name) = wanted
            && !self.vacant(id, name)?
        {
            authorize()?;
            return Err(Conflict::AlreadyExists.into());
        }
        let directory = if !reserved {
            volume.require_vacant(id)?;
            authorize()?;
            self.routes.reserve(id, volume.token)?;
            volume.create_container(id)?
        } else if let Some(original) = volume.held_container(id)? {
            original
        } else if receipt.sequence == 1 {
            // The SYSTEM reservation can survive an interrupted first save.
            volume.create_container(id)?
        } else {
            return Err(invalid("recorded DATA container is absent").into());
        };
        if receipt.sequence == 1 {
            directory.only(None)?;
        }
        // Also covers a prior failed first publication of this directory.
        volume.sync_data()?;
        let destination = receipt_name(&receipt)?;
        volume.require_named()?;
        directory.require_named()?;
        stage.publish_directory(&directory, &destination, authorize)?;
        if let Err(e) = directory
            .file
            .sync_all()
            .and_then(|()| volume.sync_staging())
        {
            *memory = None;
            return Err(Error::Uncertain(e));
        }
        if let Err(e) = selected.select(receipt.clone()) {
            // A durable rename already selected. The next reader must requalify.
            *memory = None;
            return Err(Error::Uncertain(io::Error::other(format!(
                "selected object could not update memory: {e:?}"
            ))));
        }
        if !reserved {
            *memory = Some(first);
        }
        Ok(receipt)
    }

    fn recorded(&self, id: Id) -> io::Result<&DataVolume> {
        self.routed(id)?
            .ok_or_else(|| invalid("selected container route is absent"))
    }

    fn vacant(&self, except: Id, name: &str) -> io::Result<bool> {
        for id in self.routes.ids()? {
            if id != except
                && self.recycled(id)?.is_none()
                && self.recorded(id)?.read(id)?.name.as_deref() == Some(name)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn receipt(&self, mutation: Mutation) -> io::Result<Option<Receipt>> {
        let object = self.object(mutation.container)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if let Some((_receipt, at)) = self.recycled(mutation.container)? {
            return Ok(read_state(&at, mutation.container)?
                .receipts
                .get(&mutation.request)
                .cloned());
        }
        let Some(volume) = self.routed(mutation.container)? else {
            return Ok(None);
        };
        Ok(volume
            .load(mutation.container, &mut memory)?
            .receipts
            .get(&mutation.request)
            .cloned())
    }

    pub(crate) fn describe(&self, id: Id) -> io::Result<Option<Container>> {
        self.describe_at(id, false)
    }

    pub(crate) fn detail_page(
        &self,
        id: Id,
        sequence: Option<u64>,
        after: Option<&str>,
    ) -> Result<Option<DetailPage>, Error> {
        self.project_container(id, false, |state, name| {
            select_detail_page(id, name, state, sequence, after)
        })?
        .transpose()
    }

    fn describe_at(&self, id: Id, recorded: bool) -> io::Result<Option<Container>> {
        self.project_container(id, recorded, |state, name| Container {
            id,
            name: name.to_owned(),
            tags: state.tags.clone(),
            sequence: state.sequence,
            heads: state.branches.clone(),
        })
    }

    fn project_container<T>(
        &self,
        id: Id,
        recorded: bool,
        project: impl FnOnce(&State, &str) -> T,
    ) -> io::Result<Option<T>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        let volume = if recorded {
            self.recorded(id)?
        } else {
            let Some(volume) = self.routed(id)? else {
                return Ok(None);
            };
            volume
        };
        let state = volume.load(id, &mut memory)?;
        Ok(state.name.as_deref().map(|name| project(state, name)))
    }
    pub(crate) fn list(&self) -> io::Result<Vec<Container>> {
        let mut items = Vec::new();
        for id in self.routes.ids()? {
            if let Some(current) = self.describe_at(id, true)? {
                items.push(current);
            }
        }
        items.sort_unstable_by_key(|item| item.id);
        Ok(items)
    }

    pub(crate) fn catalog_page(
        &self,
        after: Option<Id>,
        readable: impl Fn(Id) -> bool,
    ) -> io::Result<CatalogPage> {
        catalog_rows(self.routes.ids()?, after, readable, |id| {
            self.project_container(id, true, |state, name| CatalogRow {
                id,
                sequence: state.sequence,
                name: name.to_owned(),
            })
        })
    }

    pub(crate) fn recycle_receipt(&self, mutation: Mutation) -> io::Result<Option<RecycleReceipt>> {
        let object = self.object(mutation.container)?;
        let _selection = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        Ok(self
            .recycled(mutation.container)?
            .map(|(r, _)| r)
            .filter(|r| r.mutation == mutation))
    }

    /// Recycle an entire selected container, not a branch or a node.
    pub(crate) fn move_to_recycle(
        &self,
        mutation: Mutation,
        expected_sequence: u64,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<RecycleReceipt, Error> {
        let archive = self
            .archive
            .as_ref()
            .ok_or_else(|| invalid("no qualified recycle storage"))?;
        if mutation.request == [0; 16] {
            return Err(invalid("zero recycle request identity").into());
        }
        let object = self.object(mutation.container)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        authorize()?;
        if let Some((done, at)) = archive.selected(mutation.container)? {
            if (done.mutation, done.original_sequence) != (mutation, expected_sequence) {
                return Err(Conflict::RequestReused.into());
            }
            if let Err(error) = self.finish_recycle(archive, done, &at) {
                return Err(Error::CleanupPending {
                    receipt: done,
                    error,
                });
            }
            return Ok(done);
        }
        let Some(volume) = self.routed(mutation.container)? else {
            return Err(Conflict::MissingContainer.into());
        };
        let state = volume.load(mutation.container, &mut memory)?;
        if state.name.is_none() {
            return Err(Conflict::MissingContainer.into());
        }
        if state.receipts.contains_key(&mutation.request) {
            return Err(Conflict::RequestReused.into());
        }
        if state.sequence != expected_sequence {
            return Err(Conflict::SequenceChanged(state.sequence).into());
        }
        let mut incoming = self.flat_stage_at(archive.staging()?)?;
        let done = archive.prepare(mutation, volume, state, &mut incoming, || {
            authorize()?;
            volume.require_named()
        })?;
        let mut selector = self.flat_stage_at(archive.moves()?)?;
        // Only one small SYSTEM rename switches logical visibility. The
        // ordinary namespace guard contains no streaming/hash work.
        {
            let _names = self
                .names
                .lock()
                .map_err(|_| invalid("container name selection unavailable"))?;
            archive.choose(done, &mut selector, || {
                volume.require_named()?;
                authorize()
            })?;
        }
        if let Err(error) = archive.retire(done, volume, state) {
            return Err(Error::CleanupPending {
                receipt: done,
                error,
            });
        }
        *memory = None;
        Ok(done)
    }

    fn finish_recycle(
        &self,
        archive: &recycle::Recycle,
        done: RecycleReceipt,
        at: &HeldDir,
    ) -> io::Result<()> {
        let id = done.mutation.container;
        let token = self
            .routes
            .get(id)?
            .ok_or_else(|| invalid("selected container route is absent"))?;
        if let Some(volume) = self.enrolled(token) {
            return archive.retire(done, volume, &read_state(at, id)?);
        }
        if !archive.retired_data(token)? {
            return Err(unenrolled_data());
        }
        archive.require_selected(done, &read_state(at, id)?)
    }

    /// Immutable ancestry of the selected branch, newest node first.
    /// This does not read any payload bytes.
    pub(crate) fn branch_nodes(
        &self,
        id: Id,
        branch: &str,
    ) -> io::Result<Option<Vec<(NodeId, Node)>>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        let Some(volume) = self.routed(id)? else {
            return Ok(None);
        };
        let state = volume.load(id, &mut memory)?;
        let Some(mut next) = state.branches.get(branch).copied() else {
            return Ok(None);
        };
        let mut out = Vec::new();
        let mut upper = None;
        while let Some(id) = next {
            let exact = state
                .nodes
                .get(&id)
                .ok_or_else(|| invalid("branch refers to a missing node"))?;
            if upper.is_some_and(|latest| exact.sequence >= latest) {
                return Err(invalid(
                    "branch ancestry did not advance toward the original node",
                ));
            }
            upper = Some(exact.sequence);
            next = exact.prior;
            out.push((id, exact.clone()));
        }
        Ok(Some(out))
    }

    /// Verify the incoming continuation by walking back from this branch's
    /// exact still-selected head. Only the bounded page is cloned.
    pub(crate) fn branch_history(
        &self,
        id: Id,
        branch: &str,
        head: NodeId,
        next: Option<NodeId>,
    ) -> Result<Option<HistoryPage>, Error> {
        self.project_container(id, false, |state, _| {
            select_history_page(state, branch, head, next)
        })?
        .transpose()
    }

    pub(crate) fn node(&self, id: Id, node: NodeId) -> io::Result<Option<NodeBytes>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        let Some(volume) = self.routed(id)? else {
            return Ok(None);
        };
        let state = volume.load(id, &mut memory)?;
        let Some(node) = state.nodes.get(&node).cloned() else {
            return Ok(None);
        };
        let container = volume
            .held_container(id)?
            .ok_or_else(|| invalid("selected DATA container is absent"))?;
        let position = entry_leaf(node.sequence, node.request);
        let entry = held_entry(&container, &position)?;
        let payload = entry
            .opaque_pin(c"payload")?
            .ok_or_else(|| invalid("selected node payload is absent"))?;
        let file = payload.file;
        if file.metadata()?.len() != node.body.length {
            return Err(invalid(
                "complete node length differs from its durable receipt",
            ));
        }
        Ok(Some(NodeBytes { node, file }))
    }
}

fn receipt_name(receipt: &Receipt) -> io::Result<CString> {
    CString::new(entry_leaf(receipt.sequence, receipt.mutation.request))
        .map_err(|_| invalid("invalid container receipt name"))
}

fn entry_leaf(sequence: u64, request: Id) -> String {
    format!("{sequence:016x}-{}", hex(request))
}

fn held_entry(original: &HeldDir, leaf: &str) -> io::Result<HeldDir> {
    let native = CString::new(leaf).map_err(|_| invalid("invalid original receipt position"))?;
    original
        .child_directory(&native)?
        .ok_or_else(|| invalid("selected original node directory is absent"))
}

fn read_state(directory: &HeldDir, id: Id) -> io::Result<State> {
    directory.require_named()?;
    // Required before exposing any receipt/node after a prior process died.
    directory.file.sync_all()?;
    let state = read_selected_entries(directory, id)?;
    directory.require_named()?;
    Ok(state)
}

fn read_selected_entries(directory: &HeldDir, id: Id) -> io::Result<State> {
    let mut entries = Vec::new();
    // Enumerate the already-held directory without imposing a generation cap.
    let held_path = format!("/proc/self/fd/{}", directory.file.as_raw_fd());
    for item in fs::read_dir(held_path)? {
        let name = item?.file_name();
        let name = name
            .into_string()
            .map_err(|_| invalid("invalid container receipt path"))?;
        let (sequence, request) = generation_name(&name)?;
        entries.push((sequence, parse_container_id(request.as_bytes())?, name));
    }
    entries.sort_unstable_by_key(|entry| entry.0);
    let mut state = State::default();
    for (sequence, request, name) in entries {
        let entry = held_entry(directory, &name)?;
        let original = entry
            .opaque_pin(c"receipt")?
            .ok_or_else(|| invalid("stored receipt is absent"))?;
        let mut bytes = Vec::new();
        (&original.file)
            .take((codec::MAX_RECEIPT + 1) as u64)
            .read_to_end(&mut bytes)?;
        original.require(&entry, c"receipt")?;
        if bytes.len() > codec::MAX_RECEIPT {
            return Err(invalid("stored receipt exceeds metadata capacity"));
        }
        let receipt = codec::decode(&bytes)?;
        if receipt.mutation
            != (Mutation {
                container: id,
                request,
            })
            || receipt.sequence != sequence
        {
            return Err(invalid(
                "selected receipt differs from its recorded position",
            ));
        }
        state.select(receipt).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid selected history: {e:?}"),
            )
        })?;
    }
    Ok(state)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn unenrolled_data() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "selected DATA is not enrolled")
}
fn read(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match input.read(buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            value => return value,
        }
    }
}
#[cfg(test)]
fn private_dir(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != local_uid() || m.mode() & 0o077 != 0 {
        return Err(invalid(
            "container storage directory must be private and service-owned",
        ));
    }
    Ok(())
}
#[cfg(test)]
fn ensure_dir(path: &Path) -> io::Result<bool> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            private_dir(path)?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}
#[cfg(test)]
fn create_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
#[cfg(test)]
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn hex<const N: usize>(id: [u8; N]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 * N);
    for b in id {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
    out
}
fn parse_container_id(name: &[u8]) -> io::Result<Id> {
    if name.len() != 32 {
        return Err(invalid("invalid container directory identifier"));
    }
    let digit = |b| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(invalid("invalid container directory identifier")),
    };
    let mut id = [0; 16];
    for (byte, pair) in id.iter_mut().zip(name.as_chunks::<2>().0) {
        *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    if id == [0; 16] {
        return Err(invalid("invalid zero container directory identifier"));
    }
    Ok(id)
}
fn generation_name(leaf: &str) -> io::Result<(u64, &str)> {
    let (generation, request) = leaf
        .split_once('-')
        .ok_or_else(|| invalid("invalid selected container entry"))?;
    let lower_hex = |bytes: &[u8]| bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if generation.len() != 16
        || request.len() != 32
        || !lower_hex(generation.as_bytes())
        || !lower_hex(request.as_bytes())
    {
        return Err(invalid("invalid selected container entry"));
    }
    let generation =
        u64::from_str_radix(generation, 16).map_err(|_| invalid("invalid selected generation"))?;
    if generation == 0 {
        return Err(invalid("invalid selected generation"));
    }
    Ok((generation, request))
}
fn encode_tags(tags: &[String]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for tag in tags {
        if tag.is_empty()
            || tag.contains('\0')
            || 4 + tag.len() > TAG_BYTES.saturating_sub(bytes.len())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid tag or excessive tag metadata",
            ));
        }
        let length = u32::try_from(tag.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "tag length exceeds its storage field",
            )
        })?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(tag.as_bytes());
    }
    Ok(bytes)
}
fn decode_tags(mut bytes: &[u8]) -> io::Result<Vec<String>> {
    let mut tags = Vec::new();
    while !bytes.is_empty() {
        let (length, suffix) = bytes
            .split_at_checked(4)
            .ok_or_else(|| invalid("truncated tag length"))?;
        let length = u32::from_be_bytes(
            length
                .try_into()
                .map_err(|_| invalid("truncated tag length"))?,
        ) as usize;
        let (text, rest) = suffix
            .split_at_checked(length)
            .ok_or_else(|| invalid("truncated tag"))?;
        let tag = std::str::from_utf8(text).map_err(|_| invalid("invalid tag encoding"))?;
        if tag.is_empty() || tag.contains('\0') {
            return Err(invalid("invalid stored tag"));
        }
        tags.push(tag.to_owned());
        bytes = rest;
    }
    Ok(tags)
}

#[cfg(test)]
#[path = "../tests/unit/container_nodes.rs"]
mod tests;
