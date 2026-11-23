//! Complete opaque node saves and metadata selection for individual containers.
mod codec;
mod model;
mod recycle;
mod recycle_material;
mod routes;
mod volume;

use crate::{
    fs::{HeldDir, local_uid},
    hash::Sha256,
};
use model::State;
pub(crate) use model::{Body, Change, Conflict, Id, MAIN, Mutation, Node, NodeId, Receipt, Target};
pub(crate) use recycle::RecycleReceipt;
use routes::Routes;
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use volume::DataVolume;

const TAG_BYTES: usize = 1024 * 1024;

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
    ) -> io::Result<Self> {
        recycle::Recycle::require_roles(&system, &recycled, &data)?;
        let owner = system.opaque_owner(c"container-owner")?;
        let mut store = Self::open_roster(&system, data, owner)?;
        store.archive = Some(recycle::Recycle::open(system, recycled)?);
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

    fn routed(&self, id: Id) -> io::Result<Option<&DataVolume>> {
        self.routes
            .get(id)?
            .map(|token| {
                self.volumes
                    .binary_search_by_key(&token, |volume| volume.token)
                    .map(|i| &self.volumes[i])
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::NotConnected, "selected DATA is not enrolled")
                    })
            })
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

    fn recycled(&self, id: Id) -> io::Result<Option<(RecycleReceipt, PathBuf)>> {
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
        Ok(objects.entry(id).or_default().clone())
    }
    fn stage(&self, volume: &DataVolume) -> io::Result<Stage> {
        volume.require_named()?;
        self.stage_at(volume.staging())
    }
    fn stage_at(&self, root: &Path) -> io::Result<Stage> {
        loop {
            let n = self.next_stage.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("{n:016x}"));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Stage { path }),
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
        let stage = self.stage(volume)?;
        {
            let mut file = create_file(&stage.path.join("payload"))?;
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
        self.publish(mutation, change, volume, &stage, authorize)
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
        self.publish(mutation, change, volume, &self.stage(volume)?, authorize)
    }

    fn publish(
        &self,
        mutation: Mutation,
        change: Change,
        volume: &DataVolume,
        stage: &Stage,
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
        write_file(&stage.path.join("receipt"), &codec::encode(&receipt)?)?;
        sync_dir(&stage.path)?;

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
        let destination = directory
            .path
            .join(entry_leaf(receipt.sequence, receipt.mutation.request));
        match fs::symlink_metadata(&destination) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => return Err(invalid("container receipt position already exists").into()),
        }
        volume.require_named()?;
        directory.require_named()?;
        authorize()?;
        fs::rename(&stage.path, destination)?;
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

    fn describe_at(&self, id: Id, recorded: bool) -> io::Result<Option<Container>> {
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
        Ok(state.name.as_ref().map(|name| Container {
            id,
            name: name.clone(),
            tags: state.tags.clone(),
            sequence: state.sequence,
            heads: state.branches.clone(),
        }))
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
            // The result remains available from SYSTEM and R when F is
            // absent. A caller may separately query the selected receipt.
            if let Err(error) = self.recorded(mutation.container).and_then(|volume| {
                let known = read_state(&at, mutation.container)?;
                archive.retire(done, volume, &known)
            }) {
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
        let incoming = self.stage_at(archive.staging()?)?;
        let done = archive.prepare(mutation, volume, state, &incoming, || {
            authorize()?;
            volume.require_named()
        })?;
        let selector = self.stage_at(archive.moves()?)?;
        // Only one small SYSTEM rename switches logical visibility. The
        // ordinary namespace guard contains no streaming/hash work.
        {
            let _names = self
                .names
                .lock()
                .map_err(|_| invalid("container name selection unavailable"))?;
            archive.choose(done, &selector, || {
                authorize()?;
                volume.require_named()
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
        let entry = container.path.join(entry_leaf(node.sequence, node.request));
        private_dir(&entry)?;
        let file = open_file(&entry.join("payload"))?;
        if file.metadata()?.len() != node.body.length {
            return Err(invalid(
                "complete node length differs from its durable receipt",
            ));
        }
        Ok(Some(NodeBytes { node, file }))
    }
}

fn entry_leaf(sequence: u64, request: Id) -> String {
    format!("{sequence:016x}-{}", hex(request))
}

fn read_state(path: &Path, id: Id) -> io::Result<State> {
    match private_dir(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
        result => result?,
    }
    // Required before exposing any receipt/node after a prior process died.
    sync_dir(path)?;
    read_selected_entries(path, id)
}

fn read_selected_entries(path: &Path, id: Id) -> io::Result<State> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| invalid("invalid container receipt path"))?;
        let (sequence, request) = generation_name(name)?;
        entries.push((
            sequence,
            parse_container_id(request.as_bytes())?,
            item.path(),
        ));
    }
    entries.sort_unstable_by_key(|entry| entry.0);
    let mut state = State::default();
    for (sequence, request, entry) in entries {
        private_dir(&entry)?;
        let mut bytes = Vec::new();
        open_file(&entry.join("receipt"))?
            .take((codec::MAX_RECEIPT + 1) as u64)
            .read_to_end(&mut bytes)?;
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

struct Stage {
    path: PathBuf,
}
impl Drop for Stage {
    fn drop(&mut self) {
        // Own only the staging directory created by this invocation.
        // Never scan/remove a prior application object or another request.
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn read(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match input.read(buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            value => return value,
        }
    }
}
fn private_dir(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != local_uid() || m.mode() & 0o077 != 0 {
        return Err(invalid(
            "container storage directory must be private and service-owned",
        ));
    }
    Ok(())
}
fn private_file(path: &Path, file: &File) -> io::Result<()> {
    let named = fs::symlink_metadata(path)?;
    let opened = file.metadata()?;
    if !named.is_file()
        || named.dev() != opened.dev()
        || named.ino() != opened.ino()
        || opened.uid() != local_uid()
        || opened.mode() & 0o077 != 0
        || opened.nlink() != 1
    {
        return Err(invalid(
            "container storage or metadata file is not its original private file",
        ));
    }
    Ok(())
}
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
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
fn create_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
fn open_file(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    private_file(path, &file)?;
    Ok(file)
}
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn hex(id: [u8; 16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(32);
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
