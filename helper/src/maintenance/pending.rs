//! Original SYSTEM + pending register only. This is not a complete-root proof or a resume permit.
//! No stored path is opened. No tags, file bytes, names or synchronization are written here.
use super::{
    genesis::{FreshGenesis, MAX_IMAGE},
    identity::{Here, ObservedOriginal, mount_id},
    root_set::{DirectoryStamp, read_pending_only},
};
use crate::pinned::{Entity, Kind, Pinned, invalid, private_inode_reader};
use helper_linux_evidence::{component_limit, read_birth};
use rustix::{
    fs::{FlockOperation::NonBlockingLockExclusive, Mode, OFlags, fgetxattr, flock, openat},
    io::Errno,
};
use std::{fs::File, io, os::unix::{ffi::OsStrExt, fs::{FileExt, MetadataExt}}, path::Path};

#[derive(PartialEq, Eq)]
struct Mark { at: Here, length: u64, mtime: (i64, i64), ctime: (i64, i64) }
impl Mark {
    fn on(fd: &File, system: &Here, uid: u32) -> io::Result<Self> {
        let m = fd.metadata()?;
        if !m.is_file() || m.uid() != uid || m.mode() & 0o7777 != 0o600 || m.nlink() != 1
            || m.len() == 0 || m.len() > MAX_IMAGE as u64 || m.dev() != system.entity.dev {
            return Err(invalid("pending register is not a bounded singly linked private regular original"));
        }
        let at = Here { entity: Entity::of(&m)?, mount: mount_id(fd)? };
        if at.mount != system.mount || at.entity == system.entity {
            return Err(invalid("pending register is not a distinct inode on the original SYSTEM mount"));
        }
        Ok(Self { at, length: m.len(), mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()) })
    }
}

fn pin_pending(dir: &File) -> io::Result<File> {
    // Inspect kind BEFORE upgrading: even a FIFO, device or link cannot be opened for data.
    Ok(File::from(openat(dir, c"INITIALIZING",
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?))
}

struct Source {
    name: Pinned, system: File, s: ObservedOriginal,
    register: File, r: ObservedOriginal, uid: u32, op: [u8; 16],
}

/// The original independently opened, read-only ordinary register FD holds EX for this
/// object's lifetime. This does not authorize maintenance or prove the online service is off.
#[must_use]
pub(super) struct ColdPending { source: Source, observed: PendingImage }

#[derive(PartialEq, Eq)]
struct PendingImage { bytes: Vec<u8>, system_labeled: bool }
impl ColdPending {
    pub(super) fn inspect(system_path: &Path, storage_uid: u32, init_op: [u8; 16]) -> io::Result<Self> {
        if init_op == [0; 16] { return Err(invalid("a nonzero original initialization operation is required")); }
        let name = Pinned::open(system_path, Kind::StorageRoot)?;
        let system = name.storage_directory()?;
        let s = ObservedOriginal::root(&system, storage_uid)?;
        let pin = pin_pending(&system)?;
        let selected = Mark::on(&pin, &s.at, storage_uid)?;
        let register = private_inode_reader(&pin, storage_uid, s.at.mount)?;
        flock(&register, NonBlockingLockExclusive)?;
        let held = Mark::on(&register, &s.at, storage_uid)?;
        if held != selected { return Err(invalid("pending register changed before its original lock was acquired")); }
        let birth = read_birth(&register, held.at.mount)?;
        let r = ObservedOriginal { at: held.at, birth };
        let source = Source { name, system, s, register, r, uid: storage_uid, op: init_op };
        let observed = source.read_original()?;
        Ok(Self { source, observed })
    }

    pub(super) fn recheck(&self) -> io::Result<()> {
        if self.source.read_original()? != self.observed {
            return Err(invalid("stored pending image changed since the original observation"));
        }
        Ok(())
    }
}

impl Source {
    fn named_system(&self) -> io::Result<File> {
        let named = self.name.storage_directory()?;
        if ObservedOriginal::root(&named, self.uid)? != self.s
            || ObservedOriginal::root(&self.system, self.uid)? != self.s {
            return Err(invalid("pending SYSTEM no longer has its original name and birth"));
        }
        Ok(named)
    }

    fn named_register(&self) -> io::Result<Mark> {
        let held = Mark::on(&self.register, &self.s.at, self.uid)?;
        if held.at != self.r.at || read_birth(&self.register, held.at.mount)? != self.r.birth {
            return Err(invalid("held pending registration is not its original birth"));
        }
        let named = pin_pending(&self.system)?;
        if Mark::on(&named, &self.s.at, self.uid)? != held
            || Mark::on(&self.register, &self.s.at, self.uid)? != held {
            return Err(invalid("pending name and held original differ"));
        }
        Ok(held)
    }

    fn read_original(&self) -> io::Result<PendingImage> {
        let named = self.named_system()?;
        let before_dir = DirectoryStamp::on(&named)?;
        let before = self.named_register()?;
        read_pending_only(named)?; // Reject READY, an extra entry, an inconsistent stream or a missing name.
        let mut image = vec![0; before.length as usize]; // Mark already bounded it
        self.register.read_exact_at(&mut image, 0)?;
        loop {
            match self.register.read_at(&mut [0], before.length) {
                Ok(0) => break,
                Ok(_) => return Err(invalid("unexpected tail on pending GENESIS")),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let g = FreshGenesis::decode(&image).map_err(|_| invalid("not one intact MAINT-1 pending GENESIS"))?;
        if !g.has_init_operation(&self.op) || !g.matches_origin_pair(self.uid, self.r.fields(), self.s.fields())
            || !g.matches_root_origin(0, self.name.path().as_os_str().as_bytes(),
                component_limit(&self.system)?, self.s.fields()) {
            return Err(invalid("pending operation or original register/SYSTEM does not match the explicit selection"));
        }

        let mut frontier = [0; 48];
        let n = fgetxattr(&self.register, c"user.zxs.confirmed-frontier", &mut frontier[..])?;
        g.check_initial_frontier(Some(&frontier[..n])).map_err(|_| invalid("pending original lacks its exact zero frontier"))?;
        let mut label = [0; 160]; // larger than any v1 SYSTEM label; ERANGE refuses
        let system_labeled = match fgetxattr(&self.system, c"user.zxs.volume", &mut label[..]) {
            Err(Errno::NODATA) => false, // missing is a possible PENDING, not proof of completion
            Ok(n) => {
                g.check_label(0, &label[..n]).map_err(|_| invalid("unexpected pending SYSTEM label"))?;
                true
            }
            Err(e) => return Err(e.into()),
        };

        // The directory can change even with an advisory flock on the regular file.
        // Only an unchanged named original and unchanged sampled intervals qualify.
        read_pending_only(self.named_system()?)?;
        let end = self.named_system()?;
        if self.named_register()? != before || DirectoryStamp::on(&end)? != before_dir
            || DirectoryStamp::on(&self.system)? != before_dir {
            return Err(invalid("pending SYSTEM, register, or attributes changed during observation"));
        }
        Ok(PendingImage { bytes: image, system_labeled })
    }
}
// Native controls share the private test fixture that created the actual anonymous original:
// tests/maintenance/pending.rs. Production has no route to perform the fixture's first link.
