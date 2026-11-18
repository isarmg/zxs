//! Bounded directory operations through retained Linux descriptors. The C
//! shim only translates Linux ABI flags, xattrs and the directory-entry ABI.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};

unsafe extern "C" {
    fn zxs_open_directory(parent: c_int, name: *const c_char) -> c_int;
    fn zxs_directory_empty(fd: c_int) -> c_int;
    fn zxs_volume_label_present(fd: c_int) -> c_int;
    fn zxs_effective_uid() -> c_uint;
}

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entity { pub device: u64, pub inode: u64 }

fn entity(file: &File) -> io::Result<Entity> {
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.ino() == 0 {
        return Err(invalid("storage identity is not a bound directory"));
    }
    Ok(Entity { device: meta.dev(), inode: meta.ino() })
}

fn child(parent: &File, component: &CStr) -> io::Result<File> {
    // SAFETY: the borrowed descriptor and NUL-terminated component live for
    // the call. On success openat transferred a fresh descriptor to us.
    let fd = unsafe { zxs_open_directory(parent.as_raw_fd(), component.as_ptr()) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}

fn open_absolute(path: &Path) -> io::Result<File> {
    if !path.is_absolute() || path.as_os_str().as_bytes().len() > 4096 {
        return Err(invalid("supply a bounded absolute directory path"));
    }
    let mut current = File::open("/")?;
    let mut normal = PathBuf::from("/");
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                depth += 1;
                if depth > 256 { return Err(invalid("registration path is too deep")); }
                let name_c = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in directory path"))?;
                current = child(&current, &name_c)?;
                normal.push(name);
            }
            _ => return Err(invalid("relative or parent components are not allowed")),
        }
    }
    if depth == 0 || normal.as_os_str().as_bytes() != path.as_os_str().as_bytes() {
        return Err(invalid("use an exact absolute directory path, not / or an alias spelling"));
    }
    Ok(current)
}

pub(crate) struct HeldDir {
    pub(crate) file: File,
    pub(crate) path: PathBuf,
    pub(crate) entity: Entity,
}

impl HeldDir {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = open_absolute(path)?;
        let entity = entity(&file)?;
        let meta = file.metadata()?;
        // SAFETY: geteuid takes no arguments and returns the process credential.
        if meta.uid() != unsafe { zxs_effective_uid() } || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "selected directory must belong to this account and not allow group/other writes"));
        }
        Ok(Self { file, path: path.to_owned(), entity })
    }

    pub(crate) fn require_named(&self) -> io::Result<()> {
        let current = Self::open(&self.path)?;
        if current.entity != self.entity { return Err(invalid("selected directory was replaced")); }
        Ok(())
    }

    pub(crate) fn share(&self) -> io::Result<()> {
        self.file.try_lock_shared().map_err(io::Error::from)
    }

    pub(crate) fn require_fresh(&self) -> io::Result<()> {
        // SAFETY: both calls receive only our still-held read-only directory.
        let empty = unsafe { zxs_directory_empty(self.file.as_raw_fd()) };
        if empty < 0 { return Err(io::Error::last_os_error()); }
        if empty != 1 { return Err(invalid("selected directory is not empty")); }
        let label = unsafe { zxs_volume_label_present(self.file.as_raw_fd()) };
        if label < 0 { return Err(io::Error::last_os_error()); }
        if label != 0 { return Err(invalid("selected directory already carries a volume identity")); }
        Ok(())
    }

    pub(crate) fn reject_registered_ancestors(&self, selected: &std::collections::BTreeSet<Entity>) -> io::Result<()> {
        let mut current = self.file.try_clone()?;
        let mut prior = self.entity;
        for _ in 0..512 {
            let parent = child(&current, c"..")?;
            let id = entity(&parent)?;
            if id == prior { return Ok(()); }
            if selected.contains(&id) { return Err(invalid("selected storage roots overlap")); }
            current = parent;
            prior = id;
        }
        Err(invalid("cannot establish physical root ancestry"))
    }
}
