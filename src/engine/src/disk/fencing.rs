//! Typed ownership of acquired writer locks. Cloning never duplicates an FD.
use super::{ClosedDiskIo, DiskIoBinding, DiskIoLease, DiskRoot};
use crate::{Error, ErrorCode, Result};
use platform_io::fs_safety::linux::FileIdentity;
use std::{
    collections::BTreeMap,
    fs::File,
    sync::{Arc, Mutex},
};

/// Pending reservation and admitted generations share this exact OS ownership.
/// The directory reservation is retained with the writer until final retirement.
#[derive(Debug)]
pub(crate) struct FenceOwner {
    descriptor: Arc<FenceDescriptor>,
}

/// A gate keeps this cell through final closure even while the last owner's
/// destructor is running. It never owns an owner or admission gate in return.
#[derive(Debug)]
pub(super) struct FenceDescriptor {
    files: Mutex<Option<FenceFiles>>,
}

#[derive(Debug)]
struct FenceFiles {
    _reservation: Option<File>,
    writer: File,
    // A target's original directory reservation must outlive all admitted I/O
    // tails, not merely the task/root wrapper that first acquired its writer.
    _advisory_reservation: Option<Arc<platform_io::fs_safety::linux::AdvisoryLock>>,
}

impl FenceOwner {
    pub(crate) fn new(writer: File, reservation: Option<File>) -> Arc<Self> {
        Arc::new(Self {
            descriptor: Arc::new(FenceDescriptor {
                files: Mutex::new(Some(FenceFiles {
                    _reservation: reservation,
                    writer,
                    _advisory_reservation: None,
                })),
            }),
        })
    }

    pub(crate) fn with_advisory_reservation(
        writer: File,
        reservation: Arc<platform_io::fs_safety::linux::AdvisoryLock>,
    ) -> Arc<Self> {
        Arc::new(Self {
            descriptor: Arc::new(FenceDescriptor {
                files: Mutex::new(Some(FenceFiles {
                    _reservation: None,
                    writer,
                    _advisory_reservation: Some(reservation),
                })),
            }),
        })
    }

    pub(super) fn descriptor(&self) -> Arc<FenceDescriptor> {
        Arc::clone(&self.descriptor)
    }

    pub(crate) fn writer_identity(&self) -> Result<FileIdentity> {
        let files = self.descriptor.files.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "writer ownership lock poisoned",
            )
        })?;
        let files = files.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::DiskUnavailable,
                "disk writer ownership is closed",
            )
        })?;
        Ok(FileIdentity::from_fd(&files.writer)?)
    }
}

impl Drop for FenceOwner {
    fn drop(&mut self) {
        self.descriptor.close();
    }
}

impl FenceDescriptor {
    pub(super) fn close(&self) {
        let mut files = match self.files.lock() {
            Ok(files) => files,
            // Exclusive teardown only. No poisoned owner can be reused.
            Err(poisoned) => poisoned.into_inner(),
        };
        let retired = files.take();
        // Both owner Drop and final drain may close. The resource lock covers
        // actual File destruction, not only taking an Option out of its cell.
        drop(retired);
        drop(files);
    }
}

#[derive(Clone)]
pub(crate) struct DiskWriter {
    binding: DiskIoBinding,
    fence: Arc<FenceOwner>,
}

impl DiskWriter {
    pub(crate) fn adopt(root: &DiskRoot, fence: Arc<FenceOwner>) -> Result<Self> {
        let binding = root.io_binding()?.clone();
        let _admission = binding.admit()?;
        // Check the actual acquired owner before registering, outside the gate.
        fence.writer_identity()?;
        binding.register_fence(&fence)?;
        Ok(Self { binding, fence })
    }

    fn admit(&self) -> Result<DiskIoLease> {
        self.binding.admit()
    }

    fn held(&self) -> Result<()> {
        let _admission = self.admit()?;
        self.fence.writer_identity()?;
        Ok(())
    }
}

/// The system lock never joins a disk's revocation domain. Disk ownership is
/// keyed explicitly, not inferred from vector positions or filesystem paths.
#[derive(Clone)]
pub(crate) struct WriterLocks {
    system: Arc<File>,
    selection: Arc<crate::authority_selection::SelectionGuard>,
    disks: BTreeMap<String, DiskWriter>,
}

