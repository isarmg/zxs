//! Exact single-component operations against an original held directory.
use super::*;
use std::ffi::OsStr;

fn leaf(name: &CStr) -> io::Result<&OsStr> {
    let b = name.to_bytes();
    if b.is_empty() || b.len() > 255 || b == b"." || b == b".." || b.contains(&b'/') {
        return Err(invalid("not a single bounded native name"));
    }
    Ok(OsStr::from_bytes(b))
}

impl HeldDir {
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
            .get(..n as usize)
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
