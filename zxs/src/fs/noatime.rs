//! Metadata traversal on an original held directory; no atime or writes.
use super::{HeldDir, entity, invalid, mounts};
use std::{
    fs::{self, File, Metadata},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        raw::c_int,
        unix::fs::MetadataExt,
    },
};

unsafe extern "C" {
    fn zxs_open_noatime_directory(parent: c_int) -> c_int;
    fn zxs_getdents_page(fd: c_int, buffer: *mut u8, size: usize) -> isize;
}

#[derive(Clone, Copy)]
pub(crate) enum ReadAccess {
    Ordinary,
    NoAtime,
}

fn stamp(meta: &Metadata) -> (u64, i64, i64, i64, i64) {
    (
        meta.nlink(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

impl HeldDir {
    pub(crate) fn each_name(
        &self,
        access: ReadAccess,
        mut visit: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        if matches!(access, ReadAccess::Ordinary) {
            let held = format!("/proc/self/fd/{}", self.file.as_raw_fd());
            for row in fs::read_dir(held)? {
                visit(row?.file_name().as_encoded_bytes())?;
            }
            return Ok(());
        }
        self.require_named()?;
        let before = stamp(&self.file.metadata()?);
        // SAFETY: the parent is live, and on success we receive a fresh
        // no-atime directory FD with its own enumeration position.
        let raw = unsafe { zxs_open_noatime_directory(self.file.as_raw_fd()) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat transfers this descriptor to the caller.
        let directory = unsafe { File::from_raw_fd(raw) };
        if entity(&directory)? != self.entity
            || mounts::id(&directory)? != mounts::id(&self.file)?
            || stamp(&directory.metadata()?) != before
        {
            return Err(invalid("no-atime scan did not open the original directory"));
        }
        let mut buffer = [0; 4096];
        loop {
            // SAFETY: the writable buffer is exactly the supplied capacity
            // and directory retains the fd until the call returns.
            let count = unsafe {
                zxs_getdents_page(directory.as_raw_fd(), buffer.as_mut_ptr(), buffer.len())
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                break;
            }
            let count = usize::try_from(count).map_err(|_| invalid("invalid directory page"))?;
            read_page(
                buffer
                    .get(..count)
                    .ok_or_else(|| invalid("oversize directory page"))?,
                &mut visit,
            )?;
        }
        if stamp(&self.file.metadata()?) != before || stamp(&directory.metadata()?) != before {
            return Err(invalid("directory changed during no-atime enumeration"));
        }
        self.require_named()
    }
}

fn read_page(mut page: &[u8], visit: &mut impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    // Linux dirent64 uses u64 d_ino, i64 d_off, u16 d_reclen and u8 d_type.
    // The name begins at byte 19 and ends at the first NUL in the record.
    const NAME_AT: usize = 19;
    while !page.is_empty() {
        let width = page
            .get(16..18)
            .and_then(|raw| <[u8; 2]>::try_from(raw).ok())
            .map(|bytes| usize::from(u16::from_ne_bytes(bytes)))
            .filter(|width| *width > NAME_AT && *width <= page.len())
            .ok_or_else(|| invalid("invalid directory record width"))?;
        let name = &page[NAME_AT..width];
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .filter(|at| *at != 0)
            .ok_or_else(|| invalid("invalid directory component"))?;
        let name = &name[..end];
        if !matches!(name, b"." | b"..") {
            visit(name)?;
        }
        page = &page[width..];
    }
    Ok(())
}
