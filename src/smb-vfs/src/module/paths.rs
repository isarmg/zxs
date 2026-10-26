//! Samba filename conversion and confinement to the configured share root.
use super::{bad, sdk};
use std::{ffi::CStr, path::Path};

/// # Safety
/// A non-null pointer must address a NUL-terminated string live and immutable for 'a.
pub(super) unsafe fn text<'a>(pointer: *const libc::c_char) -> std::io::Result<&'a str> {
    if pointer.is_null() {
        return Err(bad());
    }
    // SAFETY: The caller guarantees a live NUL-terminated string for the returned borrow; null was
    // rejected above.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
}
/// # Safety
/// Non-null arguments and nested strings obey the host boundary contract.
pub(super) unsafe fn name(
    handle: *mut sdk::vfs_handle_struct,
    pointer: *const sdk::smb_filename,
) -> std::io::Result<String> {
    if pointer.is_null() {
        return Err(bad());
    }
    // SAFETY: The host filename is live for this callback and its non-null string fields are NUL-
    // terminated; text rejects null strings.
    if !unsafe { (*pointer).stream_name }.is_null()
        // SAFETY: The host filename is live for this callback and its non-null string fields are
        // NUL-terminated; text rejects null strings.
        && !unsafe { text((*pointer).stream_name) }?.is_empty()
    {
        return Err(std::io::Error::from_raw_os_error(libc::ENOTSUP));
    }
    // SAFETY: The host filename is live for this callback and its non-null string fields are NUL-
    // terminated; text rejects null strings.
    let name = unsafe { text((*pointer).base_name) }?;
    if name.contains('\\') {
        return Err(bad());
    }
    let name = if name.starts_with('/') {
        // SAFETY: A non-null handle is a live host object; short-circuit evaluation prevents
        // dereferencing a null handle.
        if handle.is_null() || unsafe { (*handle).conn }.is_null() {
            return Err(bad());
        }
        // SAFETY: The checked connection belongs to the live handle; its connectpath is a host-
        // owned NUL-terminated string.
        let root = unsafe { text((*(*handle).conn).connectpath) }?;
        Path::new(name)
            .strip_prefix(root)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?
            .to_str()
            .ok_or_else(bad)?
    } else {
        name
    };
    let mut components = Vec::new();
    for component in name.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(bad()),
            component => components.push(component),
        }
    }
    Ok(components.join("/"))
}
/// # Safety
/// Non-null arguments and nested filenames/strings obey the host boundary contract.
pub(super) unsafe fn at_name(
    handle: *mut sdk::vfs_handle_struct,
    directory: *const sdk::files_struct,
    file: *const sdk::smb_filename,
) -> std::io::Result<String> {
    // SAFETY: The host supplies live filename/directory objects and their string fields for this
    // callback; optional nulls are rejected or handled by the helper.
    let child = unsafe { name(handle, file) }?;
    // SAFETY: The host filename is live for this callback and its non-null string fields are NUL-
    // terminated; text rejects null strings.
    if unsafe { text((*file).base_name) }?.starts_with('/') {
        return Ok(child);
    }
    let parent = if directory.is_null() {
        String::new()
    } else {
        // SAFETY: The host supplies live filename/directory objects and their string fields for
        // this callback; optional nulls are rejected or handled by the helper.
        unsafe { name(handle, (*directory).fsp_name) }?
    };
    Ok(if parent.is_empty() {
        child
    } else if child.is_empty() {
        parent
    } else {
        format!("{parent}/{child}")
    })
}
