//! Immutable, bounded terminal authority selected by protected system state.
//! System writer ownership serializes both this path and ordinary Engine work.
use super::*;
#[path = "stop_plan.rs"]
mod stop_plan;
use crate::engine::membership_transition::pending::activation::terminal_resume::completed_selector;
use crate::{
    decode_strict, disk::MAX_METADATA, engine::membership_transition::terminal::AUTHORITY_KIND,
    journal::AuthenticatedMetadataObject,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use std::io::Read;

const DIRECTORY: &str = "terminal-management";
const HEAD: &str = "head.json";
const AUTHORITY: &str = "authority.json";
const COMPLETE: &str = "complete.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "protected-terminal-authority")]
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Head {
    format: Format,
    pool_id: String,
    operation_id: OperationId,
    authority_hash: String,
}

pub(super) struct TerminalStore {
    system: DiskRoot,
    directory: DiskRoot,
    operation: DiskRoot,
    head: Head,
}

impl TerminalStore {
    pub(super) fn prepare(engine: &Engine, authority: &TerminalAuthority) -> Result<Self> {
        let system = DiskRoot::open(&engine.root)?;
        system.private_directory(system.path())?;
        let named = system.open_regular(&system.path().join("writer.lock"))?;
        if FileIdentity::from_fd(&named)? != engine._locks.system_identity()? {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "terminal system writer identity changed",
            ));
        }
        let directory_path = system.path().join(DIRECTORY);
        system.private_dir(&directory_path)?;
        let directory = system.private_child(&directory_path)?;
        let head = Head {
            format: Format::Terminal,
            pool_id: authority.original().pool_id.clone(),
            operation_id: authority.operation_id(),
            authority_hash: authority.hash()?,
        };
        let previous = optional::<Head>(&directory, &directory.path().join(HEAD))?
            .filter(|previous| previous != &head);
        if previous.is_some() {
            let completed = completed_selector(&engine.root)?.ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "later terminal handoff requires the completed exact previous resumption",
                )
            })?;
            completed.verify_handoff(engine, authority)?;
        }
        let operation_path = directory.path().join(authority.operation_id().to_string());
        directory.private_dir(&operation_path)?;
        let operation = directory.private_child(&operation_path)?;
        let store = Self {
            system,
            directory,
            operation,
            head,
        };
        store.require_no_resume()?;
        match previous {
            Some(previous) => store.replace_consumed_head(authority, &previous)?,
            None => store.synchronize_preparation(authority)?,
        }
        Ok(store)
    }

    fn replace_consumed_head(&self, authority: &TerminalAuthority, previous: &Head) -> Result<()> {
        // The complete new authority/archive is durable and authenticated before
        // changing discovery. No stop or target I/O closure has happened yet.
        self.operation
            .write_json(&self.operation.path().join(AUTHORITY), authority, false)?;
        self.load_authority()?;
        let current: Head = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(HEAD), MAX_METADATA)?,
        )?;
        if current != *previous {
            return Err(Error::new(
                ErrorCode::LogFork,
                "consumed terminal selector changed before its successor handoff",
            ));
        }
        // Only this verified successor may replace a selector. Ordinary retries
        // keep using immutable writes and re-establish the same barriers.
        self.directory
            .write_json(&self.directory.path().join(HEAD), &self.head, true)
    }

    pub(super) fn open(root: &Path) -> Result<(Self, WriterLocks)> {
        let system = DiskRoot::open(root)?;
        system.private_directory(system.path())?;
        // Use the existing system writer, never create a replacement lock for a
        // missing/damaged protected directory. No data root is consulted.
        let lock = system.open_regular(&system.path().join("writer.lock"))?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&lock)
            .map_err(crate::disk::fs_error)?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |error| {
                Error::from(std::io::Error::from(error)).context(
                    ErrorCode::WriterAlreadyActive,
                    "another process owns this pool",
                )
            },
        )?;
        let locks = WriterLocks::new(lock);
        Ok((Self::read_system(system)?, locks))
    }

    fn read_system(system: DiskRoot) -> Result<Self> {
        system.private_directory(system.path())?;
        let directory = system.private_child(&system.path().join(DIRECTORY))?;
        let head: Head =
            decode_strict(&directory.read_bytes(&directory.path().join(HEAD), MAX_METADATA)?)?;
        storage_contract::validate_id(&head.pool_id, "pool")?;
        head.authority_hash
            .parse::<storage_contract::ContentHash>()
            .map_err(|error| {
                Error::from(std::io::Error::other(error)).context(
                    ErrorCode::ObjectCorrupt,
                    "invalid terminal authority digest",
                )
            })?;
        let operation =
            directory.private_child(&directory.path().join(head.operation_id.to_string()))?;
        Ok(Self {
            system,
            directory,
            operation,
            head,
        })
    }

    pub(super) fn reopen_held(&self) -> Result<Self> {
        Ok(Self {
            system: self.system.private_child(self.system.path())?,
            directory: self.directory.private_child(self.directory.path())?,
            operation: self.operation.private_child(self.operation.path())?,
            head: self.head.clone(),
        })
    }
    pub(super) fn system(&self) -> &DiskRoot {
        &self.system
    }
    pub(super) fn authority_hash(&self) -> &str {
        &self.head.authority_hash
    }
    pub(super) fn verify_writer(&self, locks: &WriterLocks) -> Result<()> {
        let named = self
            .system
            .open_regular(&self.system.path().join("writer.lock"))?;
        if FileIdentity::from_fd(&named)? != locks.system_identity()? {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "terminal completion does not own the protected system writer",
            ));
        }
        let head: Head = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(HEAD), MAX_METADATA)?,
        )?;
        if head != self.head {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal system authority changed before completion",
            ));
        }
        self.require_no_resume()
    }

    pub(super) fn require_no_resume(&self) -> Result<()> {
        if self
            .operation
            .open_optional_read_regular(&self.operation.path().join("resume.json"))?
            .is_some()
            || self
                .operation
                .open_optional_read_regular(&self.operation.path().join("resume-complete.json"))?
                .is_some()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal authority has a metadata resumption; continue that exact authenticated request",
            ));
        }
        Ok(())
    }

    pub(super) fn load_authority(&self) -> Result<VerifiedTerminalAuthority> {
        let object = AuthenticatedMetadataObject::authenticate(
            AUTHORITY_KIND,
            &self.head.authority_hash,
            decode_strict(
                &self
                    .operation
                    .read_bytes(&self.operation.path().join(AUTHORITY), MAX_METADATA)?,
            )?,
        )?;
        let authority: TerminalAuthority = json_codec::from_value(object.into_value())?;
        if authority.operation_id() != self.head.operation_id
            || authority.original().pool_id != self.head.pool_id
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal authority differs from protected system head",
            ));
        }
        authority.verify_archive(&self.system)
    }

    pub(super) fn synchronize_preparation(&self, authority: &TerminalAuthority) -> Result<()> {
        if authority.hash()? != self.head.authority_hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal preparation hash differs",
            ));
        }
        // Identical existing immutable writes still fsync file and parent. An
        // uncertain earlier rename never substitutes for today's durability.
        self.operation
            .write_json(&self.operation.path().join(AUTHORITY), authority, false)?;
        self.directory
            .write_json(&self.directory.path().join(HEAD), &self.head, false)?;
        Ok(())
    }
    pub(super) fn publish(&self, authority: &TerminalAuthority) -> Result<()> {
        self.require_no_resume()?;
        if authority.hash()? != self.head.authority_hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal publication differs from prepared authority",
            ));
        }
        self.operation
            .write_json(&self.operation.path().join(COMPLETE), &self.head, false)
    }
    pub(super) fn verify_publication(&self, authority: &TerminalAuthority) -> Result<()> {
        let bytes = self
            .operation
            .read_bytes(&self.operation.path().join(COMPLETE), MAX_METADATA)
            .map_err(|source| {
                source.context(
                    ErrorCode::RecoveryRequired,
                    "terminal authority has no readable completion record",
                )
            })?;
        let complete: Head = decode_strict(&bytes).map_err(|source| {
            Error::from(source).context(
                ErrorCode::RecoveryRequired,
                "terminal authority completion is malformed",
            )
        })?;
        if complete != self.head || authority.hash()? != complete.authority_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal completion differs from its exact authority",
            ));
        }
        Ok(())
    }
}

