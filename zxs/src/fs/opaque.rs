//! Mount-bound original regular files for whole opaque container retirements.
use super::{
    HeldDir, invalid, local_uid, mounts, os_result, zxs_open_pinned_record, zxs_remove_empty_dir,
    zxs_remove_name,
};
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        raw::{c_char, c_int},
        unix::fs::MetadataExt,
    },
};

pub(crate) const OPAQUE_STAMP_BYTES: usize = 56;

pub(crate) struct OpaquePin {
    pub(crate) file: File,
    stamp: [u8; OPAQUE_STAMP_BYTES],
    mount: u64,
}
impl OpaquePin {
    pub(crate) fn stamp(&self) -> [u8; OPAQUE_STAMP_BYTES] {
        self.stamp
    }
    pub(crate) fn require(&self, parent: &HeldDir, name: &CStr) -> io::Result<()> {
        parent.require_named()?;
        let named = parent
            .pin_record(name)?
            .ok_or_else(|| invalid("original opaque record absent"))?;
        if self.mount != mounts::id(&parent.file)?
            || self.mount != mounts::id(&named)?
            || self.mount != mounts::id(&self.file)?
            || opaque_stamp(&named)? != self.stamp
            || opaque_stamp(&self.file)? != self.stamp
        {
            return Err(invalid("opaque file differs from the witnessed original"));
        }
        Ok(())
    }
}
fn opaque_stamp(file: &File) -> io::Result<[u8; OPAQUE_STAMP_BYTES]> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.ino() == 0
        || m.nlink() != 1
        || m.uid() != local_uid()
        || m.mode() & 0o7777 != 0o600
    {
        return Err(invalid("not an original private opaque file"));
    }
    let fields = [
        m.dev().to_be_bytes(),
        m.ino().to_be_bytes(),
        m.len().to_be_bytes(),
        m.mtime().to_be_bytes(),
        m.mtime_nsec().to_be_bytes(),
        m.ctime().to_be_bytes(),
        m.ctime_nsec().to_be_bytes(),
    ];
    let mut out = [0; OPAQUE_STAMP_BYTES];
    for (slot, bytes) in out.as_chunks_mut::<8>().0.iter_mut().zip(fields) {
        *slot = bytes;
    }
    Ok(out)
}
impl HeldDir {
    pub(crate) fn opaque_pin(&self, name: &CStr) -> io::Result<Option<OpaquePin>> {
        self.require_named()?;
        let Some(pin) = self.pin_record(name)? else {
            return Ok(None);
        };
        let stamp = opaque_stamp(&pin)?;
        let mount = mounts::id(&self.file)?;
        if mount != mounts::id(&pin)? {
            return Err(invalid("opaque file crosses its enrolled mount"));
        }
        // SAFETY: reopen the already validated held, regular O_PATH inode.
        let raw = unsafe { zxs_open_pinned_record(pin.as_raw_fd(), 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful native open returned one owning FD.
        let file = unsafe { File::from_raw_fd(raw) };
        let result = OpaquePin { file, stamp, mount };
        result.require(self, name)?;
        Ok(Some(result))
    }
    pub(crate) fn unlink_opaque_original(&self, name: &CStr, pin: &OpaquePin) -> io::Result<()> {
        pin.require(self, name)?;
        // SAFETY: the original held parent and exact pinned regular name
        // were just validated under the application's exclusive lease.
        os_result(unsafe { zxs_remove_name(self.file.as_raw_fd(), name.as_ptr()) })?;
        self.confirm_absence(name)
    }
    pub(crate) fn unlink_empty_opaque_original(
        &self,
        name: &CStr,
        exact: &HeldDir,
    ) -> io::Result<()> {
        exact.require_named()?;
        exact.only(None)?;
        self.require_named()?;
        let named = self
            .child_directory(name)?
            .ok_or_else(|| invalid("original empty opaque directory absent"))?;
        if named.entity != exact.entity || !named.same_mount(exact)? || !self.same_mount(exact)? {
            return Err(invalid(
                "opaque directory name has different original custody",
            ));
        }
        // SAFETY: AT_REMOVEDIR acts on only the exact original empty child.
        os_result(unsafe { zxs_remove_empty_dir(self.file.as_raw_fd(), name.as_ptr()) })?;
        self.confirm_absence(name)
    }
}

impl HeldDir {
    /// Exclusively create one ordinary record relative to the held staging FD.
    pub(crate) fn create_opaque_record(&self, name: &CStr) -> io::Result<File> {
        self.checked_child_path(name)?;
        self.require_named()?;
        // SAFETY: the parent FD and checked component are held for Linux exclusive creation.
        let fd = unsafe { zxs_create_new_regular(self.file.as_raw_fd(), name.as_ptr()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation returned one owning FD.
        let file = unsafe { File::from_raw_fd(fd) };
        let named = self
            .opaque_pin(name)?
            .ok_or_else(|| invalid("new staging record lost its name"))?;
        if !self.shares_file_mount(&file)? || named.stamp() != opaque_stamp(&file)? {
            return Err(invalid("new staging record differs from the original file"));
        }
        Ok(file)
    }

    /// Acquire one exact private service lease record below the already
    /// enrolled root. Neither open branch resolves the root by pathname.
    pub(crate) fn opaque_owner(&self, name: &CStr) -> io::Result<File> {
        self.checked_child_path(name)?;
        self.require_named()?;
        let file = if let Some(pin) = self.opaque_pin(name)? {
            if pin.file.metadata()?.len() != 0 {
                return Err(invalid("service lease record is not empty"));
            }
            // SAFETY: reopen the exact bounded private inode for its
            // advisory exclusive lease, not a newly resolved pathname.
            let fd = unsafe { zxs_open_pinned_record(pin.file.as_raw_fd(), 1) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful native open returns one owning FD.
            let file = unsafe { File::from_raw_fd(fd) };
            if !self.shares_file_mount(&file)? || opaque_stamp(&file)? != pin.stamp() {
                return Err(invalid("opened service lease is not the original record"));
            }
            file
        } else {
            // SAFETY: one checked relative component, supplied root FD,
            // and exclusive-create semantics; never replace a name.
            let fd = unsafe { zxs_create_new_regular(self.file.as_raw_fd(), name.as_ptr()) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful native creation returns one owning FD.
            unsafe { File::from_raw_fd(fd) }
        };
        file.try_lock().map_err(io::Error::from)?;
        let named = self
            .opaque_pin(name)?
            .ok_or_else(|| invalid("original service lease is no longer named"))?;
        if file.metadata()?.len() != 0
            || !self.shares_file_mount(&file)?
            || named.stamp() != opaque_stamp(&file)?
        {
            return Err(invalid(
                "locked service lease differs from the original named record",
            ));
        }
        self.file.sync_all()?;
        self.require_named()?;
        Ok(file)
    }

    /// Initialize a private application directory strictly relative to the
    /// supplied volume FD while that volume's service lease is held.
    pub(crate) fn opaque_subdirectory(&self, name: &CStr) -> io::Result<HeldDir> {
        let child = match self.child_directory(name)? {
            Some(child) => child,
            None => self.new_child_directory(name)?,
        };
        if !self.same_mount(&child)? {
            return Err(invalid("service directory crossed its enrolled mount"));
        }
        child.file.sync_all()?;
        self.file.sync_all()?;
        self.require_named()?;
        Ok(child)
    }
}

unsafe extern "C" {
    fn zxs_create_new_regular(parent: c_int, name: *const c_char) -> c_int;
}
