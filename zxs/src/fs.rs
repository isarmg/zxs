//! Bounded directory operations through retained Linux descriptors. The C
//! shim only translates Linux ABI flags, xattrs and the directory-entry ABI.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};

mod mounts;
mod relative;

unsafe extern "C" {
    fn zxs_open_directory(parent: c_int, name: *const c_char) -> c_int;
    fn zxs_directory_has_only(fd: c_int, allowed: *const c_char) -> c_int;
    fn zxs_pin_record(fd: c_int, name: *const c_char) -> c_int;
    fn zxs_open_pinned_record(pin: c_int, write: c_int) -> c_int;
    fn zxs_mkdir_private(fd: c_int, name: *const c_char) -> c_int;
    fn zxs_remove_empty_dir(fd: c_int, name: *const c_char) -> c_int;
    fn zxs_remove_name(fd: c_int, name: *const c_char) -> c_int;
    fn zxs_root_names(fd: c_int, out: *mut u8, cap: usize, max_entries: usize) -> isize;
    fn zxs_effective_uid() -> c_uint;
}

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entity {
    pub device: u64,
    pub inode: u64,
}

fn entity(file: &File) -> io::Result<Entity> {
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.ino() == 0 {
        return Err(invalid("storage identity is not a bound directory"));
    }
    Ok(Entity {
        device: meta.dev(),
        inode: meta.ino(),
    })
}

fn child(parent: &File, component: &CStr) -> io::Result<File> {
    // SAFETY: the borrowed descriptor and NUL-terminated component live for
    // the call. On success openat transferred a fresh descriptor to us.
    let fd = unsafe { zxs_open_directory(parent.as_raw_fd(), component.as_ptr()) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

pub(crate) fn local_uid() -> u32 {
    // SAFETY: read the real process-effective credential; never accept a UID from a client.
    unsafe { zxs_effective_uid() }
}

pub(crate) const DIRECTORY_NAMES: usize = 8192;
pub(crate) const DIRECTORY_BYTES: usize = 512 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATH_DEPTH: usize = 256;

pub(crate) fn check_path(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes().len();
    if !path.is_absolute() || bytes > MAX_PATH_BYTES {
        return Err(invalid("supply a bounded absolute directory path"));
    }
    let mut normal = PathBuf::from("/");
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                depth += 1;
                if depth > MAX_PATH_DEPTH || name.as_bytes().contains(&0) {
                    return Err(invalid("invalid directory component or depth"));
                }
                normal.push(name);
            }
            _ => return Err(invalid("relative or parent components are not allowed")),
        }
    }
    if depth == 0 || normal.as_os_str().as_bytes() != path.as_os_str().as_bytes() {
        return Err(invalid(
            "use an exact absolute directory path, not / or an alias spelling",
        ));
    }
    Ok(())
}

fn open_absolute(path: &Path) -> io::Result<File> {
    check_path(path)?;
    let mut current = File::open("/")?;
    for component in path.components() {
        if let Component::Normal(name) = component {
            let c = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in directory path"))?;
            current = child(&current, &c)?;
        }
    }
    Ok(current)
}