fn optional<T: for<'de> Deserialize<'de>>(root: &DiskRoot, path: &Path) -> Result<Option<T>> {
    let Some(mut file) = root.open_optional_read_regular(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_METADATA as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_METADATA {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "terminal control record exceeds metadata limit",
        ));
    }
    Ok(Some(decode_strict(&bytes)?))
}

pub(in crate::engine) fn require_active_startup(root: &Path) -> Result<()> {
    if requires_management(root)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "protected terminal authority requires TerminalManagement or exact metadata resumption, not active Config startup",
        ));
    }
    Ok(())
}

pub(super) fn requires_management(root: &Path) -> Result<bool> {
    // An unreadable selector cannot authorize active Config startup. Preserve
    // the actual parse, I/O or authentication failure for recovery diagnostics.
    read_management_selection(root).map_err(|source| {
        source.context(
            ErrorCode::RecoveryRequired,
            "protected terminal selection cannot authorize active startup",
        )
    })
}

fn read_management_selection(root: &Path) -> Result<bool> {
    let system = DiskRoot::open(root)?;
    let directory = system.path().join(DIRECTORY);
    if system.optional_metadata(&directory)?.is_none() {
        return Ok(false);
    }
    // A pre-head crash has not written any stop and can recover the old fence.
    // A deleted head after a terminal stop still cannot widen discovery: the
    // independently held bootstrap ledger itself rejects all terminal probes.
    let Some(file) = system.open_optional_read_regular(&directory.join(HEAD))? else {
        return Ok(false);
    };
    let head: Head = crate::decode_strict(&crate::disk::read_file_bytes(file, MAX_METADATA)?)?;
    let completion = directory
        .join(head.operation_id.to_string())
        .join("resume-complete.json");
    if system.open_optional_read_regular(&completion)?.is_none() {
        return Ok(true);
    }
    crate::engine::membership_transition::pending::activation::terminal_resume::completed_selector(
        root,
    )?
    .ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "terminal resumption lost its authenticated selector",
        )
    })?;
    Ok(false)
}

