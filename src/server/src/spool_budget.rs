//! Recoverable admission for the HTTP spool, independent of engine/SMB budgets.
//!
//! Reservations precede body consumption. Their owner stays alive with every
//! blocking writer; dropping one settles to retained physical file length, not
//! upload success. Unknown files and uncertain I/O never earn free capacity.
use crate::{ApiError, lock, uploads::PersistedUploadRecord};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AdvisoryLock, EntryName, PrivateDirectory, RelativePath};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Covers owner, current/retained progress and one atomic metadata replacement.
/// This also bounds zero-byte uploads. It is a conservative logical charge,
/// not a claim about filesystem allocation, compression or sparse extents.
pub(crate) const METADATA_ALLOWANCE: u64 = 64 * 1024;
const MAX_METADATA: u64 = 16 * 1024;

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

#[derive(Serialize, Deserialize)]
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
}
impl Ledger {
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
    root: PathBuf,
    limits: SpoolLimits,
    ledger: Mutex<Ledger>,
    _directory_lock: AdvisoryLock,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}
fn metadata<T: data_serialization::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    let directory = PrivateDirectory::open_trusted(
        path.parent()
            .expect("invariant: spool entries have a parent"),
    )
    .map_err(crate::upload_fs_error)?;
    let name = EntryName::new(
        path.file_name()
            .expect("invariant: spool entries have a filename"),
    )
    .map_err(crate::upload_fs_error)?;
    let bytes = directory
        .read_bounded(&name, MAX_METADATA as usize)
        .map_err(crate::upload_fs_error)?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
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

/// The upload producer checkpoints a complete length before it can hand the
/// descriptor to the backend. An earlier checkpoint proves no such handoff;
/// full-length Incomplete and unknown records cannot make that claim.
fn incomplete_before_publication(name: &str, record: &PersistedUploadRecord) -> bool {
    record.state == crate::uploads::UploadState::Incomplete
        && record.persisted_bytes < record.logical_size
        && record.generation != 0
        && upload_data_name(&record.data_file, "upload-")
        && storage_contract::validate_id(&record.workspace_id, "ws").is_ok()
        && storage_contract::validate_id(&record.item_id, "entry").is_ok()
        && name_policy::PortablePath::parse(&record.path).is_ok()
        && record.persisted_hash.len() == 64
        && record.persisted_hash.bytes().all(|byte|
            byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && name == format!("{}.json", crate::uploads::key(&record.workspace_id, &record.item_id))
}

fn upload_data_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".data"))
        .is_some_and(|token| token.len() == 43 && token.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
        }))
}

