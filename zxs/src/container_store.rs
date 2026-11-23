//! Complete opaque node saves and metadata selection for individual containers.
mod codec;
mod model;
mod recycle;
mod recycle_material;

use crate::{
    fs::{HeldDir, local_uid},
    hash::Sha256,
};
use model::State;
pub(crate) use model::{Body, Change, Conflict, Id, MAIN, Mutation, Node, NodeId, Receipt, Target};
pub(crate) use recycle::RecycleReceipt;
use std::{
    collections::BTreeMap,
    ffi::CString,
    fmt,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

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

pub(crate) struct Store {
    data: PathBuf,
    staging: PathBuf,
    _owner: File,
    next_stage: AtomicU64,
    // Catalogue lock selects an independent short per-container lock.
    // No upload reads, hashing, or fsync while acquiring this map.
    objects: Mutex<BTreeMap<Id, Arc<Mutex<Option<State>>>>>,
    // Only bootstrap and rename require a shared display-namespace cut.
    names: Mutex<()>,
    data_root: HeldDir,
    data_directory: HeldDir,
    staging_directory: HeldDir,
    archive: Option<recycle::Recycle>,
}

impl Store {
    #[cfg(test)]
    fn open(root: &Path) -> io::Result<Self> {
        Self::open_ordinary(HeldDir::trusted_storage(root)?)
    }

    fn open_ordinary(data_root: HeldDir) -> io::Result<Self> {
        data_root.require_named()?;
        let owner = data_root.opaque_owner(c"owner")?;
        let data_directory = data_root.opaque_subdirectory(c"data")?;
        let staging_directory = data_root.opaque_subdirectory(c"staging")?;
        discard_interrupted_intake(&data_root, &staging_directory, &owner)?;
        let data = data_directory.path.clone();
        let staging = staging_directory.path.clone();
        data_root.require_named()?;
        Ok(Self {
            data,
            staging,
            _owner: owner,
            next_stage: AtomicU64::new(0),
            objects: Mutex::new(BTreeMap::new()),
            names: Mutex::new(()),
            data_root,
            data_directory,
            staging_directory,
            archive: None,
        })
    }

    /// Consume the exact three already-qualified original root FDs. The
    /// caller's deployment lease remains valid during startup; the Store
    /// initializes its own layout under the independent root-owner leases.
    pub(crate) fn open_with_recycle(
        system: HeldDir,
        ordinary: HeldDir,
        recycled: HeldDir,
    ) -> io::Result<Self> {
        recycle::Recycle::require_roles(&system, &recycled, &ordinary)?;
        let mut store = Self::open_ordinary(ordinary)?;
        store.archive = Some(recycle::Recycle::open(system, recycled, &store.data_root)?);
        store.data_held()?;
        Ok(store)
    }

    fn data_held(&self) -> io::Result<()> {
        self.data_root.require_named()?;
        self.data_directory.require_named()?;
        self.staging_directory.require_named()
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
    fn load<'a>(&self, id: Id, memory: &'a mut Option<State>) -> io::Result<&'a mut State> {
        if memory.is_none() {
            // Covers process cuts and same-process post-rename sync errors.
            // Only metadata is read; previous complete node bodies aren't scanned.
            self.staging_directory.file.sync_all()?;
            *memory = Some(read_state(&self.data.join(hex(id)), id)?);
        }
        memory
            .as_mut()
            .ok_or_else(|| invalid("container state unavailable"))
    }
    fn stage(&self) -> io::Result<Stage> {
        self.data_held()?;
        self.stage_at(&self.staging)
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
        target: Target,
        node: NodeId,
        body: Body,
        initial_name: Option<String>,
        mut input: impl Read,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let stage = self.stage()?;
        {
            let mut file = create_file(&stage.path.join("payload"))?;
            let mut digest = Sha256::new();
            let mut left = body.length;
            let mut block = [0; 64 * 1024];
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
        self.publish(
            mutation,
            Change::Save {
                target,
                node,
                body,
                initial_name,
            },
            stage,
            authorize,
        )
    }

    pub(crate) fn change(
        &self,
        mutation: Mutation,
        change: Change,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        if matches!(change, Change::Save { .. }) {
            return Err(invalid("a node must carry the original complete body").into());
        }
        self.publish(mutation, change, self.stage()?, authorize)
    }

    fn publish(
        &self,
        mutation: Mutation,
        change: Change,
        stage: Stage,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<Receipt, Error> {
        let id = mutation.container;
        self.data_held()?;
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            authorize()?;
            return Err(Conflict::MissingContainer.into());
        }
        let state = self.load(id, &mut memory)?;
        match state.replay(mutation, &change) {
            Ok(None) => {}
            result => {
                authorize()?;
                return result?
                    .cloned()
                    .ok_or_else(|| invalid("missing replay").into());
            }
        }
        let receipt = match state.next(mutation, change) {
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
        if let Some(name) = wanted {
            if !self.vacant(id, name)? {
                authorize()?;
                return Err(Conflict::AlreadyExists.into());
            }
        }
        let directory = self.data.join(hex(id));
        ensure_dir(&directory)?;
        // Also covers a prior failed first publication of this directory.
        self.data_held()?;
        self.data_directory.file.sync_all()?;
        let destination = directory.join(entry_leaf(receipt.sequence, receipt.mutation.request));
        match fs::symlink_metadata(&destination) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => return Err(invalid("container receipt position already exists").into()),
        }
        authorize()?;
        self.data_held()?;
        fs::rename(&stage.path, destination)?;
        if let Err(e) = sync_dir(&directory).and_then(|()| self.staging_directory.file.sync_all()) {
            *memory = None;
            return Err(Error::Uncertain(e));
        }
        if let Err(e) = state.select(receipt.clone()) {
            // A durable rename already selected. The next reader must requalify.
            *memory = None;
            return Err(Error::Uncertain(io::Error::other(format!(
                "selected object could not update memory: {e:?}"
            ))));
        }
        Ok(receipt)
    }

    fn vacant(&self, except: Id, name: &str) -> io::Result<bool> {
        for item in fs::read_dir(&self.data)? {
            let item = item?;
            let file_name = item.file_name();
            let id = parse_container_id(file_name.as_encoded_bytes())?;
            if id != except
                && self.recycled(id)?.is_none()
                && read_state(&item.path(), id)?.name.as_deref() == Some(name)
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
        self.data_held()?;
        Ok(self
            .load(mutation.container, &mut memory)?
            .receipts
            .get(&mutation.request)
            .cloned())
    }

    pub(crate) fn describe(&self, id: Id) -> io::Result<Option<Container>> {
        let object = self.object(id)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        if self.recycled(id)?.is_some() {
            return Ok(None);
        }
        self.data_held()?;
        let state = self.load(id, &mut memory)?;
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
        for item in fs::read_dir(&self.data)? {
            let item = item?;
            let name = item.file_name();
            let id = parse_container_id(name.as_encoded_bytes())?;
            if let Some(current) = self.describe(id)? {
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
        let ordinary = &self.data_root;
        if mutation.request == [0; 16] {
            return Err(invalid("zero recycle request identity").into());
        }
        let object = self.object(mutation.container)?;
        let mut memory = object
            .lock()
            .map_err(|_| invalid("container selection unavailable"))?;
        let original = self.data.join(hex(mutation.container));
        authorize()?;
        if let Some((done, at)) = archive.selected(mutation.container)? {
            if (done.mutation, done.original_sequence) != (mutation, expected_sequence) {
                return Err(Conflict::RequestReused.into());
            }
            // The result remains available from SYSTEM and R when F is
            // absent. A caller may separately query the selected receipt.
            if let Err(error) = self.data_held().and_then(|()| {
                let known = read_state(&at, mutation.container)?;
                archive.retire(done, ordinary, &original, &known)
            }) {
                return Err(Error::CleanupPending {
                    receipt: done,
                    error,
                });
            }
            return Ok(done);
        }
        self.data_held()?;
        let state = self.load(mutation.container, &mut memory)?;
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
        let done = archive.prepare(mutation, ordinary, &original, state, &incoming, || {
            authorize()?;
            self.data_held()
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
                self.data_held()
            })?;
        }
        if let Err(error) = archive.retire(done, ordinary, &original, state) {
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
        self.data_held()?;
        let state = self.load(id, &mut memory)?;
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
        self.data_held()?;
        let state = self.load(id, &mut memory)?;
        let Some(node) = state.nodes.get(&node).cloned() else {
            return Ok(None);
        };
        let entry = self
            .data
            .join(hex(id))
            .join(entry_leaf(node.sequence, node.request));
        private_dir(&entry)?;
        let file = open_file(&entry.join("payload"))?;
        if file.metadata()?.len() != node.body.length {
            return Err(invalid(
                "complete node length differs from its durable receipt",
            ));
        }
        Ok(Some(NodeBytes { file, node }))
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

/// The ordinary owner lease excludes new intake during this cold cleanup.
/// Recognize only the flat staging format; selected data and recycle roots
/// are not traversed. Never follow an unexpected member or read file bytes.
fn discard_interrupted_intake(root: &HeldDir, staging: &HeldDir, owner: &File) -> io::Result<()> {
    let owned = owner.metadata()?;
    let require_base = || {
        root.require_named()?;
        staging.require_named()?;
        let named = root
            .opaque_pin(c"owner")?
            .ok_or_else(|| invalid("intake owner lease lost its name"))?;
        let seen = named.file.metadata()?;
        if (owned.dev(), owned.ino()) != (seen.dev(), seen.ino())
            || !root.shares_file_mount(owner)?
        {
            return Err(invalid("intake owner lease was replaced"));
        }
        Ok(())
    };
    require_base()?;
    // Enumerate the still-held staging directory on Linux, not its former
    // pathname. The borrowed File outlives the streamed enumeration.
    let held_path = format!("/proc/self/fd/{}", staging.file.as_raw_fd());
    let mut changed = false;
    for item in fs::read_dir(held_path)? {
        let name = item?.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes.len() != 16 || !bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            continue;
        }
        let leaf = CString::new(bytes).map_err(|_| invalid("invalid stage name"))?;
        let candidate = || {
            let Some(stage) = staging.child_directory(&leaf)? else {
                return Ok::<_, io::Error>(None);
            };
            if stage
                .names()?
                .iter()
                .any(|name| !matches!(name.as_slice(), b"payload" | b"receipt"))
            {
                return Ok(None);
            }
            // Prove the whole flat shape before unlinking either member.
            let payload = stage.opaque_pin(c"payload")?;
            let receipt = stage.opaque_pin(c"receipt")?;
            Ok(Some((stage, payload, receipt)))
        };
        let qualified = candidate();
        // Candidate-local foreign layouts (including symlinks, unsupported
        // mounts and non-regular members) are left alone. A displaced base
        // or original owner record always invalidates the entire cleanup.
        require_base()?;
        let Ok(Some((stage, payload, receipt))) = qualified else {
            continue;
        };
        if let Some(pin) = payload.as_ref() {
            stage.unlink_opaque_original(c"payload", pin)?;
        }
        if let Some(pin) = receipt.as_ref() {
            stage.unlink_opaque_original(c"receipt", pin)?;
        }
        staging.unlink_empty_opaque_original(&leaf, &stage)?;
        changed = true;
    }
    if changed {
        staging.file.sync_all()?;
    }
    require_base()
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
    for (byte, pair) in id.iter_mut().zip(name.chunks_exact(2)) {
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
        bytes.extend_from_slice(&(tag.len() as u32).to_be_bytes());
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