impl TerminalManagement {
    /// SYSTEM-only routing before acquiring the terminal writer. A valid
    /// preparation without a resume can still need its exact stop/publication
    /// completed. Existing publication and continuation records never become
    /// permission to treat damaged or incomplete evidence as no request.
    pub fn metadata_resume_requested(root: &Path) -> Result<bool> {
        let store = TerminalStore::read_system(DiskRoot::open(root)?)?;
        let verified = store.load_authority()?;
        let requested = store
            .operation
            .open_optional_read_regular(&store.operation.path().join("resume.json"))?
            .is_some();
        for name in [
            "resume-generation.json",
            "resume-bootstrap.json",
            "resume-complete.json",
        ] {
            if !requested
                && store
                    .operation
                    .open_optional_read_regular(&store.operation.path().join(name))?
                    .is_some()
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "terminal resume continuation has no protected request",
                ));
            }
        }
        if requested
            || store
                .operation
                .open_optional_read_regular(&store.operation.path().join(COMPLETE))?
                .is_some()
        {
            store.verify_publication(&verified.authority)?;
        }
        Ok(requested)
    }

    /// Pure SYSTEM observation for startup/status selection. It grants no writer
    /// ownership, does not synchronize a stop and cannot return a management
    /// handle. Mutable consumers must separately acquire their existing locks.
    pub(in crate::engine) fn verified_completed_source(
        root: &Path,
    ) -> Result<(DiskRoot, VerifiedTerminalAuthority)> {
        let store = TerminalStore::read_system(DiskRoot::open(root)?)?;
        let verified = store.load_authority()?;
        store.verify_publication(&verified.authority)?;
        let current: Head = decode_strict(
            &store
                .directory
                .read_bytes(&store.directory.path().join(HEAD), MAX_METADATA)?,
        )?;
        if current != store.head {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal selector changed during completed-source verification",
            ));
        }
        Ok((store.system, verified))
    }
}
