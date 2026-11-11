//! Held-resource publication that preserves every displaced object. No unlink,
//! rollback, or path-reopened entity is part of this mechanism.
use super::*;
use data_serialization::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom, Write};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FileStamp {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) bytes: u64,
    pub(crate) digest: String,
}

pub(crate) struct HeldRegular {
    // Prevent inode reuse while a prepared old-file expectation is live. Managed
    // files remain revocable; no raw descriptor escapes the disk admission.
    file: ReadFile,
    stamp: FileStamp,
    bytes: Vec<u8>,
}
impl HeldRegular {
    pub(crate) fn capture(root: &DiskRoot, leaf: &str, maximum: usize) -> Result<Option<Self>> {
        Self::capture_entry(root, &EntryName::new(leaf).map_err(fs_error)?, maximum)
    }
    pub(crate) fn capture_entry(
        root: &DiskRoot,
        name: &EntryName,
        maximum: usize,
    ) -> Result<Option<Self>> {
        let Some(file) = root.open_optional_read_entry(name)? else {
            return Ok(None);
        };
        let metadata = file.metadata()?;
        let mut reader = file.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        let bytes = read_file_bytes(reader, maximum)?;
        if !metadata.is_file() || metadata.nlink() != 1 || file.metadata()?.nlink() != 1 {
            return Err(collision("database is not a single-link regular file"));
        }
        let stamp = FileStamp {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        Ok(Some(Self { file, stamp, bytes }))
    }
    pub(crate) fn stamp(&self) -> &FileStamp {
        &self.stamp
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn release_bytes(&mut self) {
        self.bytes = Vec::new();
    }
    pub(crate) fn check_running(&self) -> Result<()> {
        self.file.check_running()
    }
}

pub(crate) struct InstallExchange<'a> {
    pub(crate) staged: &'a FileStamp,
    pub(crate) old: Option<&'a FileStamp>,
    pub(crate) maximum: usize,
}

fn collision(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

impl DiskRoot {
    /// Publish a complete intent only after a separately named, synced pending
    /// file exists. Neither failed writes nor interrupted publication unlink it.
    pub(crate) fn write_install_intent(
        &self,
        bytes: &[u8],
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        self.create_install_file("intent.pending", bytes)?;
        let operation = self.operation()?;
        let prepared = inspect(&operation, "intent.pending", bytes.len())?
            .ok_or_else(|| collision("installation intent stage disappeared"))?;
        if prepared.0.bytes != bytes.len() as u64
            || prepared.0.digest != blake3::hash(bytes).to_hex().to_string()
        {
            return Err(collision("installation intent stage changed"));
        }
        observe("after_database_intent_prepare")?;
        operation.check_running()?;
        let renamed = rustix::fs::renameat_with(
            operation.held().directory(),
            "intent.pending",
            operation.held().directory(),
            "intent.json",
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from);
        preserve_error(
            renamed.map_err(Error::from),
            operation.held().directory().sync_all(),
        )?;
        let installed = inspect(&operation, "intent.json", bytes.len())?
            .ok_or_else(|| collision("published installation intent disappeared"))?;
        if installed.0 != prepared.0 {
            return Err(collision("installation intent changed during publication"));
        }
        operation.check_running()
    }

    /// Create directly in the separately owned private installation directory.
    /// Partial or failed writes remain there, never in the managed namespace.
    pub(crate) fn create_install_file(&self, leaf: &str, bytes: &[u8]) -> Result<()> {
        let operation = self.operation()?;
        let name = EntryName::new(leaf).map_err(fs_error)?;
        let mut file = File::from(
            rustix::fs::openat2(
                operation.held().directory(),
                name.as_os_str(),
                rustix::fs::OFlags::WRONLY
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                rustix::fs::ResolveFlags::BENEATH
                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                    | rustix::fs::ResolveFlags::NO_XDEV,
            )
            .map_err(std::io::Error::from)?,
        );
        SingleLinkRequirement::check(&file).map_err(fs_error)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        operation.held().directory().sync_all()?;
        operation.check_running()
    }

    /// Both ends are already held entities. EXCHANGE is deliberately not called
    /// a CAS: the displaced entry is retained and checked after the exchange.
    pub(crate) fn publish_install_preserving(
        &self,
        stage: &DiskRoot,
        target: &EntryName,
        expected: InstallExchange<'_>,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        let source = stage.operation()?;
        let destination = self.operation()?;
        let same_binding = match (self.binding(), stage.binding()) {
            (Some(left), Some(right)) => left.same_domain(right),
            (None, None) => true,
            _ => false,
        };
        if !same_binding || source.held().identity().device != destination.held().identity().device
        {
            return Err(collision("database installation crosses devices"));
        }
        let new = inspect(&source, "payload", expected.maximum)?
            .ok_or_else(|| collision("database stage is absent"))?;
        if &new.0 != expected.staged {
            return Err(collision(
                "database stage identity or complete bytes changed",
            ));
        }
        let old = inspect_entry(&destination, target, expected.maximum)?;
        if old.as_ref().map(|value| &value.0) != expected.old {
            return Err(collision("database target changed after preparation"));
        }
        observe("before_database_publish")?;
        source.check_running()?;
        destination.check_running()?;
        let flags = if expected.old.is_some() {
            rustix::fs::RenameFlags::EXCHANGE
        } else {
            rustix::fs::RenameFlags::NOREPLACE
        };
        // Even an errno leaves both entries and the persistent intent intact.
        // The caller reclassifies actual resources on recovery, never retries a
        // potentially executed exchange blindly or swaps an unknown entry back.
        let renamed = rustix::fs::renameat_with(
            source.held().directory(),
            "payload",
            destination.held().directory(),
            target.as_os_str(),
            flags,
        )
        .map_err(std::io::Error::from);
        let destination_sync = destination.held().directory().sync_all();
        let source_sync = source.held().directory().sync_all();
        let synced = preserve_error(renamed.map_err(Error::from), destination_sync);
        preserve_error(synced, source_sync)?;
        observe("after_database_publish")?;
        let installed = inspect_entry(&destination, target, expected.maximum)?;
        let displaced = inspect(&source, "payload", expected.maximum)?;
        if installed.as_ref().map(|value| &value.0) != Some(expected.staged)
            || displaced.as_ref().map(|value| &value.0) != expected.old
        {
            return Err(collision(
                "database publication displaced an unexpected object; all objects retained",
            ));
        }
        source.check_running()?;
        destination.check_running()
    }
}

fn preserve_error(result: Result<()>, tail: std::io::Result<()>) -> Result<()> {
    match (result, tail) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(operation), Err(cleanup)) => Err(Error::Cleanup {
            operation: Box::new(operation),
            cleanup,
        }),
    }
}

fn inspect(
    operation: &RootOperation<'_>,
    leaf: &str,
    maximum: usize,
) -> Result<Option<(FileStamp, File)>> {
    inspect_entry(operation, &EntryName::new(leaf).map_err(fs_error)?, maximum)
}

fn inspect_entry(
    operation: &RootOperation<'_>,
    name: &EntryName,
    maximum: usize,
) -> Result<Option<(FileStamp, File)>> {
    let Some(file) = operation.open_optional_relative_regular(&name.as_relative())? else {
        return Ok(None);
    };
    let identity = SingleLinkRequirement::check(&file).map_err(fs_error)?;
    let bytes = read_file_bytes_observed(file.try_clone()?, maximum, &mut |_| {
        operation.check_running()
    })?;
    SingleLinkRequirement::check(&file).map_err(fs_error)?;
    Ok(Some((
        FileStamp {
            device: identity.device,
            inode: identity.inode,
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        },
        file,
    )))
}
