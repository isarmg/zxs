//! Recoverable admission for the HTTP spool, independent of engine/SMB budgets.
//!
//! Reservations precede body consumption. Their owner stays alive with every
//! blocking writer; dropping one settles to retained physical file length, not
//! upload success. Unknown files and uncertain I/O never earn free capacity.
use crate::{ApiError, lock, uploads::UploadRecord};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AdvisoryLock, EntryName, PrivateDirectory, RelativePath};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
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
fn metadata<T: data_serialization::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let directory = PrivateDirectory::open_trusted(path.parent()?).ok()?;
    let name = EntryName::new(path.file_name()?).ok()?;
    let bytes = directory.read_bounded(&name, MAX_METADATA as usize).ok()?;
    storage_contract::decode_strict(&bytes).ok()
}
fn charge_bytes(bytes: u64) -> Result<u64, ApiError> {
    bytes.checked_add(METADATA_ALLOWANCE).ok_or_else(|| {
        ApiError::new(
            500,
            storage_contract::ErrorCode::UploadStateInvalid,
            "spool accounting overflow",
        )
    })
}
fn invalid_state(message: &str) -> ApiError {
    ApiError::new(
        500,
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
            platform_io::fs_safety::Error::AlreadyLocked(_) => {
                invalid_state("HTTP spool is already owned by another server")
            }
            error => crate::upload_fs_error(error),
        })?;
        let mut files = BTreeMap::new();
        let mut owners = BTreeMap::new();
        let mut covered_metadata = BTreeMap::new();
        // Inventory, never cleanup. Corrupt/unrecognized regular files remain
        // charged as unknown rather than being silently discarded or ignored.
        for entry in fs::read_dir(root).map_err(ApiError::io)? {
            let entry = entry.map_err(ApiError::io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid_state("spool filenames must be valid UTF-8"))?;
            let info = fs::symlink_metadata(entry.path()).map_err(ApiError::io)?;
            if !info.is_file() {
                return Err(invalid_state("spool contains a non-regular file"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if info.nlink() != 1 {
                    return Err(invalid_state("spool contains an aliased data file"));
                }
            }
            if name == ".budget.lock" {
                continue;
            }
            files.insert(name.clone(), info.len());
            if name.ends_with(".owner.json") {
                if let Some(owner) = metadata::<Owner>(&entry.path()) {
                    if name != format!("{}.owner.json", owner.data_file) {
                        return Err(invalid_state("spool ownership filename mismatch"));
                    }
                    covered_metadata.insert(name, owner.data_file.clone());
                    set_owner(&mut owners, owner.data_file, owner.principal)?;
                }
            } else if name.contains(".json")
                && let Some(record) = metadata::<UploadRecord>(&entry.path())
            {
                covered_metadata.insert(name, record.data_file.clone());
                set_owner(&mut owners, record.data_file, record.principal)?;
            }
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
        let mut ledger = lock(&self.ledger)?;
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
        })
    }
}

pub(crate) struct Reservation {
    budget: Arc<SpoolBudget>,
    name: String,
    principal: String,
    max_length: u64,
}
impl Reservation {
    pub(crate) fn block_admission(&self) {
        if let Ok(mut ledger) = self.budget.ledger.lock() {
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
        if path.exists() {
            let owner = metadata::<Owner>(&path)
                .ok_or_else(|| invalid_state("invalid spool owner record"))?;
            if owner.data_file != self.name || owner.principal != self.principal {
                return Err(invalid_state("spool owner record mismatch"));
            }
            return Ok(());
        }
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(ApiError::io)?;
        let bytes = json_codec::to_vec(&Owner {
            data_file: self.name.clone(),
            principal: self.principal.clone(),
        })
        .map_err(ApiError::invalid)?;
        file.write_all(&bytes).map_err(ApiError::io)?;
        file.sync_all().map_err(ApiError::io)?;
        fs::File::open(&self.budget.root)
            .and_then(|file| file.sync_all())
            .map_err(ApiError::io)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let Ok(mut ledger) = self.budget.ledger.lock() else {
            return;
        };
        let Some(previous) = ledger.files.get(&self.name) else {
            ledger.uncertain = true;
            return;
        };
        let overhead = previous.overhead;
        let maximum = previous.bytes;
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
                    Err(_) => None,
                }
            }
            _ => None,
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
        if ledger.replace(&self.name, next).is_err() {
            ledger.uncertain = true;
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/spool_budget/tests.rs"]
mod tests;
