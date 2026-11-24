//! Stable per-container DATA placement, selected by SYSTEM before publication.
use super::{Id, hex, invalid, parse_container_id};
use crate::fs::{HeldDir, ReadAccess};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_uint},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    sync::{Mutex, MutexGuard},
};

pub(super) const DIRECTORY: &CStr = c"container-routes";
const MAGIC: &[u8; 4] = b"CR1\0";
const BYTES: usize = 52;
const STAGE_SUFFIX: &[u8] = b".incoming";
const RENAME_NOREPLACE: c_uint = 1;

unsafe extern "C" {
    fn zxs_create_new_regular(parent: c_int, name: *const c_char) -> c_int;
    fn renameat2(
        from: c_int,
        source: *const c_char,
        to: c_int,
        destination: *const c_char,
        flags: c_uint,
    ) -> c_int;
}

struct BeforeRoute<'a> {
    prove: &'a dyn Fn() -> io::Result<()>,
    authorize: &'a dyn Fn() -> io::Result<()>,
}

pub(super) struct Routes {
    directory: HeldDir,
    // Serialize the small SYSTEM publication cut, never DATA intake or copy.
    // True means a rename succeeded but directory sync was not confirmed.
    unsure: Mutex<bool>,
}

impl Routes {
    /// Caller retains SYSTEM's exclusive service-owner lease for our lifetime.
    pub(super) fn open(system: &HeldDir) -> io::Result<Self> {
        Ok(Self::in_directory(system.opaque_subdirectory(DIRECTORY)?))
    }

    /// Look up the held catalog without creating it. Confirm both the
    /// directory birth and any prior unsynced route publication.
    pub(super) fn open_existing(system: &HeldDir) -> io::Result<Option<Self>> {
        let Some(directory) = existing_directory(system)? else {
            return Ok(None);
        };
        directory.file.sync_all()?;
        system.file.sync_all()?;
        system.require_named()?;
        directory.require_named()?;
        Ok(Some(Self::in_directory(directory)))
    }

    #[cfg(test)]
    pub(super) fn require_upload_parent(&self, system: &HeldDir) -> io::Result<()> {
        let at = existing_directory(system)?
            .ok_or_else(|| invalid("upload lacks the original SYSTEM routes"))?;
        if at.entity != self.directory.entity || !at.same_mount(&self.directory)? {
            return Err(invalid("upload has a different SYSTEM authority"));
        }
        self.directory.require_named()
    }

    fn in_directory(directory: HeldDir) -> Self {
        Self {
            directory,
            unsure: Mutex::new(false),
        }
    }

