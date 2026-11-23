//! First, offline headless setup on explicitly selected empty mounted directories.
//! The optional caller supplies the approved selection and one confirmed password.
use crate::{fresh_password, fresh_random::headless as random, maintenance::{DiscoveryView, HostMounts}, pinned::{Entity, Mount, Pinned}};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use rustix::{
    fs::{FlockOperation, Mode, OFlags, RawDir, RenameFlags, ResolveFlags, fchmod, flock, openat, openat2, renameat_with},
    process::{getegid, geteuid, getuid},
};
use std::{fs::File, io::{self, Read, Write}, mem::MaybeUninit, os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path, sync::Arc};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

#[path = "headless_reset.rs"]
mod reset;
#[path = "headless_import_preflight.rs"]
mod import_preflight;
pub(crate) use import_preflight::Preflight;

pub(crate) const DEFAULT_STATE: &str = "/zxs/state/headless";
const DESCRIPTOR: &str = "server.v1";
const FORMAT: &[u8] = b"ZXS-HEADLESS-1\0";
const MARKER: &str = ".zxs-role-v1";
const MARK_MAGIC: &[u8] = b"ZXS-ROLE-V1\0";
const MARK_BYTES: usize = MARK_MAGIC.len() + 1 + 16 + 32;

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "fresh headless deployment or disk identity was not proved")
}

struct HeldInput<'a> {
    pub(crate) original: &'a Pinned,
    pub(crate) directory: &'a File,
}

struct RootInput<'a> {
    pub(crate) held: HeldInput<'a>,
    pub(crate) token: [u8; 32],
}

struct Issue<'a> {
    pub(crate) state: HeldInput<'a>,
    pub(crate) roots: [RootInput<'a>; 3], // SYSTEM, data, recycle
    pub(crate) pool: [u8; 16],
    pub(crate) storage_uid: u32,
    pub(crate) proxy_uid: u32,
    pub(crate) login: &'a str,
}

#[derive(Debug)]
pub(crate) enum IssueError {
    Refused(io::Error),
    /// The exact original role marker or descriptor may exist. Preserve it;
    /// never overwrite or infer that a partial setup may be retried on new roots.
    Uncertain(io::Error),
}
impl From<io::Error> for IssueError {
    fn from(error: io::Error) -> Self { Self::Refused(error) }
}

fn require_held(input: &HeldInput<'_>, uid: u32) -> io::Result<(Entity, Mount)> {
    input.original.admit_held(input.directory)?;
    let m = input.directory.metadata()?;
    let entity = Entity::of(&m)?;
    let mount = Mount::on(input.directory)?.ok_or_else(refused)?;
    if !m.is_dir() || m.uid() != uid || m.nlink() == 0 || m.mode() & 0o7777 != 0o700
        || input.original.entity() != entity || input.original.require_same_named_mount()? != mount
    { return Err(refused()); }
    Ok((entity, mount))
}

fn role_proof(input: &RootInput<'_>, role: u8, pool: &[u8;16], uid: u32) -> io::Result<(Entity, Mount)> {
    let root = require_held(&input.held, uid)?;
    let open = || openat2(input.held.directory, MARKER,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOATIME | OFlags::CLOEXEC, Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV);
    let mut mark = File::from(open()?);
    let m = mark.metadata()?;
    if !m.is_file() || m.uid() != uid || m.nlink() != 1
        || m.mode() & 0o7777 != 0o600 || m.len() != MARK_BYTES as u64
    { return Err(refused()); }
    let mut expected = [0; MARK_BYTES];
    mark.read_exact(&mut expected)?;
    let bytes = expected.strip_prefix(MARK_MAGIC).ok_or_else(refused)?;
    if bytes[0] != role || bytes[1..17] != pool[..] || bytes[17..] != input.token
        || Entity::of(&File::from(open()?).metadata()?)? != Entity::of(&m)?
        || root != require_held(&input.held, uid)?
    { return Err(refused()); }
    Ok(root)
}

