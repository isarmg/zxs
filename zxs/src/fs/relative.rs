//! Exact single-component operations against an original held directory.
use super::{
    DIRECTORY_BYTES, DIRECTORY_NAMES, HeldDir, check_path, child, entity, invalid, local_uid,
    mounts, os_result, zxs_mkdir_private, zxs_root_names,
};
use std::{
    ffi::{CStr, OsStr, c_char, c_int},
    io,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::PathBuf,
};

fn leaf(name: &CStr) -> io::Result<&OsStr> {
    let b = name.to_bytes();
    if b.is_empty() || b.len() > 255 || b == b"." || b == b".." || b.contains(&b'/') {
        return Err(invalid("not a single bounded native name"));
    }
    Ok(OsStr::from_bytes(b))
}

unsafe extern "C" {
    fn renameat(from: c_int, source: *const c_char, to: c_int, destination: *const c_char)
    -> c_int;
}

impl HeldDir {
    /// Publish under the caller's existing private store/namespace lease.
    /// Absence is checked on the held destination, not enforced by renameat
    /// against other software using this same service account.
    pub(crate) fn rename_child_if_absent(
        &self,
        source: &CStr,
        destination: &Self,
        name: &CStr,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        leaf(source)?;
        leaf(name)?;
        self.require_named()?;
        destination.require_named()?;
        if !self.same_mount(destination)? {
            return Err(invalid("staging publication crossed its enrolled mount"));
        }
        if destination.pin_record(name)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging publication destination already exists",
            ));
        }
        authorize()?;
        // SAFETY: both FDs are held and both CStr values are checked single
        // components. The kernel resolves neither parent by an absolute path.
        os_result(unsafe {
            renameat(
                self.file.as_raw_fd(),
                source.as_ptr(),
                destination.file.as_raw_fd(),
                name.as_ptr(),
            )
        })
    }

    pub(crate) fn checked_child_path(&self, name: &CStr) -> io::Result<PathBuf> {
        let path = self.path.join(leaf(name)?);
        check_path(&path)?;
        Ok(path)
    }
    pub(crate) fn child_directory(&self, name: &CStr) -> io::Result<Option<HeldDir>> {
        let path = self.checked_child_path(name)?;
        let file = match child(&self.file, name) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if mounts::id(&file)? != mounts::id(&self.file)? {
            return Err(invalid("FILE directory crosses its original mount"));
        }
        let meta = file.metadata()?;
        if meta.uid() != local_uid() || meta.mode() & 0o7777 != 0o700 {
            return Err(invalid(
                "FILE container directory is not private to the local owner",
            ));
        }
        let held = HeldDir {
            entity: entity(&file)?,
            file,
            path,
        };
        self.require_named()?;
        held.require_named()?;
        Ok(Some(held))
    }
    pub(crate) fn new_child_directory(&self, name: &CStr) -> io::Result<HeldDir> {
        check_path(&self.path.join(leaf(name)?))?;
        self.require_named()?;
        // SAFETY: name is a bounded single component and the parent FD is held.
        os_result(unsafe { zxs_mkdir_private(self.file.as_raw_fd(), name.as_ptr()) })?;
        self.child_directory(name)?
            .ok_or_else(|| invalid("new FILE directory vanished"))
    }
    pub(crate) fn names(&self) -> io::Result<Vec<Vec<u8>>> {
        let mut buf = vec![0u8; DIRECTORY_BYTES];
        // SAFETY: the live output buffer is cap bytes and the directory is held.
        let n = unsafe {
            zxs_root_names(
                self.file.as_raw_fd(),
                buf.as_mut_ptr(),
                buf.len(),
                DIRECTORY_NAMES,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut input = buf
            .get(..usize::try_from(n).map_err(|_| invalid("invalid directory listing length"))?)
            .ok_or_else(|| invalid("invalid directory listing length"))?;
        let mut result = Vec::new();
        while let Some((&len, tail)) = input.split_first() {
            let (name, rest) = tail
                .split_at_checked(usize::from(len))
                .ok_or_else(|| invalid("incomplete name enumeration"))?;
            if name.is_empty() {
                return Err(invalid("invalid directory name"));
            }
            result.push(name.to_vec());
            input = rest;
        }
        Ok(result)
    }
}