fn discard_restart_uploads(
    directory: &PrivateDirectory,
    files: &mut BTreeMap<String, u64>,
    owners: &mut BTreeMap<String, String>,
    covered: &mut BTreeMap<String, String>,
    mut interrupted: Vec<(Option<String>, String)>,
) -> Result<(), ApiError> {
    let mut references = BTreeMap::<String, usize>::new();
    for data in covered.values() {
        *references.entry(data.clone()).or_default() += 1;
    }
    // One-shot draft/worktree HTTP bodies are only descriptor inputs. The
    // storage engine copies them into its own independently admitted material
    // and never references a spool name. After exclusive restart, an isolated
    // owner sidecar proves this isn't a retained resumable transfer.
    interrupted.extend(covered.iter().filter_map(|(metadata, data)| {
        (upload_data_name(data, "complete-upload-")
            && metadata == &format!("{data}.owner.json"))
            .then(|| (None, data.clone()))
    }));
    for (record, data) in interrupted {
        let owner = format!("{data}.owner.json");
        let owner_confirmed = covered.get(&owner) == Some(&data);
        let expected = usize::from(record.is_some()) + usize::from(owner_confirmed);
        if references.get(&data).copied() != Some(expected)
            || (files.contains_key(&data) && !owner_confirmed)
        {
            // A retained generation or missing attribution is not safe to drop.
            continue;
        }
        directory.verify_identity().map_err(crate::upload_fs_error)?;
        // Record last: another interrupted cleanup can resume from the same
        // incomplete evidence even if the data or owner is already absent.
        for name in [&data, &owner].into_iter().chain(record.as_ref()) {
            if files.contains_key(name) {
                let entry = EntryName::new(name).map_err(crate::upload_fs_error)?;
                directory.remove_file(&entry).map_err(crate::upload_fs_error)?;
                files.remove(name);
                covered.remove(name);
            }
        }
        owners.remove(&data);
    }
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
        let mut interrupted = Vec::new();
        let mut unknown_metadata = false;
        // Inventory the entire exclusive spool before touching any file.
        // Corrupt or unrecognized regular files remain charged as unknown.
        for entry in fs::read_dir(root).map_err(ApiError::io)? {
            let entry = entry.map_err(ApiError::io)?;
            let name = entry.file_name().into_string().map_err(|name| {
                invalid_state("spool filenames must be valid UTF-8")
                    .with_source(crate::http_boundary::ApiErrorSource::Filename(name))
            })?;
            let info = fs::symlink_metadata(entry.path()).map_err(ApiError::io)?;
            if !info.is_file() {
                return Err(invalid_state("spool contains a non-regular file"));
            }
            if info.nlink() != 1 {
                return Err(invalid_state("spool contains an aliased data file"));
            }
            if name == ".budget.lock" {
                continue;
            }
            files.insert(name.clone(), info.len());
            if name.ends_with(".owner.json") {
                match metadata::<Owner>(&entry.path()) {
                    Ok(owner) => {
                        if name != format!("{}.owner.json", owner.data_file) {
                            return Err(invalid_state("spool ownership filename mismatch"));
                        }
                        covered_metadata.insert(name, owner.data_file.clone());
                        set_owner(&mut owners, owner.data_file, owner.principal)?;
                    }
                    Err(error) => {
                        unknown_metadata = true;
                        tracing::warn!(error = ?error, "unreadable spool ownership remains charged as unknown")
                    }
                }
            } else if name.contains(".json") {
                match metadata::<PersistedUploadRecord>(&entry.path()) {
                    Ok(record) => {
                        if incomplete_before_publication(&name, &record) {
                            interrupted.push((Some(name.clone()), record.data_file.clone()));
                        }
                        covered_metadata.insert(name, record.data_file.clone());
                        set_owner(&mut owners, record.data_file, record.principal)?;
                    }
                    Err(error) => {
                        unknown_metadata = true;
                        tracing::warn!(error = ?error, "unreadable spool metadata remains charged as unknown")
                    }
                }
            }
        }
        if !unknown_metadata {
            discard_restart_uploads(
                &directory, &mut files, &mut owners, &mut covered_metadata, interrupted,
            )?;
        }
        let mut ledger = Ledger::default();
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
        Ok(Arc::new(Self {
            root: root.to_owned(),
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
        let available = rustix::fs::statvfs(&self.root)
            .map_err(std::io::Error::from)
            .map_err(ApiError::io)?;
        self.reserve_with_available(
            principal,
            name,
            length,
            available.f_bavail.saturating_mul(available.f_frsize),
        )
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
            principal: principal.to_owned(),
            max_length: length,
            overhead,
            charged_bytes: bytes,
        })
    }
}

pub(crate) struct Reservation {
    budget: Arc<SpoolBudget>,
    name: String,
    principal: String,
    max_length: u64,
    overhead: u64,
    charged_bytes: u64,
}
impl Reservation {
    pub(crate) fn block_admission(&self) {
        {
            let mut ledger = self
                .budget
                .ledger
                .lock()
                .expect("service state lock poisoned");
            ledger.uncertain = true;
        }
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
        let available = rustix::fs::statvfs(&self.budget.root)
            .map_err(std::io::Error::from)
            .map_err(ApiError::io)?;
        if available.f_bavail.saturating_mul(available.f_frsize)
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
    /// Establish attribution before creating/writing a new payload. On any
    /// uncertain outcome both files are retained; recovery inventories them.
    pub(crate) fn persist_owner(&self) -> Result<(), ApiError> {
        let path = self.budget.root.join(format!("{}.owner.json", self.name));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let owner = metadata::<Owner>(&path)?;
                if owner.data_file != self.name || owner.principal != self.principal {
                    return Err(invalid_state("spool owner record mismatch"));
                }
                return Ok(());
            }
            Err(error) => return Err(ApiError::io(error)),
        };
        let bytes = json_codec::to_vec(&Owner {
            data_file: self.name.clone(),
            principal: self.principal.clone(),
        })
        .expect("invariant: internal JSON serialization succeeds");
        file.write_all(&bytes).map_err(ApiError::io)?;
        file.sync_all().map_err(ApiError::io)?;
        fs::File::open(&self.budget.root)
            .and_then(|file| file.sync_all())
            .map_err(ApiError::io)
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
        let overhead = self.overhead;
        let maximum = self.charged_bytes;
        // Last writer/metadata task has finished. Unknown durability and upload
        // success are irrelevant: charge every retained physical byte.
        let retained = match fs::symlink_metadata(self.budget.root.join(&self.name)) {
            Ok(info) if info.is_file() => info.len().checked_add(overhead),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::symlink_metadata(
                    self.budget.root.join(format!("{}.owner.json", self.name)),
                ) {
                    Ok(_) => Some(overhead),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
                    Err(error) => {
                        tracing::warn!(error = ?error, "spool ownership cannot be inventoried; admission blocked");
                        None
                    }
                }
            }
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(error = ?error, "spool payload cannot be inventoried; admission blocked");
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