    fn cut(&self) -> io::Result<MutexGuard<'_, bool>> {
        let mut unsure = self
            .unsure
            .lock()
            .map_err(|_| invalid("container route selection unavailable"))?;
        self.directory.require_named()?;
        if *unsure {
            self.directory.file.sync_all()?;
            self.directory.require_named()?;
            *unsure = false;
        }
        Ok(unsure)
    }

    fn read(&self, id: Id, name: &CStr) -> io::Result<Option<[u8; 32]>> {
        read_route(&self.directory, id, name, ReadAccess::Ordinary)
    }

    pub(super) fn get(&self, id: Id) -> io::Result<Option<[u8; 32]>> {
        let name = route_name(id)?;
        let _cut = self.cut()?;
        self.read(id, &name)
    }

    pub(super) fn ids(&self) -> io::Result<Vec<Id>> {
        let _cut = self.cut()?;
        read_ids(&self.directory, ReadAccess::Ordinary)
    }

    /// Call after validating bootstrap and the complete staged DATA body, but
    /// before making any part of that container visible on DATA.
    pub(super) fn reserve(&self, id: Id, token: [u8; 32]) -> io::Result<()> {
        self.reserve_using(id, token, None)
    }

    #[cfg(test)]
    pub(super) fn reserve_upload(
        &self,
        id: Id,
        token: [u8; 32],
        prove: &impl Fn() -> io::Result<()>,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        self.reserve_using(id, token, Some(&BeforeRoute { prove, authorize }))
    }

    fn reserve_using(
        &self,
        id: Id,
        token: [u8; 32],
        gate: Option<&BeforeRoute<'_>>,
    ) -> io::Result<()> {
        let name = route_name(id)?;
        let raw = encode(id, token)?;
        let mut unsure = self.cut()?;
        if let Some(selected) = self.read(id, &name)? {
            return if selected == token {
                Ok(())
            } else {
                Err(invalid("container already belongs to another data disk"))
            };
        }

        if let Some(gate) = gate {
            (gate.prove)()?;
            (gate.authorize)()?;
        }
        let stage = CString::new(format!(".{}.incoming", hex(id)))
            .map_err(|_| invalid("invalid container route stage name"))?;
        if let Some(remnant) = self.directory.opaque_pin(&stage)? {
            // Only our precise per-ID private stage may be discarded.
            self.directory.unlink_opaque_original(&stage, &remnant)?;
        }

        let parent = self.directory.file.as_raw_fd();
        // SAFETY: the held directory and exact component stay live; the
        // native helper opens only a new private regular record.
        let fd = unsafe { zxs_create_new_regular(parent, stage.as_ptr()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful create returned one new owning FD.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(&raw)?;
        file.sync_all()?;
        let original = file.metadata()?;
        let pin = self
            .directory
            .opaque_pin(&stage)?
            .ok_or_else(|| invalid("container route stage disappeared"))?;
        let named = pin.file.metadata()?;
        if (original.dev(), original.ino()) != (named.dev(), named.ino()) {
            return Err(invalid("container route stage was replaced"));
        }
        if let Some(gate) = gate {
            (gate.prove)()?;
        }
        pin.require(&self.directory, &stage)?;
        if let Some(gate) = gate {
            self.directory.confirm_absence(&name)?;
            (gate.authorize)()?;
        }
        // SAFETY: both names are checked single components within the same
        // held private directory; the kernel must never replace a destination.
        if unsafe {
            renameat2(
                parent,
                stage.as_ptr(),
                parent,
                name.as_ptr(),
                RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        *unsure = true;
        self.directory.file.sync_all()?;
        self.directory.require_named()?;
        *unsure = false;
        Ok(())
    }
}

pub(super) fn existing_directory(system: &HeldDir) -> io::Result<Option<HeldDir>> {
    let directory = system.child_directory(DIRECTORY)?;
    system.require_named()?;
    Ok(directory)
}

pub(super) fn lookup_route(
    directory: &HeldDir,
    id: Id,
    access: ReadAccess,
) -> io::Result<Option<[u8; 32]>> {
    read_route(directory, id, &route_name(id)?, access)
}

fn read_route(
    directory: &HeldDir,
    id: Id,
    name: &CStr,
    access: ReadAccess,
) -> io::Result<Option<[u8; 32]>> {
    let Some(pin) = directory.opaque_pin_with(name, access)? else {
        return Ok(None);
    };
    if pin.file.metadata()?.len() != BYTES as u64 {
        return Err(invalid("invalid container route size"));
    }
    let mut raw = [0; BYTES];
    (&pin.file).read_exact(&mut raw)?;
    pin.require(directory, name)?;
    let (found, token) = decode(raw)?;
    if found != id {
        return Err(invalid("container route names a different container"));
    }
    Ok(Some(token))
}

pub(super) fn read_ids(directory: &HeldDir, access: ReadAccess) -> io::Result<Vec<Id>> {
    let mut ids = Vec::new();
    directory.each_name(access, |name| {
        if let Some(stage) = name
            .strip_prefix(b".")
            .and_then(|name| name.strip_suffix(STAGE_SUFFIX))
        {
            parse_container_id(stage)?;
        } else {
            ids.push(parse_container_id(name)?);
        }
        Ok(())
    })?;
    directory.require_named()?;
    ids.sort_unstable();
    Ok(ids)
}

fn route_name(id: Id) -> io::Result<CString> {
    if id == [0; 16] {
        return Err(invalid("zero container route identity"));
    }
    CString::new(hex(id)).map_err(|_| invalid("invalid container route name"))
}

fn encode(id: Id, token: [u8; 32]) -> io::Result<[u8; BYTES]> {
    if id == [0; 16] || token == [0; 32] {
        return Err(invalid("zero container or data disk route identity"));
    }
    let mut raw = [0; BYTES];
    raw[..4].copy_from_slice(MAGIC);
    raw[4..20].copy_from_slice(&id);
    raw[20..].copy_from_slice(&token);
    Ok(raw)
}

fn decode(raw: [u8; BYTES]) -> io::Result<(Id, [u8; 32])> {
    if &raw[..4] != MAGIC {
        return Err(invalid("unknown container route format"));
    }
    let id = raw[4..20]
        .try_into()
        .map_err(|_| invalid("invalid container route identity"))?;
    let token = raw[20..]
        .try_into()
        .map_err(|_| invalid("invalid data disk route identity"))?;
    if id == [0; 16] || token == [0; 32] {
        return Err(invalid("zero container or data disk route identity"));
    }
    Ok((id, token))
}

#[cfg(test)]
#[path = "../../tests/unit/container_routes.rs"]
mod tests;
