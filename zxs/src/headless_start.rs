//! Retain the exact three qualified mount roots through the fresh native Store.
use crate::{
    container_store::Store,
    fs::{HeldDir, local_uid},
    headless_auth::{Account, Descriptor, SelectedRoot},
};
use std::{
    ffi::{CStr, c_char, c_int, c_long},
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};

unsafe extern "C" {
    fn zxs_startup_open_member(parent: c_int, name: *const c_char) -> c_long;
}

pub(crate) const DEFAULT_STATE: &str = "/zxs/state/headless";
const DESCRIPTOR: &CStr = c"server.conf";
const MARKER: &CStr = c".zxs-role-v1";
const MARK_MAGIC: &[u8] = b"ZXS-ROLE-V1\0";
const MARK_BYTES: usize = MARK_MAGIC.len() + 1 + 16 + 32;

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "fresh deployment or disk identity was not proved",
    )
}

fn open_member(parent: &HeldDir, name: &CStr) -> io::Result<File> {
    // SAFETY: the original directory and NUL-terminated member stay live.
    let fd = unsafe { zxs_startup_open_member(parent.file.as_raw_fd(), name.as_ptr()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = i32::try_from(fd).map_err(|_| refused())?;
    // SAFETY: successful openat2 returns one newly owned fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn root(selected: &SelectedRoot, role: u8, pool: &[u8; 16], uid: u32) -> io::Result<HeldDir> {
    let held = HeldDir::trusted_storage(selected.path())?;
    let mut marker = open_member(&held, MARKER)?;
    let initial = marker.metadata()?;
    if !initial.is_file()
        || initial.mode() & 0o7777 != 0o600
        || initial.uid() != uid
        || initial.nlink() != 1
        || initial.len() != MARK_BYTES as u64
    {
        return Err(refused());
    }
    let mut exact = [0; MARK_BYTES];
    marker.read_exact(&mut exact)?;
    let body = exact.strip_prefix(MARK_MAGIC).ok_or_else(refused)?;
    if body[0] != role || body[1..17] != pool[..] || body[17..] != selected.token {
        return Err(refused());
    }
    let observed = open_member(&held, MARKER)?.metadata()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.ctime(),
            m.ctime_nsec(),
            m.mtime(),
            m.mtime_nsec(),
        )
    };
    if stamp(&observed) != stamp(&initial) || stamp(&marker.metadata()?) != stamp(&initial) {
        return Err(refused());
    }
    held.require_named()?;
    Ok(held)
}

pub(crate) struct SelectedStart {
    account: Account,
    state: HeldDir,
    system: HeldDir,
    data: HeldDir,
    recycle: HeldDir,
}

impl SelectedStart {
    pub(crate) fn open_default() -> io::Result<Self> {
        let uid = local_uid();
        if matches!(uid, 0 | 65534 | u32::MAX) {
            return Err(refused());
        }
        let state = HeldDir::trusted_storage(Path::new(DEFAULT_STATE))?;
        state.share()?; // same original directory lease used by the offline issuer
        let (roots, account) =
            Descriptor::read_selected(&open_member(&state, DESCRIPTOR)?, uid)?.into_parts();
        let system = root(&roots.system, 0, &roots.pool, uid)?;
        let data = root(&roots.data, 1, &roots.pool, uid)?;
        let recycle = root(&roots.recycle, 2, &roots.pool, uid)?;
        let roots = [&system, &data, &recycle];
        if roots.iter().any(|root| {
            state.path.starts_with(&root.path)
                || root.path.starts_with(&state.path)
                || root.entity == state.entity
        }) || system.entity == data.entity
            || system.entity == recycle.entity
            || data.entity == recycle.entity
            || !recycle.distinct_filesystems(&system)?
            || !recycle.distinct_filesystems(&data)?
        {
            return Err(refused());
        }
        let start = Self {
            account,
            state,
            system,
            data,
            recycle,
        };
        start.require_named()?;
        Ok(start)
    }

    fn require_named(&self) -> io::Result<()> {
        self.state.require_named()?;
        self.system.require_named()?;
        self.data.require_named()?;
        self.recycle.require_named()
    }

    /// Move the original capabilities, never their strings, into Store.
    /// The chosen native Store retains these exact objects through drain.
    pub(crate) fn bind(self) -> io::Result<Bound> {
        self.require_named()?;
        let Self {
            account,
            state,
            system,
            data,
            recycle,
        } = self;
        let store = Store::open_with_recycle(system, data, recycle)?;
        state.require_named()?;
        Ok(Bound {
            store,
            account,
            _state: state,
        })
    }
}

pub(crate) struct Bound {
    store: Store,
    account: Account,
    _state: HeldDir,
}
impl Bound {
    pub(crate) fn store(&self) -> &Store {
        &self.store
    }
    pub(crate) fn account(&self) -> &Account {
        &self.account
    }
}
