//! Retain the qualified system, recycle and data roster through the native Store.
use crate::{
    container_store::{DataRoot, Store, import_one::OfflineImport, offline_catalog},
    fs::{HeldDir, ReadAccess, local_uid},
    headless_auth::{Account, Descriptor, SelectedRoot},
};
use std::{
    collections::BTreeSet,
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
pub(super) const DESCRIPTOR: &CStr = c"server.conf";
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

fn role_member(root: &HeldDir, access: ReadAccess) -> io::Result<File> {
    match access {
        ReadAccess::Ordinary => open_member(root, MARKER),
        ReadAccess::NoAtime => root
            .opaque_pin_with(MARKER, access)?
            .map(|pin| pin.file)
            .ok_or_else(refused),
    }
}

fn root(
    selected: &SelectedRoot,
    role: u8,
    pool: &[u8; 16],
    uid: u32,
    access: ReadAccess,
) -> io::Result<HeldDir> {
    let held = HeldDir::trusted_storage(selected.path())?;
    let mut marker = role_member(&held, access)?;
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
    let observed = role_member(&held, access)?.metadata()?;
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

fn recycle_disjoint(recycle: &HeldDir, other: &HeldDir) -> io::Result<bool> {
    Ok(recycle.entity.device != other.entity.device && recycle.distinct_filesystems(other)?)
}

pub(crate) struct SelectedStart {
    account: Account,
    pool: [u8; 16],
    state: HeldDir,
    system: HeldDir,
    data: Vec<DataRoot>,
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
        let descriptor = Descriptor::read_selected(&open_member(&state, DESCRIPTOR)?, uid)?;
        Self::from_original(state, descriptor, ReadAccess::Ordinary)
    }

    pub(super) fn from_original(
        state: HeldDir,
        descriptor: Descriptor,
        access: ReadAccess,
    ) -> io::Result<Self> {
        let uid = local_uid();
        let (roots, account) = descriptor.into_parts();
        let system = root(&roots.system, 0, &roots.pool, uid, access)?;
        let recycle = root(&roots.recycle, 2, &roots.pool, uid, access)?;
        let data = roots
            .data
            .iter()
            .map(|selected| {
                Ok(DataRoot {
                    token: selected.token,
                    root: root(selected, 1, &roots.pool, uid, access)?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let held = [&system, &recycle]
            .into_iter()
            .chain(data.iter().map(|volume| &volume.root))
            .collect::<Vec<_>>();
        for (i, root) in held.iter().enumerate() {
            if state.path.starts_with(&root.path)
                || root.path.starts_with(&state.path)
                || root.entity == state.entity
                || held[..i].iter().any(|older| older.entity == root.entity)
            {
                return Err(refused());
            }
        }
        if !recycle_disjoint(&recycle, &system)? {
            return Err(refused());
        }
        for (i, volume) in data.iter().enumerate() {
            if !recycle_disjoint(&recycle, &volume.root)? {
                return Err(refused());
            }
            for earlier in &data[..i] {
                if volume.root.entity.device == earlier.root.entity.device
                    || volume.root.same_mount(&earlier.root)?
                {
                    return Err(refused());
                }
            }
        }
        let start = Self {
            account,
            pool: roots.pool,
            state,
            system,
            data,
            recycle,
        };
        start.require_named()?;
        Ok(start)
    }

    pub(super) fn offline_names(
        &self,
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<BTreeSet<String>> {
        offline_catalog::live_names(&self.system, &self.recycle, &self.data, self.pool, || {
            require_state(&self.state)?;
            self.require_named()
        })
    }

    fn require_named(&self) -> io::Result<()> {
        self.state.require_named()?;
        self.system.require_named()?;
        self.recycle.require_named()?;
        self.data
            .iter()
            .try_for_each(|volume| volume.root.require_named())
    }

    pub(super) fn bind_import(
        self,
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<(OfflineImport, HeldDir)> {
        self.require_named()?;
        let Self {
            pool,
            state,
            system,
            data,
            recycle,
            ..
        } = self;
        let writer = OfflineImport::open(system, recycle, data, pool, || require_state(&state))?;
        Ok((writer, state))
    }

    /// Move the original capabilities, never their strings, into Store.
    /// The chosen native Store retains these exact objects through drain.
    pub(crate) fn bind(self) -> io::Result<Bound> {
        self.require_named()?;
        let Self {
            account,
            pool,
            state,
            system,
            data,
            recycle,
        } = self;
        let store = Store::open_with_recycle(system, data, recycle, pool)?;
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
