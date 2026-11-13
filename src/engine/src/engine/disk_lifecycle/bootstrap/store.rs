//! Held-control-directory persistence with a bounded immutable stop ledger.
mod root_extension;
pub(in crate::engine) use root_extension::ChainRootPlanInfo;
pub(crate) use root_extension::PreparedRootExtension;

use super::*;
use crate::persist::layout;
#[path = "readmission.rs"]
mod readmission;
#[path = "stop_publication.rs"]
mod stop_publication;
use platform_io::fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root, open_private_regular},
};
pub(in crate::engine) use readmission::PreparedReadmission;
use std::{io::Read, os::unix::fs::MetadataExt};
pub(in crate::engine) use stop_publication::{PreparedStop, RetirementStopSnapshot};

const HEAD_FILE: &str = "head.json";

/// The supplied descriptor must belong to protected SYSTEM state, not a storage
/// root. No method reopens its diagnostic pathname. The caller establishes the
/// directory/ancestor durability when installing that control directory.
///
/// A dedicated advisory lock serializes cooperating readers/writers for this
/// store's lifetime. Old records are never replaced or deleted. Chain validation
/// streams one bounded record at a time, with no lifetime record-count limit. There is no automatic GC; root expansion requires independently authenticated held resources.
/// A complete malicious/external rollback of this directory is out of scope;
/// crash safety assumes successful file and directory fsync are honored.
pub(crate) struct BootstrapStore {
    directory: PrivateDirectory,
    reader: OpenAt2Root,
    _lock: BootstrapWriter,
    #[cfg(test)]
    fault: Option<WriteFault>,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Injection points bracket the complete record/head durability barriers.
pub(in crate::engine) enum WriteFault {
    BeforeRecord,
    AfterRecord,
    BeforeHead,
    AfterHead,
}
#[cfg(test)]
std::thread_local! {
    static NEXT_WRITE_FAULT: std::cell::Cell<Option<WriteFault>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(in crate::engine) fn inject_next(phase: WriteFault) {
    NEXT_WRITE_FAULT.with(|fault| fault.set(Some(phase)));
}

enum BootstrapWriter {
    Create { _lock: AdvisoryLock },
    Existing { _file: File },
}

impl BootstrapStore {
    /// Acquires/synchronizes only the control lock, never a configured root.
    pub(crate) fn hold(directory: File, path: PathBuf) -> Result<Self> {
        Self::hold_with_existing(directory, path, None)
    }

    pub(crate) fn hold_existing(
        directory: File,
        path: PathBuf,
        writer: storage_contract::working_tree::LocalEntityIdentity,
    ) -> Result<Self> {
        Self::hold_with_existing(directory, path, Some(writer))
    }

    fn hold_with_existing(
        directory: File,
        path: PathBuf,
        existing: Option<storage_contract::working_tree::LocalEntityIdentity>,
    ) -> Result<Self> {
        let reader = directory
            .try_clone()
            .map_err(|e| BootstrapError::io("control descriptor clone", e))?;
        let directory = PrivateDirectory::from_directory(directory, path)
            .map_err(|e| BootstrapError::fs("held control directory", e))?;
        let reader = OpenAt2Root::from_directory(reader, MountPolicy::SameMount)
            .map_err(|e| BootstrapError::fs("held control reader", e))?;
        let lock = if let Some(expected) = existing {
            let file = directory.open_payload(&layout::CONTROL_WRITER_LOCK,
                platform_io::fs_safety::PayloadOpenMode::Existing)
                .map_err(|error| BootstrapError::fs("existing control lock", error))?;
            let observed = platform_io::fs_safety::linux::FileIdentity::from_fd(&file)
                .map_err(|error| BootstrapError::io("existing control identity", error))?;
            if storage_contract::working_tree::LocalEntityIdentity::new(
                observed.device, observed.inode,
            ) != expected || file.metadata()
                .map_err(|error| BootstrapError::io("existing control metadata", error))?.len() != 0
            {
                return Err(BootstrapError::fs("existing control identity",
                    platform_io::fs_safety::Error::IdentityChanged(directory.path().to_owned())));
            }
            file.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => BootstrapError::fs("existing control lock",
                    platform_io::fs_safety::Error::AlreadyLocked(directory.path().to_owned())),
                std::fs::TryLockError::Error(error) => BootstrapError::io("existing control lock", error),
            })?;
            directory.verify_payload(&layout::CONTROL_WRITER_LOCK, &file)
                .map_err(|error| BootstrapError::fs("existing control identity", error))?;
            BootstrapWriter::Existing { _file: file }
        } else {
            BootstrapWriter::Create {
                _lock: AdvisoryLock::acquire(&directory, &layout::CONTROL_WRITER_LOCK.as_relative())
                    .map_err(|error| BootstrapError::fs("control lock", error))?,
            }
        };
        Ok(Self {
            directory,
            reader,
            _lock: lock,
            #[cfg(test)]
            fault: NEXT_WRITE_FAULT.with(std::cell::Cell::take),
        })
    }
    /// Explicit installation only. Never call this from a missing-locator
    /// fallback on old Config. Observations are not registration authority.
    pub(crate) fn initialize(
        &mut self,
        pool_id: &str,
        roots: Vec<RootObservation>,
    ) -> Result<LoadedLocator> {
        self.publish(None, Record::initial(pool_id, roots)?)
    }
    /// Any absent/corrupt head, missing payload, dangling newer record, partial
    /// temporary file, inconsistent chain or unknown entry closes discovery.
    /// This function NEVER selects the newest apparently valid old payload.
    pub(crate) fn reload(&self, pool_id: &str) -> Result<LoadedLocator> {
        let inventory = self.inventory()?;
        let head = self.read_head()?.ok_or(BootstrapError::ManagementOnly(
            "durable locator head is missing",
        ))?;
        if head.pool_id != pool_id {
            return Err(BootstrapError::ManagementOnly(
                "durable locator belongs to another pool",
            ));
        }
        if inventory != expected_records(head.generation) {
            return Err(BootstrapError::ManagementOnly(
                "head does not cover the exact immutable ledger",
            ));
        }
        self.load_chain(&head)
    }
    /// Re-establish the exact current stop record and head barriers using only
    /// held system state. Completion can call this without touching a stopped root.
    pub(crate) fn sync_current(&self, expected: &LoadedLocator) -> Result<()> {
        let current = self.reload(&expected.head.pool_id)?;
        if current.head != expected.head {
            return Err(BootstrapError::ManagementOnly(
                "current stop head changed before synchronization",
            ));
        }
        self.sync_existing(
            &record_name(current.head.generation)?,
            &current.record.encode()?.0,
        )?;
        self.sync_existing(&name(HEAD_FILE)?, &current.head.bytes()?)
    }
    fn load_chain(&self, head: &Head) -> Result<LoadedLocator> {
        let mut previous = None;
        for generation in 1..=head.generation {
            let bytes = self.read_required(&record_name(generation)?, MAX_RECORD_BYTES)?;
            let record = Record::decode(&bytes)?;
            let (_, calculated) = record.encode()?;
            if calculated.generation != generation || calculated.pool_id != head.pool_id {
                return Err(BootstrapError::ManagementOnly(
                    "payload pool or generation differs from its address",
                ));
            }
            self.check_control_separation(&record)?;
            if let Some(older) = &previous {
                record.follows(older)?;
            }
            previous = Some(LoadedLocator {
                head: calculated,
                record,
            });
        }
        let loaded = previous.ok_or(BootstrapError::ManagementOnly("empty locator chain"))?;
        if loaded.head != *head {
            return Err(BootstrapError::ManagementOnly(
                "head hash differs from exact current payload",
            ));
        }
        Ok(loaded)
    }
    fn publish(&mut self, previous: Option<&Head>, record: Record) -> Result<LoadedLocator> {
        self.check_control_separation(&record)?;
        let (bytes, next) = record.encode()?;
        let inventory = self.inventory()?;
        let current = self.read_head()?;
        if current.as_ref() != previous && current.as_ref() != Some(&next) {
            return Err(BootstrapError::ManagementOnly(
                "head changed; refusing replacement or rollback",
            ));
        }
        let minimum = previous.map_or(0, |head| head.generation);
        let before = expected_records(minimum);
        let after = expected_records(next.generation);
        if inventory != before && inventory != after {
            return Err(BootstrapError::ManagementOnly(
                "publication has unrelated, missing or newer records",
            ));
        }
        if let Some(head) = &current {
            let loaded = self.load_chain(head)?;
            if head != &next {
                record.follows(&loaded)?;
            }
        }
        #[cfg(test)]
        self.fail(WriteFault::BeforeRecord)?;
        self.install_record(&record_name(next.generation)?, &bytes)?;
        #[cfg(test)]
        self.fail(WriteFault::AfterRecord)?;
        #[cfg(test)]
        self.fail(WriteFault::BeforeHead)?;
        let head_name = name(HEAD_FILE)?;
        let head_bytes = next.bytes()?;
        if current.as_ref() == Some(&next) {
            // A previous rename may have succeeded before its caller observed
            // a directory-sync failure. Re-establish both real barriers.
            self.sync_existing(&head_name, &head_bytes)?;
        } else {
            self.atomic(&head_name, &head_bytes, current.is_some())?;
        }
        #[cfg(test)]
        self.fail(WriteFault::AfterHead)?;
        Ok(LoadedLocator { head: next, record })
    }
    fn check_control_separation(&self, record: &Record) -> Result<()> {
        let control = canonical_root(self.directory.path())?;
        let identity = self.reader.identity();
        if record.roots.iter().any(|root| {
            overlaps(&root.path, &control)
                || (root.device, root.inode) == (identity.device, identity.inode)
        }) {
            return Err(BootstrapError::ManagementOnly(
                "control directory overlaps or aliases a recorded storage root",
            ));
        }
        Ok(())
    }
    fn inventory(&self) -> Result<Inventory> {
        let entries = self
            .reader
            .entries()
            .map_err(|e| BootstrapError::fs("control inventory", e))?;
        let mut inventory = Inventory {
            count: 0,
            maximum: 0,
        };
        for entry in entries {
            let entry = entry.map_err(|e| BootstrapError::fs("control inventory entry", e))?;
            let value = entry
                .as_os_str()
                .to_str()
                .ok_or(BootstrapError::ManagementOnly("non-UTF-8 control entry"))?;
            match value {
                HEAD_FILE => (),
                _ if entry == *layout::CONTROL_WRITER_LOCK => (),
                _ => {
                    if AtomicFile::is_temporary_name(&entry) {
                        return Err(BootstrapError::ManagementOnly(
                            "unfinished atomic control write",
                        ));
                    }
                    let generation = value
                        .strip_prefix("record-")
                        .and_then(|s| s.strip_suffix(".json"))
                        .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|n| *n > 0)
                        .ok_or(BootstrapError::ManagementOnly(
                            "unknown control record name",
                        ))?;
                    inventory.count =
                        inventory
                            .count
                            .checked_add(1)
                            .ok_or(BootstrapError::ManagementOnly(
                                "control inventory count overflow",
                            ))?;
                    inventory.maximum = inventory.maximum.max(generation);
                }
            }
        }
        Ok(inventory)
    }
    fn read_head(&self) -> Result<Option<Head>> {
        self.read_optional(&name(HEAD_FILE)?, 2048)?
            .map(|bytes| Head::decode(&bytes))
            .transpose()
    }
    fn read_required(&self, name: &EntryName, maximum: usize) -> Result<Vec<u8>> {
        self.read_optional(name, maximum)?
            .ok_or(BootstrapError::ManagementOnly(
                "referenced control payload is missing",
            ))
    }
    fn read_optional(&self, name: &EntryName, maximum: usize) -> Result<Option<Vec<u8>>> {
        let Some(mut file) = self.open_optional(name)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        (&mut file)
            .take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| BootstrapError::io("bounded control read", e))?;
        if bytes.len() > maximum {
            return Err(BootstrapError::ManagementOnly(
                "control payload exceeds byte bound",
            ));
        }
        Ok(Some(bytes))
    }
    fn open_optional(&self, name: &EntryName) -> Result<Option<File>> {
        let file = match open_private_regular(&self.directory, name) {
            Ok(file) => file,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(BootstrapError::fs("control file open", error)),
        };
        let metadata = file
            .metadata()
            .map_err(|e| BootstrapError::io("control file metadata", e))?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(BootstrapError::ManagementOnly(
                "control file is not owned private mode 0600",
            ));
        }
        Ok(Some(file))
    }
    fn install_record(&self, name: &EntryName, bytes: &[u8]) -> Result<()> {
        match self.read_optional(name, MAX_RECORD_BYTES)? {
            Some(existing) if existing == bytes => self.sync_existing(name, bytes),
            Some(_) => Err(BootstrapError::ManagementOnly(
                "immutable control record collision",
            )),
            None => self.atomic(name, bytes, false),
        }
    }
    fn sync_existing(&self, name: &EntryName, bytes: &[u8]) -> Result<()> {
        // The control lock excludes cooperating mutation; name is opened with
        // no links and exact ownership before the held bytes are compared/synced.
        let mut file = self
            .open_optional(name)?
            .ok_or(BootstrapError::ManagementOnly("retry payload disappeared"))?;
        let mut existing = Vec::new();
        (&mut file)
            .take(bytes.len() as u64 + 1)
            .read_to_end(&mut existing)
            .map_err(|e| BootstrapError::io("retry payload read", e))?;
        if existing != bytes {
            return Err(BootstrapError::ManagementOnly("retry payload differs"));
        }
        file.sync_all()
            .map_err(|e| BootstrapError::io("retry file barrier", e))?;
        self.directory
            .sync()
            .map_err(|e| BootstrapError::fs("retry directory barrier", e))
    }
    fn atomic(&self, name: &EntryName, bytes: &[u8], replace: bool) -> Result<()> {
        let prepared = AtomicFile::stage(&self.directory, name)
            .map_err(|e| BootstrapError::fs("atomic stage", e))?
            .prepare(bytes)
            .map_err(|e| BootstrapError::fs("atomic file barrier", e))?;
        let result = if replace {
            prepared.replace()
        } else {
            prepared.create()
        };
        result
            .map(drop)
            .map_err(|source| BootstrapError::Commit { source })
    }
    #[cfg(test)]
    fn fail(&mut self, phase: WriteFault) -> Result<()> {
        if self.fault == Some(phase) {
            self.fault = None;
            return Err(BootstrapError::io(
                "injected interruption",
                std::io::Error::other(format!("interrupted at {phase:?}")),
            ));
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn inject(&mut self, phase: WriteFault) {
        self.fault = Some(phase);
    }
}
fn name(value: &str) -> Result<EntryName> {
    EntryName::new(value).map_err(|e| BootstrapError::fs("control entry name", e))
}
fn record_name(generation: u64) -> Result<EntryName> {
    name(&format!("record-{generation:020}.json"))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Inventory {
    count: u64,
    maximum: u64,
}
fn expected_records(generation: u64) -> Inventory {
    Inventory {
        count: generation,
        maximum: generation,
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/bootstrap_store_fixtures.rs"]
mod fixtures;
