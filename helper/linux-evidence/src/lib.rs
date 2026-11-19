//! Fixed Linux ABI boundaries: held-object evidence and one close-on-exec barrier.
//! No path input, handle reopening, disk write or device control interface.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", target_pointer_width = "64")))]
compile_error!("helper-linux-evidence supports x86_64 Linux LP64 only");

pub mod diagnostic_process;
pub mod sqlite_vfs;

use linux_raw_sys::general as uapi;
use rustix::fs::{OFlags, fcntl_getfl, fstatfs};
use std::{fs::{File, Metadata}, io, mem::{align_of, offset_of, size_of}, os::fd::AsRawFd};

const CAPACITY: usize = 128;
// Linux include/uapi/linux/fs.h: FS_IOC_GETFSUUID = _IOR(0x15, 0, struct fsuuid2).
const _: () = assert!(size_of::<uapi::fsuuid2>() == 17 && offset_of!(uapi::fsuuid2, uuid) == 1);

// libc's file_handle ends in a flexible array. Allocate the full bounded buffer,
// with the same alignment and offsets; pass a pointer originating from the FULL object.
#[repr(C)]
struct HandleBuffer { len: libc::c_uint, kind: libc::c_int, opaque: [u8; CAPACITY] }
const _: () = {
    assert!(size_of::<libc::file_handle>() == 8 && size_of::<HandleBuffer>() == 8 + CAPACITY);
    assert!(align_of::<HandleBuffer>() == align_of::<libc::file_handle>());
    assert!(offset_of!(libc::file_handle, handle_bytes) == offset_of!(HandleBuffer, len));
    assert!(offset_of!(libc::file_handle, handle_type) == offset_of!(HandleBuffer, kind));
    assert!(offset_of!(libc::file_handle, f_handle) == offset_of!(HandleBuffer, opaque));
};

/// One whitelist for both opaque-data inspection and this narrower kernel call.
/// This is not a claim that every accepted filesystem actually implements either request.
pub fn supported_local_magic(magic: u32) -> bool {
    matches!(magic, uapi::EXT4_SUPER_MAGIC | uapi::XFS_SUPER_MAGIC | uapi::BTRFS_SUPER_MAGIC | uapi::F2FS_SUPER_MAGIC)
}

fn unsupported(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, message) }

/// Complete opaque birth payload, for equality / encoding only. Not an opener
/// or a log value. A mount ID is tied to this observation, never part of the payload.
#[derive(PartialEq, Eq)]
pub struct Birth(Vec<u8>);
impl Birth {
    pub fn payload(&self) -> &[u8] { &self.0 }
    pub fn filesystem_uuid(&self) -> &[u8] { &self.0[..16] }
}

fn filesystem_uuid(uuid: uapi::fsuuid2) -> io::Result<[u8; 16]> {
    if uuid.len != 16 || uuid.uuid == [0; 16] { return Err(unsupported("no complete nonzero filesystem UUID")); }
    Ok(uuid.uuid)
}

fn collect(uuid: [u8; 16], h: &HandleBuffer, mount: libc::c_int, expected_mount: u64) -> io::Result<Birth> {
    // AT_EMPTY_PATH without AT_HANDLE_MNT_ID_UNIQUE returns the ordinary int mount ID.
    // Compare the same legacy mount-ID domain as statx(STATX_MNT_ID); never truncate statx.
    if expected_mount == 0 || u64::from(mount.cast_unsigned()) != expected_mount {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file handle belongs to a different or unknown mount"));
    }
    let n = h.len as usize;
    if !(1..=CAPACITY).contains(&n) { return Err(unsupported("no complete in-range kernel file handle")); }
    let mut b = Vec::with_capacity(20 + n);
    b.extend_from_slice(&uuid);
    b.extend_from_slice(&h.kind.to_be_bytes());
    b.extend_from_slice(&h.opaque[..n]);
    Ok(Birth(b))
}

fn local_object(file: &File) -> io::Result<Metadata> {
    let m = file.metadata()?;
    if (!m.is_dir() && !m.is_file()) || fcntl_getfl(file)?.contains(OFlags::PATH) {
        return Err(unsupported("expected a real open local directory or regular file"));
    }
    if !u32::try_from(fstatfs(file)?.f_type).ok().is_some_and(supported_local_magic) {
        return Err(unsupported("object filesystem is not an admitted local data filesystem"));
    }
    Ok(m)
}

fn encoded_name_max(value: libc::c_long) -> io::Result<u8> {
    if value <= 0 { return Err(unsupported("native directory component limit is unavailable or indeterminate")); }
    Ok(value.min(255) as u8)
}

/// Component budget of this exact open directory, not a path-based guess.
/// POSIX permits -1 without setting errno. Both indeterminate and error refuse
/// the observation; never report an unrelated stale errno, zero, or default 255.
pub fn component_limit(dir: &File) -> io::Result<u8> {
    local_object(dir)?; // preserve the historical MAINT-1 input boundary
    portable_component_limit(dir)
}

/// Ordinary POSIX limit at this real directory; no filesystem brand or UUID
/// is involved. This cannot establish whether a mount is suitable for storage.
#[allow(unsafe_code)]
pub fn portable_component_limit(dir: &File) -> io::Result<u8> {
    if !dir.metadata()?.is_dir() || fcntl_getfl(dir)?.contains(OFlags::PATH) {
        return Err(unsupported("NAME_MAX requires a real open directory"));
    }
    // SAFETY: the borrowed real directory FD stays open. This fixed POSIX
    // query takes no pointer and changes neither directory nor filesystem.
    let raw = unsafe { libc::fpathconf(dir.as_raw_fd(), libc::_PC_NAME_MAX) };
    encoded_name_max(raw)
}

