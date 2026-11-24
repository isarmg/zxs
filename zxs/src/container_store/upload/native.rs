//! Linux witness operations, restricted to qualified upload staging and controls.
use super::record::Birth;
use crate::{
    container_store::invalid,
    fs::{HeldDir, OpaquePin},
};
use std::{
    ffi::{CStr, c_char, c_int, c_uint},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
};

unsafe extern "C" {
    fn zxs_upload_birth(fd: c_int, inode: *mut u64, seconds: *mut i64, nanos: *mut u32) -> c_int;
    fn zxs_open_pinned_record(pin: c_int, write: c_int) -> c_int;
    fn zxs_remove_name(parent: c_int, name: *const c_char) -> c_int;
    fn renameat2(
        source_fd: c_int,
        source: *const c_char,
        destination_fd: c_int,
        destination: *const c_char,
        flags: c_uint,
    ) -> c_int;
}

pub(super) fn birth(file: &File) -> io::Result<Birth> {
    let mut proof = Birth {
        inode: 0,
        seconds: 0,
        nanos: 0,
    };
    // SAFETY: the file remains held, and each pointer is a live writable integer.
    if unsafe {
        zxs_upload_birth(
            file.as_raw_fd(),
            &raw mut proof.inode,
            &raw mut proof.seconds,
            &raw mut proof.nanos,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if !proof.valid() || file.metadata()?.ino() != proof.inode {
        return Err(invalid(
            "upload native birth did not identify the held inode",
        ));
    }
    Ok(proof)
}

pub(super) fn require_same(parent: &HeldDir, original: &File, seen: &File) -> io::Result<()> {
    let old = original.metadata()?;
    let new = seen.metadata()?;
    if (old.dev(), old.ino()) != (new.dev(), new.ino())
        || !parent.shares_file_mount(original)?
        || !parent.shares_file_mount(seen)?
    {
        return Err(invalid("upload record is not the same held file"));
    }
    parent.require_named()
}

pub(super) fn require_owner(parent: &HeldDir, name: &CStr, owner: &File) -> io::Result<()> {
    let named = parent
        .opaque_pin(name)?
        .ok_or_else(|| invalid("upload owner lease is no longer named"))?;
    if named.file.metadata()?.len() != 0 {
        return Err(invalid(
            "upload owner lease is not the original empty lease",
        ));
    }
    require_same(parent, owner, &named.file)?;
    named.require(parent, name)
}

/// Open the same private, already qualified raw inode, never its path or a
/// possibly foreign name. The caller checks authorization again before pwrite.
pub(super) fn open_payload_writer(parent: &HeldDir, pinned: &OpaquePin) -> io::Result<File> {
    pinned.require(parent, c"payload")?;
    // SAFETY: the qualified original file is retained in pinned.
    let raw = unsafe { zxs_open_pinned_record(pinned.file.as_raw_fd(), 1) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful reopen returned a new owning descriptor.
    let file = unsafe { File::from_raw_fd(raw) };
    require_same(parent, &pinned.file, &file)?;
    pinned.require(parent, c"payload")?;
    Ok(file)
}

/// Both names are privately generated, and the caller holds the one per-C
/// gate and proves the exact old record (or its absence) at the final cut.
pub(super) fn promote_control(
    directory: &HeldDir,
    temporary: &CStr,
    original: &CStr,
    replace: bool,
) -> io::Result<()> {
    rename(
        directory,
        temporary,
        directory,
        original,
        c_uint::from(!replace),
    )
}

pub(super) fn move_fresh(
    origin: &HeldDir,
    source: &CStr,
    destination: &HeldDir,
    name: &CStr,
    prove_source: impl FnOnce() -> io::Result<()>,
    authorize: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    origin.checked_child_path(source)?;
    destination.checked_child_path(name)?;
    if !origin.same_mount(destination)? {
        return Err(invalid(
            "resumed publication must stay on the original mount",
        ));
    }
    prove_source()?;
    origin.require_named()?;
    destination.confirm_absence(name)?;
    authorize()?;
    rename(origin, source, destination, name, 1)
}

fn rename(
    origin: &HeldDir,
    source: &CStr,
    destination: &HeldDir,
    name: &CStr,
    flags: c_uint,
) -> io::Result<()> {
    // SAFETY: the held descriptors and exact checked NUL-terminated leaf
    // names remain live; the private publisher has checked its own authority.
    if unsafe {
        renameat2(
            origin.file.as_raw_fd(),
            source.as_ptr(),
            destination.file.as_raw_fd(),
            name.as_ptr(),
            flags,
        )
    } < 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn remove_control(directory: &HeldDir, name: &CStr) -> io::Result<()> {
    // SAFETY: the caller has just reproved the exact small authorized
    // control record inside this held parent, under the cooperative gate.
    if unsafe { zxs_remove_name(directory.file.as_raw_fd(), name.as_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
