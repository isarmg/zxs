//! Retained ownership of the exact selected target, separate from runtime I/O.
//! A file fence alone is not I/O admission. Only the full native task can attach
//! a target-only DiskWriter; no ordinary Engine or source authority is changed.
use super::*;
use crate::engine::membership_transition::pending::activation::BackupWriterIoConfig;
use platform_io::fs_safety::linux::FileIdentity;

struct WriterIo {
    plan_hash: String,
    handle: Arc<DiskHandle>,
    writer: DiskWriter,
}

pub(super) struct Target {
    io: Arc<Mutex<Option<WriterIo>>>,
    writer: Arc<Mutex<Option<Arc<FenceOwner>>>>,
    // Taken exactly once into FenceOwner. Idle Target aliases must not keep an
    // untracked strong lock after the gate has certified final fence closure.
    directory: Arc<Mutex<Option<Arc<AdvisoryLock>>>>,
    pub(super) root: DiskRoot,
}

impl Target {
    pub(super) fn new(root: DiskRoot, lock: AdvisoryLock) -> Self {
        Self {
            io: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(None)),
            directory: Arc::new(Mutex::new(Some(Arc::new(lock)))),
            root,
        }
    }

    /// Retain the same held root and open-file-description locks. Do not reopen
    /// or briefly release the old reservation when a task/query borrows it.
    pub(super) fn retain(&self) -> Result<Self> {
        Ok(Self {
            io: Arc::clone(&self.io),
            writer: Arc::clone(&self.writer),
            directory: Arc::clone(&self.directory),
            root: self.root.retained(self.root.path().to_owned())?,
        })
    }

    fn named_writer(&self) -> Result<File> {
        let file = self
            .root
            .open_regular(&self.root.path().join(".writer.lock"))?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&file).map_err(fs_error)?;
        if file.metadata()?.len() != 0 {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "Backup target writer file contains unrelated bytes; retained unchanged",
            ));
        }
        Ok(file)
    }

    /// The caller has separately authorized this name. Never create/replace it
    /// here, including cold reacquisition and inspection after task completion.
    pub(super) fn acquire_writer(&self) -> Result<PhysicalBinding> {
        let mut held = self.writer.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer ownership is poisoned",
            )
        })?;
        let named = self.named_writer()?;
        let identity = FileIdentity::from_fd(&named)?;
        match held.as_ref() {
            Some(owner) if owner.writer_identity()? != identity => {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "Backup target writer file was replaced while its original owner is held",
                ));
            }
            Some(_) => (),
            None => {
                rustix::fs::flock(&named, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                    .map_err(|error| {
                        Error::from(std::io::Error::from(error)).context(
                            ErrorCode::WriterAlreadyActive,
                            "another writer owns the Backup target",
                        )
                    })?;
                // Preserve ownership before any later fallible synchronization
                // or receipt write. The surrounding Target retains its directory.
                let reservation = self
                    .directory
                    .lock()
                    .map_err(|_| {
                        Error::new(
                            ErrorCode::RecoveryRequired,
                            "Backup target directory ownership is poisoned",
                        )
                    })?
                    .take()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::RecoveryRequired,
                            "Backup target directory reservation was already transferred",
                        )
                    })?;
                *held = Some(FenceOwner::with_advisory_reservation(named, reservation));
            }
        }
        Ok(PhysicalBinding {
            device: identity.device,
            inode: identity.inode,
        })
    }

    pub(super) fn require_writer(&self, expected: &PhysicalBinding) -> Result<()> {
        let held = self.writer.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer ownership is poisoned",
            )
        })?;
        let owner = held.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target has no retained writer owner",
            )
        })?;
        let actual = owner.writer_identity()?;
        if actual.device != expected.device
            || actual.inode != expected.inode
            || FileIdentity::from_fd(&self.named_writer()?)? != actual
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup target writer identity differs from its original durable binding",
            ));
        }
        Ok(())
    }

    pub(super) fn recheck_writer(&self) -> Result<()> {
        let held = self.writer.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer ownership is poisoned",
            )
        })?;
        if let Some(owner) = held.as_ref()
            && owner.writer_identity()? != FileIdentity::from_fd(&self.named_writer()?)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup target writer name no longer identifies the retained owner",
            ));
        }
        Ok(())
    }

    /// Bind the same held directory to the ordinary gate and adopt the SAME
    /// acquired OS writer. This cannot reparse a Config or reacquire a lock.
    pub(super) fn bind_writer_io(
        &self,
        admitted: &BackupWriterIoConfig,
        disk_id: &str,
        writer: &PhysicalBinding,
        plan_hash: &str,
    ) -> Result<String> {
        self.require_writer(writer)?;
        let config = admitted.config();
        let disk = config
            .io_disks()
            .find(|disk| disk.disk_id == disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskNotFound,
                    "Backup writer is not an admitted selected target",
                )
            })?;
        if disk.root != self.root.path() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup writer I/O cannot rebind its exact original root address",
            ));
        }
        let epoch = config
            .io_admission_epoch(disk_id)?
            .expect("invariant: native selected target epoch");
        let seal = crate::disk::identity_at(config, disk, &self.root.directory(self.root.path())?)?;
        let mut io = self.io.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer I/O ownership is poisoned",
            )
        })?;
        if let Some(current) = io.as_ref() {
            if current.plan_hash != plan_hash {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "Backup target already retains another writer I/O plan",
                ));
            }
            validate_stored_seal(&current.handle.seal, &seal)?;
        } else {
            let owner = self
                .writer
                .lock()
                .map_err(|_| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "Backup target writer ownership is poisoned",
                    )
                })?
                .as_ref()
                .cloned()
                .expect("invariant: verified target writer owner remains retained");
            let handle = DiskHandle::new(self.root.retained(disk.root.clone())?, seal, config)?;
            let writer = DiskWriter::adopt(&handle.root, owner)?;
            // Retain before any subsequent write/readback can fail. The root's
            // permanent shared binding also governs aliases created earlier.
            *io = Some(WriterIo {
                plan_hash: plan_hash.into(),
                handle: Arc::new(handle),
                writer,
            });
        }
        let current = io.as_ref().expect("invariant: writer I/O was retained");
        let _lease = current.handle.root.admit_io()?;
        self.recheck_writer()?;
        Ok(epoch)
    }

    /// Use the real ordinary I/O gate, with ownership pinned through the full
    /// operation and its error/sync tail. No raw FD or active writer is returned.
    pub(super) fn with_writer_io<T>(
        &self,
        plan_hash: &str,
        action: impl FnOnce(&DiskRoot) -> Result<T>,
    ) -> Result<T> {
        self.with_writer_handle(plan_hash, |handle| action(&handle.root))
    }

    /// The seal recipe must come from this retained native handle, not a new
    /// Config parsed from progress or a target-local diagnostic record.
    pub(super) fn writer_seal(&self, plan_hash: &str) -> Result<DiskSeal> {
        self.with_writer_handle(plan_hash, |handle| Ok(handle.seal.clone()))
    }

    /// Hand over the same native gate and writer description after final
    /// admission. Cloning retains ownership; no lock is released or reacquired.
    pub(super) fn serving_resources(
        &self,
        plan_hash: &str,
    ) -> Result<(Arc<DiskHandle>, DiskWriter)> {
        let io = self.io.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer I/O ownership is poisoned",
            )
        })?;
        let held = io.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup serving target lost its retained native writer",
            )
        })?;
        if held.plan_hash != plan_hash {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Backup serving handoff changed the original writer plan",
            ));
        }
        let lease = held.handle.root.admit_io()?;
        self.recheck_writer()?;
        lease.check_running()?;
        Ok((Arc::clone(&held.handle), held.writer.clone()))
    }

    fn with_writer_handle<T>(
        &self,
        plan_hash: &str,
        action: impl FnOnce(&DiskHandle) -> Result<T>,
    ) -> Result<T> {
        let io = self.io.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target writer I/O ownership is poisoned",
            )
        })?;
        let current = io.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup target has no admitted task-local DiskWriter",
            )
        })?;
        if current.plan_hash != plan_hash {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Backup target DiskWriter belongs to another original I/O plan",
            ));
        }
        let lease = current.handle.root.admit_io()?;
        self.recheck_writer()?;
        let result = action(&current.handle)?;
        self.recheck_writer()?;
        lease.check_running()?;
        Ok(result)
    }
}