/// Read ONLY from the supplied live descriptor. The caller has separately
/// selected its path and obtained existing_mount from statx on this same FD.
/// No guess, path reopening, retry with a bigger buffer, or partial result.
#[allow(unsafe_code)]
pub fn read_birth(file: &File, existing_mount: u64) -> io::Result<Birth> {
    if existing_mount == 0 { return Err(unsupported("original mount is unknown")); }
    local_object(file)?;
    let mut uuid = uapi::fsuuid2 { len: 0, uuid: [0; 16] };
    let request = libc::_IOR::<uapi::fsuuid2>(0x15, 0);
    // SAFETY: one fixed Linux generic filesystem output ioctl, never a user request.
    // Live regular/directory FD on an allowed local FS; initialized UAPI buffer has
    // exactly the 17 bytes and lifetime required by this ioctl. It is not retained.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), request, &raw mut uuid) };
    if rc == -1 { return Err(io::Error::last_os_error()); }
    if rc != 0 { return Err(unsupported("kernel did not report a filesystem UUID")); }
    let uuid = filesystem_uuid(uuid)?;

    let mut h = HandleBuffer { len: CAPACITY as libc::c_uint, kind: 0, opaque: [0; CAPACITY] };
    let mut mount: libc::c_int = 0;
    // SAFETY: the empty static C string with AT_EMPTY_PATH selects this exact live
    // FD. The kernel may copy a header plus at most its declared 128 bytes into
    // the C-aligned complete allocation; the independent mount result is one int.
    // Both pointers are valid for the call only. EOVERFLOW is not a successful read.
    let rc = unsafe { libc::name_to_handle_at(file.as_raw_fd(), c"".as_ptr(),
        (&raw mut h).cast::<libc::file_handle>(), &raw mut mount, libc::AT_EMPTY_PATH) };
    if rc == -1 { return Err(io::Error::last_os_error()); }
    if rc != 0 { return Err(unsupported("kernel did not return a complete handle")); }
    collect(uuid, &h, mount, existing_mount)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn uuid() -> uapi::fsuuid2 { uapi::fsuuid2 { len: 16, uuid: [0xA4; 16] } }
    #[test]
    fn component_budget_uses_the_actual_positive_result_and_only_caps_over_255() {
        for (raw, wire) in [(1, 1), (37, 37), (254, 254), (255, 255), (300, 255), (libc::c_long::MAX, 255)] {
            assert_eq!(encoded_name_max(raw).unwrap(), wire);
        }
        assert!(encoded_name_max(-1).is_err());
        assert!(encoded_name_max(0).is_err());
    }
    #[test]
    fn encoding_keeps_all_handle_type_bits_and_only_the_reported_payload() {
        let mut h = HandleBuffer { len: 3, kind: i32::MIN + 1, opaque: [0xDE; 128] };
        h.opaque[..3].copy_from_slice(&[0, 0xFA, 1]);
        let b = collect(filesystem_uuid(uuid()).unwrap(), &h, 37, 37).unwrap();
        assert_eq!(b.filesystem_uuid(), &[0xA4; 16]);
        assert_eq!(&b.payload()[16..], &[0x80, 0, 0, 1, 0, 0xFA, 1]);
        for length in [1, 128] {
            h.len = length;
            assert_eq!(collect(filesystem_uuid(uuid()).unwrap(), &h, 37, 37).unwrap().payload().len(), 20 + length as usize);
        }
    }
    #[test]
    fn unavailable_truncated_oversize_and_wrong_mount_results_never_become_identifiers() {
        let mut h = HandleBuffer { len: 1, kind: 0, opaque: [0; 128] };
        assert!(filesystem_uuid(uapi::fsuuid2 { len: 15, ..uuid() }).is_err());
        assert!(filesystem_uuid(uapi::fsuuid2 { len: 16, uuid: [0; 16] }).is_err());
        for (observed, expected) in [(0, 0), (7, 8), (7, (1u64 << 32) + 7)] {
            assert!(collect(filesystem_uuid(uuid()).unwrap(), &h, observed, expected).is_err());
        }
        for length in [0, 129, u32::MAX] {
            h.len = length;
            assert!(collect(filesystem_uuid(uuid()).unwrap(), &h, 7, 7).is_err());
        }
    }
}

/// One-shot CLI launch boundary. Atomically privatize this thread's descriptor
/// table and mark *all* non-standard descriptors close-on-exec, including
/// unknown inherited ones. This does not close or retarget existing objects.
/// Only explicitly chosen read-only runtime/image FDs and the monitor pipe
/// may be reintroduced before spawn. No retries with a weaker descriptor set.
#[allow(unsafe_code)]
pub fn isolate_check_descriptors()->io::Result<()> {
    // SAFETY: close_range accepts only unsigned scalars; no pointer, Rust
    // object or descriptor is invalidated by the CLOEXEC operation.
    let result=unsafe {libc::syscall(libc::SYS_close_range,3u32,u32::MAX,
        libc::CLOSE_RANGE_CLOEXEC|libc::CLOSE_RANGE_UNSHARE)};
    if result==0 {Ok(())}else{Err(io::Error::last_os_error())}
}
