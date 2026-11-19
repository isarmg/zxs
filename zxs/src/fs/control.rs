//! One named, one-link ordinary control file in a caller-held protected
//! parent. No export handle, xattr, private filesystem ioctl or media writes.
use super::*;
use std::os::unix::fs::FileExt;

pub(crate) struct ControlRead {
    name: &'static CStr,
    file: File,
    opening: FileStamp,
    parent: Entity,
    mount: u64,
    limit: u64,
}
impl ControlRead {
    pub(crate) fn open(dir: &HeldDir, name: &'static CStr, limit: usize) -> io::Result<Self> {
        let limit = u64::try_from(limit).map_err(|_| invalid("control length limit overflow"))?;
        dir.checked_child_path(name)?;
        let pin = dir.pin_record(name)?.ok_or_else(|| invalid("missing portable control file"))?;
        let opening = record_stamp(&pin, limit)?;
        let mount = mounts::id(&dir.file)?;
        if opening.dev != dir.entity.device || mounts::id(&pin)? != mount {
            return Err(invalid("portable record is not on its selected parent mount"));
        }
        // Only activate the pinned ordinary file; never chase the user path
        // a second time to obtain a content FD (FIFO/device/symlink).
        let fd = unsafe { zxs_open_pinned_record(pin.as_raw_fd(), 0) };
        if fd < 0 { return Err(io::Error::last_os_error()); }
        let file = unsafe { File::from_raw_fd(fd) };
        let held = Self { name, file, opening, parent:dir.entity, mount, limit };
        held.require(dir)?;
        Ok(held)
    }
    /// Only the original REGISTRATION is a lifetime lock target. Floor/mark
    /// reads take the existing short directory cut, not additional flocks.
    pub(crate) fn shared_registration(dir: &HeldDir, limit: usize) -> io::Result<Self> {
        let r = Self::open(dir, crate::pool::READY, limit)?;
        r.file.try_lock_shared().map_err(io::Error::from)?;
        r.require(dir)?;
        Ok(r)
    }
    pub(crate) fn require(&self, dir: &HeldDir) -> io::Result<()> {
        if dir.entity != self.parent || mounts::id(&dir.file)? != self.mount {
            return Err(invalid("different original parent of the portable control file"));
        }
        let named = dir.pin_record(self.name)?.ok_or_else(|| invalid("portable control name vanished"))?;
        if mounts::id(&self.file)? != self.mount ||
            mounts::id(&named)? != self.mount || record_stamp(&self.file, self.limit)? != self.opening ||
            record_stamp(&named, self.limit)? != self.opening {
            return Err(invalid("portable control file or its original name changed"));
        }
        Ok(())
    }
    pub(crate) fn bytes(&self, dir: &HeldDir) -> io::Result<Vec<u8>> {
        self.require(dir)?;
        let n = usize::try_from(self.opening.len).map_err(|_| invalid("portable record too large"))?;
        let mut b = vec![0; n];
        self.file.read_exact_at(&mut b, 0)?;
        self.require(dir)?;
        Ok(b)
    }
}
