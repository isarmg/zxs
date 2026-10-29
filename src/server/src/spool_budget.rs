//! Recoverable admission for the HTTP spool, independent of engine/SMB budgets.
//!
//! Reservations precede body consumption. Their owner stays alive with every
//! blocking writer; dropping one settles to retained physical file length, not
//! upload success. Unknown files and uncertain I/O never earn free capacity.
use crate::{ApiError, lock, uploads::UploadRecord};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AdvisoryLock, AtomicFile, EntryMetadata, EntryName, PrivateDirectory, RelativePath,
};
#[cfg(test)]
use std::fs;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
};

/// Covers owner, current/retained progress and one atomic metadata replacement.
/// This also bounds zero-byte uploads. It is a conservative logical charge,
/// not a claim about filesystem allocation, compression or sparse extents.
pub(crate) const METADATA_ALLOWANCE: u64 = 64 * 1024;
const MAX_METADATA: u64 = 16 * 1024;
mod retirement;

#[derive(Clone, Debug)]
pub struct SpoolLimits {
    pub bytes_per_account: u64,
    pub bytes_total: u64,
    /// Free filesystem bytes kept outside all admitted upload reservations.
    pub recovery_reserve_bytes: u64,
}
impl Default for SpoolLimits {
    fn default() -> Self {
        Self {
            bytes_per_account: 100 * 1024 * 1024 * 1024,
            bytes_total: 500 * 1024 * 1024 * 1024,
            recovery_reserve_bytes: 1024 * 1024 * 1024,
        }
    }
}
impl SpoolLimits {
    pub(crate) fn validate(&self) -> Result<(), ApiError> {
        if self.bytes_per_account < METADATA_ALLOWANCE
            || self.bytes_total < self.bytes_per_account
            || self.recovery_reserve_bytes == 0
        {
            return Err(ApiError::invalid(
                "HTTP spool limits require account >= 65536, total >= account, and positive recovery reserve",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Owner {
    data_file: String,
    principal: String,
}
struct Charge {
    owner: Option<String>,
    bytes: u64,
    overhead: u64,
    /// Keep the whole reservation in the free-space calculation while active.
    /// Written bytes may therefore be counted twice there, conservatively.
    reserved: u64,
}
#[derive(Default)]
struct Ledger {
    files: BTreeMap<String, Charge>,
    uncertain: bool,
    total: u64,
    unknown: u64,
    accounts: BTreeMap<String, u64>,
    reserved: u64,
    metadata: BTreeMap<String, MetadataReference>,
    references: BTreeMap<String, BTreeSet<String>>,
    current_by_workspace: BTreeMap<String, BTreeSet<String>>,
    maintenance_issues: BTreeSet<String>,
}
#[derive(Clone, PartialEq, Eq)]
enum MetadataReference {
    Owner(Owner),
    Upload(UploadRecord),
}
impl MetadataReference {
    fn data_file(&self) -> &str {
        match self {
            Self::Owner(owner) => &owner.data_file,
            Self::Upload(record) => &record.data_file,
        }
    }
}
impl Ledger {
    fn current_names(&self, workspace: &str, item: Option<&str>) -> Vec<String> {
        let Some(names) = self.current_by_workspace.get(workspace) else {
            return Vec::new();
        };
        match item {
            Some(item) => {
                let name = format!("{}.json", crate::uploads::key(workspace, item));
                if names.contains(&name) {
                    vec![name]
                } else {
                    Vec::new()
                }
            }
            None => names.iter().cloned().collect(),
        }
    }
    fn remove_current(&mut self, name: &str, reference: &MetadataReference) {
        if let MetadataReference::Upload(record) = reference
            && name == retirement::current_name(record)
        {
            if let Some(names) = self.current_by_workspace.get_mut(&record.workspace_id) {
                names.remove(name);
                if names.is_empty() {
                    self.current_by_workspace.remove(&record.workspace_id);
                }
            }
            self.maintenance_issues.remove(name);
        }
    }

    fn record_metadata(&mut self, name: String, reference: MetadataReference) {
        if let Some(previous) = self.metadata.remove(&name) {
            self.remove_current(&name, &previous);
            let names = self
                .references
                .get_mut(previous.data_file())
                .expect("invariant: indexed metadata has its reference set");
            names.remove(&name);
            if names.is_empty() {
                self.references.remove(previous.data_file());
            }
        }
        self.references
            .entry(reference.data_file().to_owned())
            .or_default()
            .insert(name.clone());
        if let MetadataReference::Upload(record) = &reference
            && record.state == crate::uploads::UploadState::Complete
            && name == retirement::current_name(record)
        {
            self.current_by_workspace
                .entry(record.workspace_id.clone())
                .or_default()
                .insert(name.clone());
        }
        self.metadata.insert(name, reference);
    }

    fn maintenance_failure(&mut self, name: &str, error: &ApiError) {
        if self.maintenance_issues.insert(name.to_owned()) {
            tracing::warn!(error = ?error, "HTTP copy maintenance failed; copy remains charged");
        }
    }
    fn rebuild_totals(&mut self) -> Result<(), ApiError> {
        self.total = 0;
        self.unknown = 0;
        self.reserved = 0;
        self.accounts.clear();
        for entry in self.files.values() {
            self.total = self
                .total
                .checked_add(entry.bytes)
                .ok_or_else(|| invalid_state("spool accounting overflow"))?;
            if let Some(owner) = &entry.owner {
                let count = self.accounts.entry(owner.clone()).or_default();
                *count = count
                    .checked_add(entry.bytes)
                    .ok_or_else(|| invalid_state("spool account overflow"))?;
            } else {
                self.unknown = self
                    .unknown
                    .checked_add(entry.bytes)
                    .ok_or_else(|| invalid_state("spool accounting overflow"))?;
            }
            self.reserved = self
                .reserved
                .checked_add(entry.reserved)
                .ok_or_else(|| invalid_state("spool reservation overflow"))?;
        }
        Ok(())
    }
    /// All live accounting changes use this one transition. Ownership never
    /// changes through admission; recovery is the only attribution boundary.
    fn replace(&mut self, name: &str, next: Option<Charge>) -> Result<(), ApiError> {
        let old = self.files.get(name);
        let owner = next
            .as_ref()
            .and_then(|entry| entry.owner.as_ref())
            .or_else(|| old.and_then(|entry| entry.owner.as_ref()))
            .cloned();
        if old
            .zip(next.as_ref())
            .is_some_and(|(old, next)| old.owner != next.owner)
        {
            return Err(invalid_state(
                "spool ownership cannot change during admission",
            ));
        }
        let old_bytes = old.map_or(0, |entry| entry.bytes);
        let new_bytes = next.as_ref().map_or(0, |entry| entry.bytes);
        let old_reserved = old.map_or(0, |entry| entry.reserved);
        let new_reserved = next.as_ref().map_or(0, |entry| entry.reserved);
        let update = |value: u64, before: u64, after: u64| {
            value
                .checked_sub(before)
                .and_then(|value| value.checked_add(after))
                .ok_or_else(|| invalid_state("spool accounting overflow"))
        };
        let total = update(self.total, old_bytes, new_bytes)?;
        let reserved = update(self.reserved, old_reserved, new_reserved)?;
        let account = update(
            owner.as_ref().map_or(self.unknown, |owner| {
                self.accounts.get(owner).copied().unwrap_or(0)
            }),
            old_bytes,
            new_bytes,
        )?;
        self.total = total;
        self.reserved = reserved;
        if let Some(owner) = owner {
            if account == 0 {
                self.accounts.remove(&owner);
            } else {
                self.accounts.insert(owner, account);
            }
        } else {
            self.unknown = account;
        }
        match next {
            Some(entry) => {
                self.files.insert(name.to_owned(), entry);
            }
            None => {
                self.files.remove(name);
            }
        }
        Ok(())
    }
}
pub(crate) struct SpoolBudget {
    directory: Arc<PrivateDirectory>,
    limits: SpoolLimits,
    ledger: Mutex<Ledger>,
    _directory_lock: AdvisoryLock,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}
fn metadata<T: data_serialization::de::DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &EntryName,
) -> Result<T, ApiError> {
    let bytes = directory
        .read_bounded(name, MAX_METADATA as usize)
        .map_err(crate::upload_fs_error)?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}
fn inventory_error(error: platform_io::fs_safety::Error) -> ApiError {
    match error {
        platform_io::fs_safety::Error::Io(error) => ApiError::io(error),
        error => crate::upload_fs_error(error),
    }
}

fn charge_bytes(bytes: u64) -> Result<u64, ApiError> {
    bytes.checked_add(METADATA_ALLOWANCE).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::UploadStateInvalid,
            "spool accounting overflow",
        )
    })
}
fn invalid_state(message: &str) -> ApiError {
    ApiError::new(
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::UploadStateInvalid,
        message,
    )
}
fn set_owner(
    owners: &mut BTreeMap<String, String>,
    name: String,
    owner: String,
) -> Result<(), ApiError> {
    if !valid_name(&name) || owner.is_empty() || owner.len() > 1024 {
        return Err(invalid_state("invalid spool ownership metadata"));
    }
    if owners.get(&name).is_some_and(|previous| previous != &owner) {
        return Err(invalid_state("conflicting spool ownership metadata"));
    }
    owners.insert(name, owner);
    Ok(())
}

impl SpoolBudget {
    pub(crate) fn recover(root: &Path, limits: SpoolLimits) -> Result<Arc<Self>, ApiError> {
        limits.validate()?;
        let directory = PrivateDirectory::open_trusted(root).map_err(crate::upload_fs_error)?;
        let directory_lock = AdvisoryLock::acquire(
            &directory,
            &RelativePath::new(".budget.lock").map_err(crate::upload_fs_error)?,
        )
        .map_err(|error| match error {
            source @ platform_io::fs_safety::Error::AlreadyLocked(_) => {
                invalid_state("HTTP spool is already owned by another server").with_source(source)
            }
            error => crate::upload_fs_error(error),
        })?;
        let mut files = BTreeMap::new();
        let mut owners = BTreeMap::new();
        let mut covered_metadata = BTreeMap::new();
        let mut ledger = Ledger::default();
        // Inventory, never cleanup. Corrupt/unrecognized regular files remain
        // charged as unknown rather than being silently discarded or ignored.
        for entry in directory.entries().map_err(inventory_error)? {
            let entry = entry.map_err(inventory_error)?;
            let name = entry
                .as_os_str()
                .to_str()
                .ok_or_else(|| {
                    invalid_state("spool filenames must be valid UTF-8").with_source(
                        crate::http_boundary::ApiErrorSource::Filename(
                            entry.as_os_str().to_owned(),
                        ),
                    )
                })?
                .to_owned();
            let size = match directory.entry_metadata(&entry).map_err(inventory_error)? {
                EntryMetadata::File { bytes } => bytes,
                EntryMetadata::Directory => {
                    return Err(invalid_state("spool contains a non-regular file"));
                }
            };
            if name == ".budget.lock" {
                continue;
            }
            files.insert(name.clone(), size);
            if name.ends_with(".owner.json") {
                match metadata::<Owner>(&directory, &entry) {
                    Ok(owner) => {
                        if name != format!("{}.owner.json", owner.data_file) {
                            return Err(invalid_state("spool ownership filename mismatch"));
                        }
                        ledger
                            .record_metadata(name.clone(), MetadataReference::Owner(owner.clone()));
                        covered_metadata.insert(name, owner.data_file.clone());
                        set_owner(&mut owners, owner.data_file, owner.principal)?;
                    }
                    Err(error) => {
                        tracing::warn!(error = ?error, "unreadable spool ownership remains charged as unknown")
                    }
                }
            } else if name.contains(".json") {
                match metadata::<UploadRecord>(&directory, &entry) {
                    Ok(record) => {
                        ledger.record_metadata(
                            name.clone(),
                            MetadataReference::Upload(record.clone()),
                        );
                        covered_metadata.insert(name, record.data_file.clone());
                        set_owner(&mut owners, record.data_file, record.principal)?;
                    }
                    Err(error) => {
                        tracing::warn!(error = ?error, "unreadable spool metadata remains charged as unknown")
                    }
                }
            }
        }
        // A durable owner record with a missing data file still costs metadata.
        for (name, principal) in owners {
            let bytes = files.get(&name).copied().unwrap_or(0);
            ledger.files.insert(
                name,
                Charge {
                    owner: Some(principal),
                    bytes: charge_bytes(bytes)?,
                    overhead: METADATA_ALLOWANCE,
                    reserved: 0,
                },
            );
        }
        let mut metadata_sizes = BTreeMap::<String, u64>::new();
        for (name, bytes) in files {
            if ledger.files.contains_key(&name) {
                continue;
            }
            if let Some(data) = covered_metadata.get(&name) {
                let size = metadata_sizes.entry(data.clone()).or_default();
                *size = size
                    .checked_add(bytes)
                    .ok_or_else(|| invalid_state("spool accounting overflow"))?;
            } else {
                ledger.files.insert(
                    name,
                    Charge {
                        owner: None,
                        bytes: charge_bytes(bytes)?,
                        overhead: METADATA_ALLOWANCE,
                        reserved: 0,
                    },
                );
            }
        }
        // A damaged/legacy directory may contain more metadata than the normal
        // per-file allowance. Count the excess rather than assuming a bound.
        for (name, bytes) in metadata_sizes {
            if bytes > METADATA_ALLOWANCE {
                let entry = ledger
                    .files
                    .get_mut(&name)
                    .ok_or_else(|| invalid_state("missing spool owner"))?;
                entry.overhead = bytes;
                entry.bytes = entry
                    .bytes
                    .checked_add(bytes - METADATA_ALLOWANCE)
                    .ok_or_else(|| invalid_state("spool accounting overflow"))?;
            }
        }
        ledger.rebuild_totals()?;
        directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        Ok(Arc::new(Self {
            directory: Arc::new(directory),
            limits,
            ledger: Mutex::new(ledger),
            _directory_lock: directory_lock,
        }))
    }