impl Issue<'_> {
    fn require_roots(&self, marked: bool) -> io::Result<()> {
        let uid = self.storage_uid;
        if self.state.original.path() != Path::new(DEFAULT_STATE)
            || matches!(uid, 0 | 65534 | u32::MAX)
            || matches!(self.proxy_uid, 0 | 65534 | u32::MAX)
            || self.proxy_uid == uid || self.pool == [0;16]
        { return Err(refused()); }
        let (state, _) = require_held(&self.state, uid)?;
        let mut held = Vec::new();
        let state_path = self.state.original.path();
        for (i, root) in self.roots.iter().enumerate() {
            let path = root.held.original.path();
            let bytes = path.as_os_str().as_bytes();
            let absolute = bytes.strip_prefix(b"/").ok_or_else(refused)?;
            if bytes.len() > 4095 || bytes.contains(&0)
                || absolute.split(|b| *b == b'/').any(|part| matches!(part, b"" | b"." | b".."))
                || state_path.starts_with(path) || path.starts_with(state_path)
                || root.token == [0;32]
                || self.roots[..i].iter().any(|old| {
                    let original = old.held.original.path();
                    old.token == root.token || path.starts_with(original) || original.starts_with(path)
                        || old.held.original.entity_is_visible_ancestor_of(root.held.original)
                        || root.held.original.entity_is_visible_ancestor_of(old.held.original)
                })
            { return Err(refused()); }
            let actual = if marked {
                role_proof(root, u8::try_from(i).map_err(|_| refused())?, &self.pool, uid)?
            } else { require_held(&root.held, uid)? };
            if actual.0 == state || held.iter().any(|&(entity, mount): &(Entity, Mount)| {
                entity == actual.0 || i == 2 && (entity.dev == actual.0.dev || mount.id == actual.1.id)
            }) { return Err(refused()); }
            held.push(actual);
        }
        Ok(())
    }

    fn publish_locked(&self, dir: &File, password: &str) -> Result<[u8;16], IssueError> {
        if !fresh_password::admissible(password) || !login_ok(self.login) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid fresh headless account").into());
        }
        if Entity::of(&dir.metadata()?)? != require_held(&self.state, self.storage_uid)?.0 {
            return Err(refused().into());
        }
        self.require_roots(false)?;
        require_empty(dir)?;
        for root in &self.roots { require_empty(root.held.directory)?; }

        let epoch = random::<16>()?;
        if epoch == self.pool { return Err(refused().into()); }
        let salt = random::<16>()?;
        let tag = fresh_verifier(password, &salt)?;

        let mut encoded = Zeroizing::new(Vec::new());
        encoded.extend_from_slice(FORMAT);
        encoded.extend_from_slice(&self.pool);
        encoded.extend_from_slice(&epoch);
        encoded.extend_from_slice(&self.proxy_uid.to_be_bytes());
        encoded.extend_from_slice(&salt);
        encoded.extend_from_slice(&tag[..]);
        encoded.push(u8::try_from(self.login.len()).map_err(|_| refused())?);
        encoded.extend_from_slice(self.login.as_bytes());
        for root in &self.roots {
            let path = root.held.original.path().as_os_str().as_bytes();
            encoded.extend_from_slice(&u16::try_from(path.len()).map_err(|_| refused())?.to_be_bytes());
            encoded.extend_from_slice(path);
            encoded.extend_from_slice(&root.token);
        }

        // From the first marker write onward, any failure preserves the exact
        // original evidence. Core cannot start without the final descriptor.
        let persist = || -> io::Result<()> {
            self.require_roots(false)?;
            require_empty(dir)?;
            for root in &self.roots { require_empty(root.held.directory)?; }
            for (index, root) in self.roots.iter().enumerate() {
                let role = u8::try_from(index).map_err(|_| refused())?;
                let original = require_held(&root.held, self.storage_uid)?;
                let mut marker = [0; MARK_BYTES];
                marker[..MARK_MAGIC.len()].copy_from_slice(MARK_MAGIC);
                marker[MARK_MAGIC.len()] = role;
                marker[MARK_MAGIC.len() + 1..MARK_MAGIC.len() + 17].copy_from_slice(&self.pool);
                marker[MARK_MAGIC.len() + 17..].copy_from_slice(&root.token);
                publish_file(root.held.directory, ".role-stage", MARKER, &marker, self.storage_uid, || {
                    if require_held(&root.held, self.storage_uid)? != original { return Err(refused()); }
                    Ok(())
                })?;
                role_proof(root, role, &self.pool, self.storage_uid)?;
            }
            self.require_roots(true)?;
            publish_file(dir, &stage_name(&epoch), DESCRIPTOR, &encoded, self.storage_uid, || {
                require_held(&self.state, self.storage_uid)?;
                self.require_roots(true)
            })
        };
        persist().map_err(IssueError::Uncertain)?;
        Ok(epoch)
    }
}

