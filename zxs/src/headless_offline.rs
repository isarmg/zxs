//! Private Helper bridge. One inherited read-only state directory, no listener.
use crate::{
    display,
    fs::{HeldDir, OpaquePin, ReadAccess, local_uid},
    headless_auth::Descriptor,
    headless_start::{DEFAULT_STATE, DESCRIPTOR, SelectedStart},
};
use std::{
    collections::BTreeSet,
    ffi::{c_int, c_uint},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};

unsafe extern "C" {
    fn zxs_open_lock_fdinfo(target: c_int) -> c_int;
    fn zxs_lock_device(device_major: c_uint, device_minor: c_uint) -> u64;
}

const FDINFO_BYTES: usize = 8 * 1024;
const MAGIC: &[u8; 8] = b"ZXCAT1\0\0";
const END: &[u8; 8] = b"ZXEND1\0\0";

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the original exclusive offline state lease was not proved",
    )
}

pub(crate) fn run() -> io::Result<()> {
    let uid = local_uid();
    if matches!(uid, 0 | 65534 | u32::MAX) {
        return Err(refused());
    }
    let lease = File::from(io::stdin().as_fd().try_clone_to_owned()?);
    let state = HeldDir::trusted_storage(Path::new(DEFAULT_STATE))?;
    verify_lease(&lease, &state, uid)?;
    let original = state
        .opaque_pin_with(DESCRIPTOR, ReadAccess::NoAtime)?
        .ok_or_else(refused)?;
    let (descriptor, binding) = Descriptor::read_bound(&original.file, uid)?;
    original.require(&state, DESCRIPTOR)?;
    let selection = SelectedStart::from_original(state, descriptor, ReadAccess::NoAtime)?;
    let names = selection.offline_names(|state| {
        verify_lease(&lease, state, uid)?;
        verify_descriptor(state, &original, uid, binding)
    })?;
    let mut output = io::stdout().lock();
    write_frame(&mut output, binding, &names)?;
    output.flush()
}

fn verify_descriptor(
    state: &HeldDir,
    original: &OpaquePin,
    uid: u32,
    binding: [u8; 32],
) -> io::Result<()> {
    original.require(state, DESCRIPTOR)?;
    let named = state
        .opaque_pin_with(DESCRIPTOR, ReadAccess::NoAtime)?
        .ok_or_else(refused)?;
    if named.stamp() != original.stamp() || Descriptor::binding(&named.file, uid)? != binding {
        return Err(refused());
    }
    named.require(state, DESCRIPTOR)?;
    original.require(state, DESCRIPTOR)
}

fn verify_lease(lease: &File, state: &HeldDir, uid: u32) -> io::Result<()> {
    state.require_named()?;
    let meta = lease.metadata()?;
    if !meta.is_dir()
        || meta.uid() != uid
        || meta.mode() & 0o7777 != 0o700
        || (meta.dev(), meta.ino()) != (state.entity.device, state.entity.inode)
        || !state.shares_file_mount(lease)?
    {
        return Err(refused());
    }
    // SAFETY: lease owns the exact FD through this open. The native helper
    // opens its numeric child under the authentic current procfs fdinfo.
    let fd = unsafe { zxs_open_lock_fdinfo(lease.as_raw_fd()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful open transferred one new fd to the caller.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    file.take((FDINFO_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if !exclusive_fdinfo(&bytes, meta.dev(), meta.ino()) {
        return Err(refused());
    }
    state.require_named()
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn coordinates(value: &str, device: u64, inode: u64) -> bool {
    let mut fields = value.split(':');
    let (Some(major), Some(minor), Some(ino), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let (Ok(major), Ok(minor)) = (
        u32::from_str_radix(major, 16),
        u32::from_str_radix(minor, 16),
    ) else {
        return false;
    };
    // SAFETY: makedev is pure integer packing on the target Linux libc.
    let packed = unsafe { zxs_lock_device(major, minor) };
    packed == device && decimal(ino) == Some(inode)
}

fn exclusive_fdinfo(bytes: &[u8], device: u64, inode: u64) -> bool {
    if bytes.len() > FDINFO_BYTES || !bytes.ends_with(b"\n") {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let (mut flags, mut observed, mut lock) = (None, None, false);
    for line in text.lines() {
        let mut columns = line.split_ascii_whitespace();
        match columns.next() {
            Some("flags:") if flags.is_none() => {
                flags = columns.next().and_then(|f| u64::from_str_radix(f, 8).ok());
                if flags.is_none() || columns.next().is_some() {
                    return false;
                }
            }
            Some("ino:") if observed.is_none() => {
                observed = columns.next().and_then(decimal);
                if observed.is_none() || columns.next().is_some() {
                    return false;
                }
            }
            Some("lock:") if !lock => {
                let [
                    Some(slot),
                    Some("FLOCK"),
                    Some("ADVISORY"),
                    Some("WRITE"),
                    Some(pid),
                    Some(location),
                    Some("0"),
                    Some("EOF"),
                ] = std::array::from_fn(|_| columns.next())
                else {
                    return false;
                };
                if columns.next().is_some() {
                    return false;
                }
                lock = slot
                    .strip_suffix(':')
                    .and_then(decimal)
                    .is_some_and(|n| n != 0)
                    && decimal(pid).is_some()
                    && coordinates(location, device, inode);
                if !lock {
                    return false;
                }
            }
            Some("flags:" | "ino:" | "lock:") => return false,
            _ => {}
        }
    }
    lock && flags.is_some_and(|bits| bits & 3 == 0) && observed == Some(inode)
}

fn write_frame(
    output: &mut impl Write,
    binding: [u8; 32],
    names: &BTreeSet<String>,
) -> io::Result<()> {
    let count = u64::try_from(names.len()).map_err(|_| refused())?;
    for name in names {
        display::component(name.as_bytes(), display::MAX).map_err(|_| refused())?;
    }
    output.write_all(MAGIC)?;
    output.write_all(&binding)?;
    output.write_all(&count.to_be_bytes())?;
    for name in names {
        output.write_all(
            &u16::try_from(name.len())
                .map_err(|_| refused())?
                .to_be_bytes(),
        )?;
        output.write_all(name.as_bytes())?;
    }
    output.write_all(END)
}

#[cfg(test)]
#[path = "../tests/unit/offline_worker.rs"]
mod tests;