    pub(crate) fn reserve(
        self: &Arc<Self>,
        principal: &str,
        name: &str,
        length: u64,
    ) -> Result<Reservation, ApiError> {
        let available = self.directory.available_bytes().map_err(ApiError::io)?;
        self.reserve_with_available(principal, name, length, available)
    }

    fn reserve_with_available(
        self: &Arc<Self>,
        principal: &str,
        name: &str,
        length: u64,
        available: u64,
    ) -> Result<Reservation, ApiError> {
        if !valid_name(name) || principal.is_empty() || principal.len() > 1024 {
            return Err(ApiError::invalid("invalid spool reservation identity"));
        }
        self.directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        let payload_name = EntryName::new(name).map_err(crate::upload_fs_error)?;
        let owner_name =
            EntryName::new(format!("{name}.owner.json")).map_err(crate::upload_fs_error)?;
        let mut ledger = lock(&self.ledger);
        if ledger.uncertain {
            return Err(invalid_state(
                "spool accounting requires restart and inventory",
            ));
        }
        let previous = ledger.files.get(name);
        if previous
            .is_some_and(|entry| entry.reserved != 0 || entry.owner.as_deref() != Some(principal))
        {
            return Err(invalid_state(
                "spool file is already reserved or has another owner",
            ));
        }
        let overhead = previous.map_or(METADATA_ALLOWANCE, |entry| entry.overhead);
        let requested = length
            .checked_add(overhead)
            .ok_or_else(|| invalid_state("spool accounting overflow"))?;
        let baseline = previous.map_or(0, |entry| entry.bytes);
        let bytes = requested.max(baseline);
        let growth = bytes - baseline;
        let total = ledger.total;
        let account = ledger
            .accounts
            .get(principal)
            .copied()
            .unwrap_or(0)
            .checked_add(ledger.unknown)
            .ok_or_else(|| invalid_state("spool accounting overflow"))?;
        if growth > self.limits.bytes_total.saturating_sub(total)
            || growth > self.limits.bytes_per_account.saturating_sub(account)
            || (growth > 0
                && (total > self.limits.bytes_total || account > self.limits.bytes_per_account))
        {
            return Err(ApiError::limit(
                "HTTP spool retained data and reservations exceed the account or total budget",
            ));
        }
        let reserved = ledger.reserved;
        if available
            < self
                .limits
                .recovery_reserve_bytes
                .saturating_add(reserved)
                .saturating_add(growth)
        {
            return Err(ApiError::limit(
                "HTTP spool must preserve filesystem recovery headroom",
            ));
        }
        ledger.replace(
            name,
            Some(Charge {
                owner: Some(principal.to_owned()),
                bytes,
                overhead,
                reserved: growth.max(1),
            }),
        )?;
        Ok(Reservation {
            budget: self.clone(),
            name: name.to_owned(),
            payload_name,
            owner_name,
            principal: principal.to_owned(),
            max_length: length,
            overhead,
            charged_bytes: bytes,
            settlement_failed: false,
        })
    }
}

pub(crate) struct Reservation {
    budget: Arc<SpoolBudget>,
    name: String,
    payload_name: EntryName,
    owner_name: EntryName,
    principal: String,
    max_length: u64,
    overhead: u64,
    charged_bytes: u64,
    settlement_failed: bool,
}
impl Reservation {
    pub(crate) fn record_upload(&self, name: String, record: &UploadRecord) {
        lock(&self.budget.ledger).record_metadata(name, MetadataReference::Upload(record.clone()));
    }
    pub(crate) fn directory(&self) -> &Arc<PrivateDirectory> {
        &self.budget.directory
    }
    pub(crate) fn payload_name(&self) -> &EntryName {
        &self.payload_name
    }

