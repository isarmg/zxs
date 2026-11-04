//! Read-only observations whose real descriptors and blocking tails stay admitted.
use super::{DiskIoLease, DiskIoResource};
use crate::{
    DiskConfig, Error, ErrorCode, Result, ValidatedConfig,
    disk::{
        DiskSeal, MAX_METADATA, decode_seal, existing_private_dir, fs_error, identity_at,
        read_file_bytes_observed, validate_stored_seal,
    },
};
use platform_io::fs_safety::{
    RelativePath,
    linux::{MountPolicy, OpenAt2Root},
};
use std::fs::Metadata;

/// A verified registered directory with revocable, read-only ownership.
/// No file, descriptor or writable capability can be extracted from this value.
#[derive(Debug)]
pub struct DiskObservationDirectory {
    directory: DiskIoResource<OpenAt2Root>,
}

/// Scoped access to the owned directory. It cannot outlive its admitted resource
/// guard, including while a dedicated observer completes subprocess cleanup.
pub struct DiskObservationAccess<'a> {
    directory: &'a OpenAt2Root,
    owner: &'a DiskIoResource<OpenAt2Root>,
}

/// Informational mount facts from the verified descriptor, not storage identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskObservationMount {
    pub mount_id: u64,
    pub device_major: u32,
    pub device_minor: u32,
}

impl DiskObservationDirectory {
    /// The caller admits before any named-directory lookup. Opening and seal
    /// reads transfer into the same revocable owner before it can escape.
    pub(crate) fn open(
        lease: DiskIoLease,
        config: &ValidatedConfig,
        disk: &DiskConfig,
        expected: &DiskSeal,
    ) -> Result<Self> {
        lease.check_running()?;
        let directory = OpenAt2Root::from_private_directory(
            existing_private_dir(&disk.root)?,
            MountPolicy::SameMount,
        )
        .map_err(fs_error)?;
        let observed = Self {
            directory: lease.hold(directory),
        };
        observed.observe(|access| access.verify_seal(config, disk, expected))?;
        Ok(observed)
    }

    /// Run one synchronous observation with the actual resource held until its
    /// last syscall and cleanup return. The callback receives no raw descriptor.
    ///
    /// # Errors
    /// Returns admission failure before invoking the callback, or cancellation
    /// after its real tail. The callback's concrete failures take precedence.
    pub fn observe<T>(
        &self,
        observe: impl FnOnce(&DiskObservationAccess<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.directory.access_checked()?;
        let access = DiskObservationAccess {
            directory: &guard.value().resource,
            owner: &self.directory,
        };
        let result = observe(&access)?;
        self.directory.check_running()?;
        Ok(result)
    }

    /// Duplicate only managed ownership, never a raw descriptor. An idle clone
    /// is revoked at quiesce even if its recipient has not started or polled it.
    ///
    /// # Errors
    /// Preserves descriptor-duplication errors and refuses stopped bindings.
    pub fn try_clone(&self) -> Result<Self> {
        let (directory, lease) = {
            let mut guard = self.directory.access_checked()?;
            let value = guard.value();
            let directory = OpenAt2Root::from_directory(
                value.resource.directory().try_clone()?,
                MountPolicy::SameMount,
            )
            .map_err(fs_error)?;
            (directory, value.lease.retain())
        };
        let directory = lease.hold(directory);
        directory.check_running()?;
        Ok(Self { directory })
    }

    /// Read metadata from the admitted descriptor, without reopening its name.
    ///
    /// # Errors
    /// Preserves filesystem failures and refuses revoked ownership.
    pub fn metadata(&self) -> Result<Metadata> {
        self.observe(|access| access.metadata())
    }

    pub(crate) fn capacity(&self) -> Result<(u64, u64, u64)> {
        self.observe(|access| {
            access.check_running()?;
            let stat =
                rustix::fs::fstatvfs(access.directory.directory()).map_err(std::io::Error::from)?;
            let total = stat.f_blocks.saturating_mul(stat.f_frsize);
            let available = stat.f_bavail.saturating_mul(stat.f_frsize);
            let used = stat
                .f_blocks
                .saturating_sub(stat.f_bfree)
                .saturating_mul(stat.f_frsize);
            Ok((total, available, used))
        })
    }
}

impl DiskObservationAccess<'_> {
    /// Check cancellation between bounded observation steps. Existing cleanup
    /// remains owned by this access until the enclosing callback returns.
    ///
    /// # Errors
    /// Returns DiskUnavailable after quiesce, or the admission registry error.
    pub fn check_running(&self) -> Result<()> {
        self.owner.check_running()
    }

    /// Retain an observation subprocess whose cleanup did not establish exit.
    /// This consumes the actual child, faults its disk admission domain and
    /// keeps this scoped resource guard and worker alive. New observations are
    /// refused; recovery requires establishing that the original process ended.
    ///
    /// This is only for an unsuccessful wait after a real observation child was
    /// started. It never returns, retries the wait or claims completed cleanup.
    pub fn retain_unresolved_process_cleanup(
        &self,
        process: std::process::Child,
        source: std::io::Error,
    ) -> ! {
        tracing::error!(process_id = process.id(), error = ?source,
            "observation subprocess exit is unconfirmed; retaining disk I/O ownership");
        match self.owner.binding.gate.state.lock() {
            Ok(mut state) => state.failed = true,
            Err(_) => {
                tracing::error!("unresolved observation cleanup found poisoned disk admission")
            }
        }
        // No further syscall, retry or destructor can retire this uncertain
        // child. The one dedicated worker retains it and the access guard even
        // through an outer panic; spurious unparks cannot release either owner.
        let _process = process;
        loop {
            std::thread::park();
        }
    }

    /// Read the held directory's metadata without exposing its file or fd.
    ///
    /// # Errors
    /// Preserves filesystem errors and refuses canceled operations.
    pub fn metadata(&self) -> Result<Metadata> {
        self.check_running()?;
        self.directory.directory().metadata().map_err(Into::into)
    }

    /// Resolve Linux mount facts on this same held directory object.
    ///
    /// # Errors
    /// Preserves statx failures; a missing kernel mount ID is not guessed.
    pub fn mount(&self) -> Result<DiskObservationMount> {
        self.check_running()?;
        let stat = rustix::fs::statx(
            self.directory.directory(),
            "",
            rustix::fs::AtFlags::EMPTY_PATH | rustix::fs::AtFlags::NO_AUTOMOUNT,
            rustix::fs::StatxFlags::MNT_ID,
        )
        .map_err(std::io::Error::from)?;
        if stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() == 0 {
            return Err(Error::new(
                ErrorCode::IoError,
                "kernel did not return a mount ID for the observed directory",
            ));
        }
        Ok(DiskObservationMount {
            mount_id: stat.stx_mnt_id,
            device_major: stat.stx_dev_major,
            device_minor: stat.stx_dev_minor,
        })
    }

    fn verify_seal(
        &self,
        config: &ValidatedConfig,
        disk: &DiskConfig,
        expected: &DiskSeal,
    ) -> Result<()> {
        self.check_running()?;
        let identity = identity_at(config, disk, self.directory.directory())?;
        validate_stored_seal(expected, &identity)?;
        let seal_path = RelativePath::new("disk.json").map_err(fs_error)?;
        self.check_running()?;
        let seal = self
            .directory
            .open_file(&seal_path, false)
            .map_err(fs_error)?;
        let stored = decode_seal(&read_file_bytes_observed(seal, MAX_METADATA, &mut |_| {
            self.check_running()
        })?)?;
        validate_stored_seal(&stored, &identity)
    }
}
