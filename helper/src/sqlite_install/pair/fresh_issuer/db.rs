//! Carry the same original SQLite inode through the two foreground cuts,
//! without ever opening a root-readable SQLite or changing a VFS lock.
use super::bad;
use crate::pinned::{Entity, Mount};
use rustix::{
    fs::{Mode, OFlags, ResolveFlags, fcntl_getfl, openat2},
    io::{FdFlags, fcntl_getfd},
};
use std::{fs::File, io, os::unix::fs::MetadataExt};

pub(in crate::sqlite_install::pair) struct InitialDatabase {
    original: File,
    place: [u64; 3],
    uid: u32,
}
impl InitialDatabase {
    pub(in crate::sqlite_install::pair) fn received(original: File, uid: u32) -> io::Result<Self> {
        let place = Self::scope(&original, uid)?;
        Ok(Self {
            original,
            place,
            uid,
        })
    }
    pub(in crate::sqlite_install::pair) fn scope(fd: &File, uid: u32) -> io::Result<[u64; 3]> {
        let f = fcntl_getfl(fd)?;
        let m = fd.metadata()?;
        let e = Entity::of(&m)?;
        let mount = Mount::on(fd)?
            .filter(|x| x.id != 0)
            .ok_or_else(|| bad("no original fresh SQLite mount"))?;
        if uid == 0
            || m.uid() != uid
            || m.mode() != 0o100_600
            || m.nlink() != 1
            || m.len() == 0
            || !f.contains(OFlags::PATH)
            || !fcntl_getfd(fd)?.contains(FdFlags::CLOEXEC)
            || e.ino == 0
        {
            return Err(bad(
                "only the exact non-readable private fresh SQLite pin may cross this lane",
            ));
        }
        Ok([e.dev, e.ino, mount.id])
    }
    pub(in crate::sqlite_install::pair) fn named(&self, system: &File) -> io::Result<[u64; 3]> {
        let fd = File::from(openat2(
            system,
            c"SYSTEM.sqlite3",
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_XDEV
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_SYMLINKS,
        )?);
        let expected = self.original()?;
        if Self::scope(&fd, self.uid)? != expected {
            return Err(bad("the named fresh SQLite original was replaced"));
        }
        Ok(expected)
    }
    // Metadata only, on the same non-readable SQLite inode. The agreeing
    // low reader and final host verifier use this only for a short observation.
    pub(in crate::sqlite_install::pair) fn observation_on(
        fd: &File,
        uid: u32,
    ) -> io::Result<[u8; 64]> {
        let place = Self::scope(fd, uid)?;
        let m = fd.metadata()?;
        let mut out = [0; 64];
        for (slot, value) in
            out[..32]
                .as_chunks_mut::<8>()
                .0
                .iter_mut()
                .zip([place[0], place[1], place[2], m.len()])
        {
            *slot = value.to_be_bytes();
        }
        for (slot, value) in out[32..].as_chunks_mut::<8>().0.iter_mut().zip([
            m.ctime(),
            m.ctime_nsec(),
            m.mtime(),
            m.mtime_nsec(),
        ]) {
            *slot = value.to_be_bytes();
        }
        if Self::scope(fd, uid)? != place {
            return Err(bad("different non-readable original SYSTEM database"));
        }
        Ok(out)
    }
    pub(in crate::sqlite_install::pair) fn original(&self) -> io::Result<[u64; 3]> {
        if Self::scope(&self.original, self.uid)? != self.place {
            return Err(bad("changed or removed original fresh SQLite pin"));
        }
        Ok(self.place)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    #[test]
    fn fresh_v18_original_db_pin_rejects_readable_fd_and_same_byte_named_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("SYSTEM.sqlite3");
        std::fs::write(&path, b"fixture-owned").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let dir = File::open(tmp.path()).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let now = || {
            File::from(
                openat2(
                    &dir,
                    c"SYSTEM.sqlite3",
                    OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                    ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS,
                )
                .unwrap(),
            )
        };
        let active = std::fs::OpenOptions::new()
            .read(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        assert!(InitialDatabase::received(active, uid).is_err());
        if uid == 0 {
            assert!(InitialDatabase::received(now(), uid).is_err());
            return;
        }
        let kept = InitialDatabase::received(now(), uid).unwrap();
        kept.named(&dir).unwrap();
        let prior = tmp.path().join("old");
        std::fs::rename(&path, &prior).unwrap();
        std::fs::copy(&prior, &path).unwrap();
        kept.original().unwrap();
        assert!(kept.named(&dir).is_err());
    }
}