fn require_empty(dir: &File) -> io::Result<()> {
    let reader = File::from(openat2(dir, ".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(), ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV)?);
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut scan = RawDir::new(&reader, &mut buf);
    while let Some(item) = scan.next() {
        if !matches!(item?.file_name().to_bytes(), b"." | b"..") {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "fresh setup requires empty original directories"));
        }
    }
    Ok(())
}

fn publish_file(dir: &File, stage: &str, final_name: &str, bytes: &[u8], uid: u32,
    still_selected: impl Fn() -> io::Result<()>) -> io::Result<()> {
    let scoped = ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV;
    let mut file = File::from(openat2(dir, stage,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC, Mode::RUSR | Mode::WUSR, scoped)?);
    fchmod(&file, Mode::RUSR | Mode::WUSR)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let expected = file.metadata()?;
    let entity = Entity::of(&expected)?;
    if !expected.is_file() || expected.uid() != uid || expected.nlink() != 1
        || expected.mode() & 0o7777 != 0o600 || expected.len() != bytes.len() as u64 {
        return Err(refused());
    }
    let named = |name: &str| openat2(dir, name, OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty(), scoped).map(File::from);
    let mut check = named(stage)?;
    if Entity::of(&check.metadata()?)? != entity { return Err(refused()); }
    let mut raw = Zeroizing::new(vec![0; bytes.len()]);
    check.read_exact(&mut raw)?;
    if raw.as_slice() != bytes { return Err(refused()); }
    still_selected()?;
    renameat_with(dir, stage, dir, final_name, RenameFlags::NOREPLACE)?;
    dir.sync_all()?;
    if Entity::of(&named(final_name)?.metadata()?)? != entity { return Err(refused()); }
    still_selected()
}

struct Held {
    original: Pinned,
    directory: File,
}
impl Held {
    fn select(path: &Path, storage_uid: u32, scope: &Arc<DiscoveryView>) -> io::Result<Self> {
        let original = Pinned::local_storage(path, scope)?;
        original.same_local_chain(|file, _| {
            let m = file.metadata()?;
            if !(m.uid() == 0 || m.uid() == storage_uid) || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0) {
                return Err(refused());
            }
            Ok(())
        })?;
        let directory = original.storage_directory()?;
        let out = Self { original, directory };
        require_held(&out.input(), storage_uid)?;
        Ok(out)
    }

    fn input(&self) -> HeldInput<'_> {
        HeldInput { original: &self.original, directory: &self.directory }
    }
}

/// Read-only selection for a trusted offline setup UI. No disk formatting,
/// daemon, privileged account creation, service start or gateway enrollment.
pub(crate) struct FreshSelection {
    state: Held,
    roots: [Held; 3],
    scope: Arc<DiscoveryView>,
    uid: u32,
    ipc_gid: u32,
    gateway_uid: u32,
    pool: [u8; 16],
    tokens: [[u8; 32]; 3],
}

impl FreshSelection {
    pub(crate) fn select(paths: [&Path; 3], gateway_uid: u32, ipc_gid: u32) -> io::Result<Self> {
        let uid = geteuid().as_raw();
        if getuid().as_raw() != uid || matches!(uid, 0 | 65534 | u32::MAX)
            || matches!(ipc_gid, 0 | 65534 | u32::MAX) || getegid().as_raw() != ipc_gid
            || gateway_uid == uid || matches!(gateway_uid, 0 | 65534 | u32::MAX) {
            return Err(refused());
        }
        let scope = Arc::new(HostMounts::discovery_view(false)?);
        let selected = Self {
            state: Held::select(Path::new(DEFAULT_STATE), uid, &scope)?,
            roots: [Held::select(paths[0], uid, &scope)?, Held::select(paths[1], uid, &scope)?, Held::select(paths[2], uid, &scope)?],
            scope, uid, ipc_gid, gateway_uid, pool: random::<16>()?,
            tokens: [random::<32>()?, random::<32>()?, random::<32>()?],
        };
        selected.require_site()?;
        Ok(selected)
    }

    /// The UI presents this original ordered selection, not replacement paths.
    pub(crate) fn paths(&self) -> [&Path; 3] {
        self.roots.each_ref().map(|entry| entry.original.path())
    }

