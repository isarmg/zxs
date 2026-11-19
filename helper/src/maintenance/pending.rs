//! Original SYSTEM + pending register only. This is not a complete-root proof or a resume permit.
//! No stored path is opened. No tags, file bytes, names or synchronization are written here.
use super::{
    genesis::{FreshGenesis, MAX_IMAGE},
    identity::{Here, ObservedOriginal, mount_id},
    root_set::{DirectoryStamp, read_pending_only},
    register_flock::staging::first_name::FirstLinkedRegister,
};
use crate::pinned::{Entity, Kind, Pinned, invalid, private_inode_reader};
use helper_linux_evidence::{component_limit, read_birth};
use rustix::{
    fs::{FlockOperation::NonBlockingLockExclusive, Mode, OFlags, fgetxattr, flock, openat},
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

enum Registration { Cold(File), First(FirstLinkedRegister) }
impl Registration {
    fn reader(&self) -> &File {
        match self { Self::Cold(file) => file, Self::First(linked) => linked.original() }
    }
}

struct Source {
    name: Pinned, system: File, s: ObservedOriginal,
    register: Registration, r: ObservedOriginal, uid: u32, op: [u8; 16],
}

/// The original ordinary register FD holds EX (independent read-only cold open, or the
/// very same open description handed over after its first link) for this
/// object's lifetime. This does not authorize maintenance or prove the online service is off.
#[must_use]
pub(super) struct ColdPending { source: Source, observed: PendingImage }

#[derive(PartialEq, Eq)]
struct PendingImage { bytes: Vec<u8>, system_labeled: bool }
impl ColdPending {
    pub(super) fn inspect(system_path: &Path, storage_uid: u32, init_op: [u8; 16]) -> io::Result<Self> {
        Self::inspect_held(system_path, storage_uid, init_op, None)
    }

    // The warm input is the sealed original just linked and synced by this process.
    // It keeps its EX throughout, without re-locking. Its bytes and fixed name
    // are still read back independently; receiving this token is not operator approval.
    pub(super) fn from_first_name(system_path: &Path, uid: u32, op: [u8; 16],
                                 register: FirstLinkedRegister, expected: &[u8]) -> io::Result<Self> {
        let pending = Self::inspect_held(system_path, uid, op, Some(register))?;
        if pending.observed.system_labeled || pending.observed.bytes.as_slice() != expected {
            return Err(invalid("first name differs from the prepared image or SYSTEM was already claimed"));
        }
        Ok(pending)
    }

    fn inspect_held(system_path: &Path, storage_uid: u32, init_op: [u8; 16], prior: Option<FirstLinkedRegister>) -> io::Result<Self> {
        if init_op == [0; 16] { return Err(invalid("a nonzero original initialization operation is required")); }
        let name = Pinned::open(system_path, Kind::StorageRoot)?;
        let system = name.storage_directory()?;
        let s = ObservedOriginal::root(&system, storage_uid)?;
        let pin = pin_pending(&system)?;
        let selected = Mark::on(&pin, &s.at, storage_uid)?;
        let register = match prior {
            Some(original) => Registration::First(original),
            None => {
                let file = private_inode_reader(&pin, storage_uid, s.at.mount)?;
                flock(&file, NonBlockingLockExclusive)?;
                Registration::Cold(file)
            }
        };
        let held = Mark::on(register.reader(), &s.at, storage_uid)?;
        if held != selected { return Err(invalid("pending register changed before its original lock was acquired")); }
        let birth = read_birth(register.reader(), held.at.mount)?;
        let r = ObservedOriginal { at: held.at, birth };
        let source = Source { name, system, s, register, r, uid: storage_uid, op: init_op };
        let observed = source.read_original()?;
        Ok(Self { source, observed })
    }

    /// Cold observation did not sync or advance anything. Before any explicit
    /// label application make the existing first name durable on the original
    /// selected SYSTEM. Our own first link already synchronized this same parent.
    pub(super) fn ensure_name_durable(&self) -> io::Result<()> {
        self.recheck()?;
        if matches!(&self.source.register, Registration::Cold(_)) { self.source.system.sync_all()?; }
        self.recheck()
    }

    /// Only a successful CREATE + sync of this specific original SYSTEM label
    /// can advance the observation we retained. An unsolicited xattr change,
    /// equal bytes on another inode or success from another pool is not adopted.
    pub(super) fn observe_own_system_label(&mut self, created: super::root_set::SystemLabelCreated<'_>) -> io::Result<()> {
        let now = self.source.read_original()?;
        let g = FreshGenesis::decode(&self.observed.bytes).map_err(|_| invalid("invalid original pending image"))?;
        if self.observed.system_labeled || !now.system_labeled || now.bytes != self.observed.bytes
            || g.check_label(0, created.bytes()).is_err() {
            return Err(invalid("SYSTEM did not retain its own just-created original label"));
        }
        self.observed = now;
        Ok(())
    }

    pub(super) fn storage_uid(&self) -> u32 { self.source.uid }

    /// Couple the complete group's row 0 to THIS retained SYSTEM, including its
    /// in-process mount ID and label state (neither is implied by the stored row).
    pub(super) fn check_system_observation(&self, root: &ObservedOriginal, labelled: bool) -> io::Result<()> {
        if root != &self.source.s || labelled != self.observed.system_labeled {
            return Err(invalid("pending registration and selected volumes did not observe the same SYSTEM"));
        }
        Ok(())
    }

    pub(super) fn verified_genesis(&self) -> io::Result<FreshGenesis<'_>> {
        self.recheck()?;
        FreshGenesis::decode(&self.observed.bytes).map_err(|_| invalid("invalid observed pending image"))
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
        let held = Mark::on(self.register.reader(), &self.s.at, self.uid)?;
        if held.at != self.r.at || read_birth(self.register.reader(), held.at.mount)? != self.r.birth {
            return Err(invalid("held pending registration is not its original birth"));
        }
        let named = pin_pending(&self.system)?;
        if Mark::on(&named, &self.s.at, self.uid)? != held
            || Mark::on(self.register.reader(), &self.s.at, self.uid)? != held {
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
        self.register.reader().read_exact_at(&mut image, 0)?;
        loop {
            match self.register.reader().read_at(&mut [0], before.length) {
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
        let n = fgetxattr(self.register.reader(), c"user.zxs.confirmed-frontier", &mut frontier[..])?;
        g.check_initial_frontier(Some(&frontier[..n])).map_err(|_| invalid("pending original lacks its exact zero frontier"))?;
        let system_labeled = super::identity::original_pending_label(&self.system, &g, 0)?;

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
// The private fixtures exercise genuinely cold opens, independently of the warm handoff.