impl WriterLocks {
    pub(crate) fn new(system: File, selection: crate::authority_selection::SelectionGuard) -> Self {
        Self {
            system: Arc::new(system),
            selection: Arc::new(selection),
            disks: BTreeMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, disk: String, writer: DiskWriter) {
        self.disks.insert(disk, writer);
    }

    pub(crate) fn extend(&mut self, writers: Vec<(String, DiskWriter)>) {
        self.disks.extend(writers);
    }

    pub(crate) fn len(&self) -> usize {
        self.disks.len() + 1
    }

    pub(crate) fn system_identity(&self) -> Result<FileIdentity> {
        Ok(FileIdentity::from_fd(&*self.system)?)
    }

    /// Management retains the pool writer, not any disk writer or I/O grant.
    pub(crate) fn system_only(&self) -> Self {
        Self {
            system: Arc::clone(&self.system),
            selection: Arc::clone(&self.selection),
            disks: BTreeMap::new(),
        }
    }

    pub(crate) fn retain(&self) -> Result<Self> {
        let _activity = self.activity()?;
        Ok(self.clone())
    }

    /// Target-local control writes retain exactly their admitted disk, without
    /// acquiring ordinary I/O on unrelated members. The caller must keep the
    /// target activity lease alive across the entire write and error cleanup.
    pub(crate) fn retain_disk(&self, disk: &crate::DiskConfig, root: &DiskRoot) -> Result<Self> {
        let writer = self.disks.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "member writer lock is not held",
            )
        })?;
        if !writer.binding.same_domain(root.io_binding()?) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "target root and retained writer have different admission domains",
            ));
        }
        self.verify_disk(disk, root)?;
        Ok(Self {
            system: Arc::clone(&self.system),
            selection: Arc::clone(&self.selection),
            disks: BTreeMap::from([(disk.disk_id.clone(), writer.clone())]),
        })
    }

    /// A detached synchronous worker keeps every real tail fenced, including
    /// errors and stack-owned cleanup. Idle preparations hold no ordinary count.
    pub(crate) fn activity(&self) -> Result<Vec<DiskIoLease>> {
        self.disks.values().map(DiskWriter::admit).collect()
    }

    /// Bind the retained OS writer lock to the actual named member lock. The
    /// caller keeps this owner and its I/O activity alive through publication.
    pub(crate) fn verify_disk(&self, disk: &crate::DiskConfig, root: &DiskRoot) -> Result<()> {
        let writer = self.disks.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "member writer lock is not held",
            )
        })?;
        let _activity = root.io_binding()?.admit()?;
        writer.held()?;
        let named = root.open_regular(&root.path().join(".writer.lock"))?;
        if FileIdentity::from_fd(&named)? != writer.fence.writer_identity()?
            || named.metadata()?.len() != 0
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "member writer lock was replaced or contains unrelated bytes",
            ));
        }
        Ok(())
    }

    /// Remove only the writer whose physical admission domain has completed
    /// every descriptor and lock tail. Other disk locks and the system lock are
    /// retained unchanged; this operation never opens the retired root.
    pub(crate) fn excluding_closed(&self, closed: &ClosedDiskIo) -> Result<Self> {
        let disk_id = &closed.identity().disk_id;
        let writer = self.disks.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "closed disk is not among the held writer locks",
            )
        })?;
        closed.verify_binding(&writer.binding)?;
        Ok(Self {
            system: Arc::clone(&self.system),
            selection: Arc::clone(&self.selection),
            disks: self
                .disks
                .iter()
                .filter(|(id, _)| *id != disk_id)
                .map(|(id, writer)| (id.clone(), writer.clone()))
                .collect(),
        })
    }

    pub(crate) fn require_all<'a>(
        &self,
        disks: impl IntoIterator<Item = &'a crate::DiskConfig>,
    ) -> Result<()> {
        for disk in disks {
            self.disks
                .get(&disk.disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "recovery requires every original disk writer lock",
                    )
                })?
                .held()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/fencing_resources_tests.rs"]
mod tests;