pub(crate) struct HeldDir {
    pub(crate) file: File,
    pub(crate) path: PathBuf,
    pub(crate) entity: Entity,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Location {
    entity: Entity,
    mount: u64,
}
#[cfg(test)]
fn lineage_end(here: Location, parent: Location, namespace_root: Location) -> io::Result<bool> {
    if here.entity != parent.entity {
        return Ok(false);
    }
    if here != parent || here != namespace_root {
        return Err(invalid(
            "repeated directory entity is not the same held namespace root",
        ));
    }
    Ok(true)
}

impl HeldDir {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = open_absolute(path)?;
        let entity = entity(&file)?;
        let meta = file.metadata()?;
        // SAFETY: geteuid takes no arguments and returns the process credential.
        if meta.uid() != unsafe { zxs_effective_uid() } || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "selected directory must belong to this account and not allow group/other writes",
            ));
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            entity,
        })
    }

    /// Each canonical path component must resist untrusted rename, not
    /// just the selected directory.
    /// A trusted-owned sticky ancestor (e.g. /tmp) protects its existing,
    /// trusted-owned child. Symlink traversal is never allowed.
    pub(crate) fn trusted_storage(path: &Path) -> io::Result<Self> {
        check_path(path)?;
        let uid = local_uid();
        if uid == 0 || uid == u32::MAX {
            return Err(invalid("storage requires a real non-root service account"));
        }
        let mut file = File::open("/")?;
        for component in path.components() {
            if let Component::Normal(name) = component {
                let p = file.metadata()?;
                if !(p.uid() == 0 || p.uid() == uid)
                    || (p.mode() & 0o022 != 0 && p.mode() & 0o1000 == 0)
                {
                    return Err(invalid("untrusted storage ancestor"));
                }
                file = child(
                    &file,
                    &CString::new(name.as_bytes())
                        .map_err(|_| invalid("invalid storage component"))?,
                )?;
            }
        }
        let m = file.metadata()?;
        if m.uid() != uid || m.mode() & 0o7777 != 0o700 {
            return Err(invalid(
                "selected storage requires its exact private directory",
            ));
        }
        Ok(Self {
            entity: entity(&file)?,
            file,
            path: path.to_owned(),
        })
    }

    pub(crate) fn require_named(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        if current.entity != self.entity || mounts::id(&current.file)? != mounts::id(&self.file)? {
            return Err(invalid(
                "selected directory or its named mount was replaced",
            ));
        }
        Ok(())
    }

    pub(crate) fn share(&self) -> io::Result<()> {
        self.file.try_lock_shared().map_err(io::Error::from)
    }
}

fn os_result(rc: c_int) -> io::Result<()> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl HeldDir {
    /// Same open directory, not a fresh lookup; retains the existing flock.
    pub(crate) fn same_mount(&self, other: &Self) -> io::Result<bool> {
        Ok(mounts::id(&self.file)? == mounts::id(&other.file)?)
    }
    pub(crate) fn distinct_filesystems(&self, other: &Self) -> io::Result<bool> {
        mounts::Mounts::read()?.distinct_filesystems(&self.file, &other.file)
    }
    pub(crate) fn only(&self, name: Option<&CStr>) -> io::Result<()> {
        // SAFETY: the optional allowed name is NUL terminated for this call.
        let rc = unsafe {
            zxs_directory_has_only(
                self.file.as_raw_fd(),
                name.map_or(std::ptr::null(), CStr::as_ptr),
            )
        };
        match rc.cmp(&0) {
            std::cmp::Ordering::Less => Err(io::Error::last_os_error()),
            std::cmp::Ordering::Equal => Err(invalid(
                "unexpected existing contents at registration boundary",
            )),
            std::cmp::Ordering::Greater => Ok(()),
        }
    }
    pub(crate) fn pin_record(&self, name: &CStr) -> io::Result<Option<File>> {
        // SAFETY: O_PATH|O_NOFOLLOW pins an entry without activating a device,
        // opening a FIFO or following a symlink at this untrusted boundary.
        let fd = unsafe { zxs_pin_record(self.file.as_raw_fd(), name.as_ptr()) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(e)
            };
        }
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }
    pub(crate) fn shares_file_mount(&self, file: &File) -> io::Result<bool> {
        Ok(mounts::id(&self.file)? == mounts::id(file)?)
    }
    pub(crate) fn confirm_absence(&self, name: &CStr) -> io::Result<()> {
        self.require_named()?;
        if self.pin_record(name)?.is_some() {
            return Err(invalid("retired entry unexpectedly remains named"));
        }
        self.file.sync_all()?;
        if self.pin_record(name)?.is_some() {
            return Err(invalid("retired name reused at release"));
        }
        self.require_named()
    }
}

mod opaque;
pub(crate) use opaque::OPAQUE_STAMP_BYTES;