    pub(crate) fn retain_unsettled_charge(&mut self) {
        self.settlement_failed = true;
    }

    pub(crate) fn block_admission(&self) {
        let mut ledger = self
            .budget
            .ledger
            .lock()
            .expect("service state lock poisoned");
        ledger.uncertain = true;
    }
    pub(crate) fn check_headroom(
        &self,
        current_length: u64,
        next_write: u64,
    ) -> Result<(), ApiError> {
        if current_length
            .checked_add(next_write)
            .is_none_or(|length| length > self.max_length)
        {
            return Err(invalid_state("write exceeds its spool reservation"));
        }
        let available = self
            .budget
            .directory
            .available_bytes()
            .map_err(ApiError::io)?;
        if available
            < self
                .budget
                .limits
                .recovery_reserve_bytes
                .saturating_add(next_write)
        {
            return Err(ApiError::limit(
                "HTTP spool must preserve filesystem recovery headroom",
            ));
        }
        Ok(())
    }
    /// Establish attribution before creating/writing a new payload. Failure
    /// retains existing evidence and blocks admission until recovery inventories it.
    pub(crate) fn persist_owner(&self) -> Result<(), ApiError> {
        let result = (|| {
            let directory = &self.budget.directory;
            directory
                .verify_identity()
                .map_err(crate::upload_fs_error)?;
            match directory.read_bounded(&self.owner_name, MAX_METADATA as usize) {
                Ok(bytes) => return self.check_owner(&bytes),
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(crate::upload_fs_error(error)),
            }
            let bytes = json_codec::to_vec(&Owner {
                data_file: self.name.clone(),
                principal: self.principal.clone(),
            })
            .expect("invariant: internal JSON serialization succeeds");
            match AtomicFile::create(directory, &self.owner_name, &bytes) {
                Ok(()) => Ok(()),
                Err(platform_io::fs_safety::Error::DestinationExists(_)) => {
                    let bytes = directory
                        .read_bounded(&self.owner_name, MAX_METADATA as usize)
                        .map_err(crate::upload_fs_error)?;
                    self.check_owner(&bytes)
                }
                Err(error) => Err(crate::upload_fs_error(error)),
            }
        })();
        if result.is_err() {
            self.block_admission();
        } else {
            lock(&self.budget.ledger).record_metadata(
                format!("{}.owner.json", self.name),
                MetadataReference::Owner(Owner {
                    data_file: self.name.clone(),
                    principal: self.principal.clone(),
                }),
            );
        }
        result
    }
    fn check_owner(&self, bytes: &[u8]) -> Result<(), ApiError> {
        let owner: Owner =
            storage_contract::decode_strict(bytes).map_err(ApiError::invalid_source)?;
        if owner.data_file != self.name || owner.principal != self.principal {
            return Err(invalid_state("spool owner record mismatch"));
        }
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut ledger = match self.budget.ledger.lock() {
            Ok(ledger) => ledger,
            Err(error) => {
                tracing::error!(%error, "spool settlement skipped; ledger remains poisoned");
                return;
            }
        };
        if self.settlement_failed {
            ledger.uncertain = true;
            return;
        }
        if ledger.uncertain {
            return;
        }
        let overhead = self.overhead;
        let maximum = self.charged_bytes;
        // Only this verified directory can settle the last task's retained bytes.
        let retained = (|| -> Result<u64, ApiError> {
            let directory = &self.budget.directory;
            directory
                .verify_identity()
                .map_err(crate::upload_fs_error)?;
            let bytes = match directory.entry_metadata(&self.payload_name) {
                Ok(EntryMetadata::File { bytes }) => bytes.checked_add(overhead),
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    match directory.entry_metadata(&self.owner_name) {
                        Ok(EntryMetadata::File { .. }) => Some(overhead),
                        Err(platform_io::fs_safety::Error::Io(error))
                            if error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            Some(0)
                        }
                        Ok(EntryMetadata::Directory) => {
                            return Err(invalid_state("spool ownership is not a regular file"));
                        }
                        Err(error) => return Err(inventory_error(error)),
                    }
                }
                Ok(EntryMetadata::Directory) => {
                    return Err(invalid_state("spool payload is not a regular file"));
                }
                Err(error) => return Err(inventory_error(error)),
            };
            bytes.ok_or_else(|| invalid_state("spool settlement accounting overflow"))
        })();
        let retained = match retained {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                tracing::warn!(error = ?error, "spool accounting could not settle; admission blocked");
                None
            }
        };
        let Some(bytes) = retained.filter(|bytes| *bytes <= maximum) else {
            ledger.uncertain = true;
            return;
        };
        let next = (bytes > 0).then(|| Charge {
            owner: Some(self.principal.clone()),
            bytes,
            overhead,
            reserved: 0,
        });
        if let Err(error) = ledger.replace(&self.name, next) {
            ledger.uncertain = true;
            tracing::warn!(error = ?error, "spool accounting could not settle; admission blocked");
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/spool_budget/tests.rs"]
mod tests;