    fn require_site(&self) -> io::Result<()> {
        self.scope.require_unchanged()?;
        for name in ["/zxs", "/zxs/bin", "/zxs/state"] {
            let dir = Pinned::local_storage(Path::new(name), &self.scope)?;
            let m = dir.metadata()?;
            if m.uid() != 0 || m.mode() & 0o022 != 0 { return Err(refused()); }
            dir.require_same_named_mount()?;
        }
        let parent = Pinned::local_storage(Path::new("/zxs/run"), &self.scope)?;
        let m = parent.metadata()?;
        if m.uid() != self.uid || m.gid() != self.ipc_gid || m.mode() & 0o7777 != 0o710 {
            return Err(refused());
        }
        parent.require_same_named_mount()?;
        Ok(())
    }

    fn issue<'a>(&'a self, login: &'a str) -> Issue<'a> {
        Issue {
            state: self.state.input(),
            roots: [
                RootInput { held: self.roots[0].input(), token: self.tokens[0] },
                RootInput { held: self.roots[1].input(), token: self.tokens[1] },
                RootInput { held: self.roots[2].input(), token: self.tokens[2] },
            ],
            pool: self.pool, storage_uid: self.uid, proxy_uid: self.gateway_uid, login,
        }
    }

    /// Read-only observation; the same objects are revalidated under the
    /// offline lease before changing anything.
    pub(crate) fn check_ready(&self) -> io::Result<Readiness> {
        self.require_site()?;
        self.issue("").require_roots(false)?;
        for held in std::iter::once(&self.state).chain(&self.roots) {
            match require_empty(&held.directory) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(Readiness::Occupied),
                Err(error) => return Err(error),
            }
        }
        Ok(Readiness::Ready)
    }

    /// The caller presents these exact held roles for explicit setup approval
    /// and obtains both masked entries, never via argv/environment.
    pub(crate) fn prepare(self, login: &str, secret: &str, repeated: &str)
        -> Result<[u8;16], IssueError> {
        if !fresh_password::admissible(secret) || !fresh_password::admissible(repeated)
            || !bool::from(secret.as_bytes().ct_eq(repeated.as_bytes())) || !login_ok(login) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the two valid account password entries must match").into());
        }
        self.require_site()?;
        let dir = File::from(openat(&self.state.directory, ".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
            .map_err(io::Error::from)?);
        flock(&dir, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::from)?;
        let issue = self.issue(login);
        self.require_site()?;
        let epoch = issue.publish_locked(&dir, secret)?;
        self.require_site().map_err(IssueError::Uncertain)?;
        Ok(epoch)
    }
}

pub(crate) enum Readiness { Ready, Occupied }
impl Readiness {
    pub(crate) fn is_ready(&self) -> bool { matches!(self, Self::Ready) }
}
impl std::fmt::Display for Readiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ready => "Setup ready: the selected original directories are empty.",
            Self::Occupied => "Setup not ready: original records or other entries are present. Preserve them.",
        })
    }
}

pub(crate) fn login_ok(login: &str) -> bool {
    let raw = login.as_bytes();
    (1..=64).contains(&raw.len()) && (raw[0].is_ascii_lowercase() || raw[0].is_ascii_digit())
        && raw.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(b))
}
fn fresh_verifier(password: &str, salt: &[u8;16]) -> io::Result<Zeroizing<[u8;32]>> {
    let params = Params::new(65_536, 3, 4, Some(32)).map_err(|_| refused())?;
    let count = params.block_count();
    let mut workspace = Zeroizing::new(Vec::<Block>::new());
    workspace.try_reserve_exact(count).map_err(|_| io::Error::other("cannot reserve fresh Argon workspace"))?;
    workspace.resize(count, Block::new());
    let mut tag = Zeroizing::new([0;32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(password.as_bytes(), salt, &mut tag[..], workspace.as_mut_slice())
        .map_err(|_| io::Error::other("fresh Argon derivation failed"))?;
    Ok(tag)
}
fn stage_name(epoch: &[u8;16]) -> String {
    const HEX: &[u8;16] = b"0123456789abcdef";
    let mut name = String::from(".server-stage-");
    for b in epoch {
        name.push(char::from(HEX[usize::from(b >> 4)]));
        name.push(char::from(HEX[usize::from(b & 15)]));
    }
    name
}
