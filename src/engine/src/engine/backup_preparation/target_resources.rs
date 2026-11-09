//! Retained ownership of the exact selected target, separate from runtime I/O.
//! A writer-file fence is not a DiskWriter or permission to mutate user data.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;

pub(super) struct Target {
    writer: Arc<Mutex<Option<Arc<FenceOwner>>>>,
    directory: Arc<AdvisoryLock>,
    pub(super) root: DiskRoot,
}

impl Target {
    pub(super) fn new(root: DiskRoot, lock: AdvisoryLock) -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
            directory: Arc::new(lock),
            root,
        }
    }

    /// Retain the same held root and open-file-description locks. Do not reopen
    /// or briefly release the old reservation when a task/query borrows it.
    pub(super) fn retain(&self) -> Result<Self> {
        Ok(Self {
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
                *held = Some(FenceOwner::new(named, None));
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
}
