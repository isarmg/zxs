//! Single transition: empty anonymous original -> synchronized anonymous original.
//! No producer here touches a root attribute, a directory name or a previously named file.
use super::UnpublishedRegisterFlock;
use crate::maintenance::{genesis::{FreshGenesis, InitialBytes}, root_set::{FreshRootLocks, UnpublishedProposal}};
use crate::pinned::invalid;
use rustix::{fs::{XattrFlags, fgetxattr, fsetxattr}, io::Errno};
use std::{ffi::CStr, fs::File, io, os::unix::fs::{FileExt, MetadataExt}};

const FRONTIER: &CStr = c"user.zxs.confirmed-frontier";

// A stamp only detects changes during this read. It never replaces byte/EOF,
// frontier or original-inode verification. A read may legitimately change atime.
#[derive(PartialEq, Eq)]
struct ContentStamp { mtime: (i64, i64), ctime: (i64, i64) }
impl ContentStamp {
    fn on(fd: &File, length: u64) -> io::Result<Self> {
        let m = fd.metadata()?;
        if !m.is_file() || m.nlink() != 0 || m.len() != length {
            return Err(invalid("original registration no longer has its exact unnamed shape"));
        }
        Ok(Self { mtime: (m.mtime(), m.mtime_nsec()), ctime: (m.ctime(), m.ctime_nsec()) })
    }
}

#[must_use]
pub(in crate::maintenance) struct SyncedUnlinked<'r> {
    roots: &'r FreshRootLocks,
    register: UnpublishedRegisterFlock<'r>,
    bytes: InitialBytes,
}

impl<'r> SyncedUnlinked<'r> {
    // There is no constructor accepting an arbitrary target FD, pathname, body
    // or caller-supplied assertion of a lock. The group remains borrowed throughout.
    pub(in crate::maintenance) fn prepare(proposal: UnpublishedProposal<'r>) -> io::Result<Self> {
        let (roots, register, bytes) = proposal.into_checked_parts()?;
        write_initial(&register, &bytes)?;
        let synced = Self { roots, register, bytes };
        synced.recheck()?;
        Ok(synced)
    }

    pub(in crate::maintenance) fn recheck(&self) -> io::Result<()> {
        let g = self.bytes.checked().map_err(|_| invalid("invalid original in-memory proposal"))?;
        // The potentially long ancestry/volume pass must not come after the
        // final body and frontier read. This is an ordered observation, not an atomic snapshot.
        self.roots.check_image_roots(&g)?;
        let stamp = self.sample_original(&g)?;
        self.finish_original(&g, stamp)
    }

    fn sample_original(&self, g: &FreshGenesis<'_>) -> io::Result<ContentStamp> {
        let length = self.bytes.image.len() as u64; // bounded by FreshGenesis::decode
        self.register.check_unlinked_image(g, length)?;
        let fd = &self.register.held;
        let stamp = ContentStamp::on(fd, length)?;
        let mut buf = [0u8; 8192];
        let mut offset = 0;
        for original in self.bytes.image.chunks(buf.len()) {
            let slice = &mut buf[..original.len()];
            fd.read_exact_at(slice, offset)?;
            if &*slice != original { return Err(invalid("registration bytes differ on the held original")); }
            offset += original.len() as u64;
        }
        loop {
            match fd.read_at(&mut buf[..1], offset) {
                Ok(0) => break,
                Ok(_) => return Err(invalid("unexpected tail on the original registration")),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let mut frontier = [0u8; 48];
        let n = fgetxattr(fd, FRONTIER, &mut frontier[..])?;
        if n != frontier.len() || frontier != self.bytes.frontier {
            return Err(invalid("confirmed frontier is not the exact original zero frontier"));
        }
        Ok(stamp)
    }

    fn finish_original(&self, g: &FreshGenesis<'_>, before: ContentStamp) -> io::Result<()> {
        let length = self.bytes.image.len() as u64;
        self.register.check_unlinked_image(g, length)?;
        if ContentStamp::on(&self.register.held, length)? != before {
            return Err(invalid("original registration changed during its final read"));
        }
        Ok(())
    }
}

// Private to this sealed transition. On failure its caller drops the original
// still-unlinked inode. Never truncate, overwrite an earlier frontier, or retry
// by finding another file. sync_all is not evidence of a published directory entry.
fn write_initial(register: &UnpublishedRegisterFlock<'_>, bytes: &InitialBytes) -> io::Result<()> {
    let g = bytes.checked().map_err(|_| invalid("invalid proposed initial image"))?;
    register.check_image_origins(&g)?; // nlink=0, MODE 0600, original Birth, size=0
    let fd = &register.held;
    let mut existing = [0u8; 1];
    match fgetxattr(fd, FRONTIER, &mut existing[..]) {
        Err(Errno::NODATA) => {}
        Ok(_) | Err(Errno::RANGE) => return Err(invalid("anonymous original already carries a frontier")),
        Err(e) => return Err(e.into()),
    }
    fd.write_all_at(&bytes.image, 0)?;
    register.check_unlinked_image(&g, bytes.image.len() as u64)?;
    fsetxattr(fd, FRONTIER, &bytes.frontier, XattrFlags::CREATE)?;
    fd.sync_all()
}

#[cfg(test)]
#[path = "../../../tests/maintenance/unlinked_stage.rs"]
mod tests;
