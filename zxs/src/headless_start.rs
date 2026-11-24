//! Retain the qualified system, recycle and data roster through the native Store.
use crate::{
    container_store::{DataRoot, Store, import_one::OfflineImport, offline_catalog},
    fs::{HeldDir, ReadAccess, TrustedRoot, local_uid},
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
) -> io::Result<TrustedRoot> {
    let selection = TrustedRoot::open(selected.path())?;
    let held = &selection.held;
    let mut marker = role_member(held, access)?;
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
    let observed = role_member(held, access)?.metadata()?;
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
    Ok(selection)
}

fn recycle_disjoint(recycle: &HeldDir, other: &HeldDir) -> io::Result<bool> {
    Ok(recycle.entity.device != other.entity.device && recycle.distinct_filesystems(other)?)
}

pub(crate) struct SelectedStart {
    account: Account,
    pool: [u8; 16],
    state: HeldDir,
    system: HeldDir,
    #[cfg(target_arch = "x86_64")]
    system_token: [u8; 32],
    data: Vec<DataRoot>,
    recycle: HeldDir,
    recycle_token: [u8; 32],
}

impl SelectedStart {
    pub(crate) fn open_default() -> io::Result<Self> {
        let uid = local_uid();
        if matches!(uid, 0 | 65534 | u32::MAX) {
            return Err(refused());
        }
        let state = TrustedRoot::open(Path::new(DEFAULT_STATE))?;
        state.held.share()?; // same original directory lease used by the offline issuer
        #[cfg(target_arch = "x86_64")]
        crate::container_store::require_move_absent(&state.held)?;
        let descriptor = Descriptor::read_selected(&open_member(&state.held, DESCRIPTOR)?, uid)?;
        Self::from_original(state, descriptor, ReadAccess::Ordinary)
    }

    pub(super) fn from_original(
        state: TrustedRoot,
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
            .map(|selected| Ok((selected.token, root(selected, 1, &roots.pool, uid, access)?)))
            .collect::<io::Result<Vec<_>>>()?;
        let roles = [&state, &system, &recycle]
            .into_iter()
            .chain(data.iter().map(|(_, root)| root))
            .collect::<Vec<_>>();
        if roles
            .iter()
            .enumerate()
            .any(|(i, root)| roles[..i].iter().any(|older| root.overlaps(older)))
        {
            return Err(refused());
        }
        let (state, system, recycle) = (state.held, system.held, recycle.held);
        let data = data
            .into_iter()
            .map(|(token, at)| DataRoot {
                token,
                root: at.held,
            })
            .collect::<Vec<_>>();
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
            #[cfg(target_arch = "x86_64")]
            system_token: roots.system.token,
            data,
            recycle,
            recycle_token: roots.recycle.token,
        };
        start.require_named()?;
        Ok(start)
    }

    pub(super) fn offline_names(
        &self,
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<BTreeSet<String>> {
        offline_catalog::live_names(
            &self.system,
            &self.recycle,
            &self.data,
            self.pool,
            self.recycle_token,
            || {
                require_state(&self.state)?;
                self.require_named()
            },
        )
    }

    pub(super) fn verify_live_roles(
        &self,
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<Vec<[u64; 3]>> {
        require_state(&self.state)?;
        let held = [&self.system, &self.recycle]
            .into_iter()
            .chain(self.data.iter().map(|volume| &volume.root));
        let coordinates = held
            .map(HeldDir::live_identity)
            .collect::<io::Result<_>>()?;
        require_state(&self.state)?;
        Ok(coordinates)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn offline_move(
        &self,
        request: crate::container_store::MoveRequest,
        coordinates: &[[u64; 3]],
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
        pulse: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<crate::container_store::MoveCompletion> {
        let live = || -> io::Result<()> {
            pulse()?;
            require_state(&self.state)?;
            let roles = [&self.system, &self.recycle]
                .into_iter()
                .chain(self.data.iter().map(|volume| &volume.root));
            for (held, &expected) in roles.zip(coordinates) {
                if held.live_identity()? != expected {
                    return Err(refused());
                }
            }
            pulse()
        };
        if coordinates.len() != self.data.len() + 2 {
            return Err(refused());
        }
        crate::container_store::execute_move(
            crate::container_store::MoveRoles {
                state: &self.state,
                system: &self.system,
                recycle: &self.recycle,
                data: &self.data,
                pool: self.pool,
                system_token: self.system_token,
            },
            request,
            &live,
            pulse,
        )
    }

    pub(super) fn offline_verify(
        &self,
        id: crate::container_store::Id,
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<offline_catalog::Verification> {
        offline_catalog::verify_container(
            &self.system,
            &self.recycle,
            &self.data,
            self.pool,
            self.recycle_token,
            id,
            || {
                require_state(&self.state)?;
                self.require_named()
            },
        )
    }

    fn require_named(&self) -> io::Result<()> {
        self.state.require_named()?;
        self.system.require_named()?;
        self.recycle.require_named()?;
        self.data
            .iter()
            .try_for_each(|volume| volume.root.require_named())
    }

    pub(super) fn init_data(
        &self,
        pool: [u8; 16],
        token: [u8; 32],
        path: &[u8],
        roles: &[[u64; 3]],
        require_state: impl Fn(&HeldDir) -> io::Result<()>,
    ) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        if pool != self.pool || roles.len() != self.data.len() + 2 {
            return Err(refused());
        }
        // The parent and child independently resolve names. Under the same
        // namespace prove that both actually hold the same original role FDs,
        // including the SYSTEM whose prior routes we will consult.
        let prove_roles = || -> io::Result<()> {
            let held = [&self.system, &self.recycle]
                .into_iter()
                .chain(self.data.iter().map(|d| &d.root));
            for (dir, expected) in held.zip(roles) {
                if dir.live_identity()? != *expected {
                    return Err(refused());
                }
            }
            Ok(())
        };
        prove_roles()?;
        let target = self
            .data
            .iter()
            .find(|disk| disk.token == token && disk.root.path.as_os_str().as_bytes() == path)
            .ok_or_else(refused)?;
        let mark = target
            .root
            .opaque_pin_with(MARKER, ReadAccess::NoAtime)?
            .ok_or_else(refused)?;
        let mut bytes = [0; MARK_BYTES];
        if mark.file.metadata()?.len() != MARK_BYTES as u64 {
            return Err(refused());
        }
        (&mark.file).read_exact(&mut bytes)?;
        if !bytes.starts_with(MARK_MAGIC)
            || bytes[MARK_MAGIC.len()] != 1
            || bytes[MARK_MAGIC.len() + 1..MARK_MAGIC.len() + 17] != pool
            || bytes[MARK_MAGIC.len() + 17..] != token
        {
            return Err(refused());
        }
        crate::container_store::init_data::initialize(
            &self.system,
            &self.recycle,
            &self.data,
            pool,
            target,
            MARKER,
            || {
                require_state(&self.state)?;
                prove_roles()?;
                mark.require(&target.root, MARKER)
            },
        )
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
            recycle_token,
            ..
        } = self;
        let writer = OfflineImport::open(system, recycle, data, pool, recycle_token, || {
            require_state(&state)
        })?;
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
            recycle_token,
            ..
        } = self;
        #[cfg(target_arch = "x86_64")]
        crate::container_store::require_move_absent(&state)?;
        let store = Store::open_with_recycle(system, data, recycle, pool, recycle_token)?;
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
