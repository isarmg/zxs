//! Durable, conservative admission for descriptor-backed private SMB copies.
//!
//! A peer can retain a writable descriptor after disconnect. Its highest
//! admitted extent stays charged until explicit successful cleanup, including
//! across restart. Flush is not cleanup and never earns capacity back.
use super::*;
use crate::SpoolLimits;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AdvisoryLock, EntryName, PrivateDirectory, RelativePath};

const OVERHEAD: u64 = 64 * 1024;
const RECORD_LIMIT: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Owner {
    principal: String,
    source: String,
    #[serialization(with = "storage_contract::u64_string")]
    admitted_end: u64,
}
struct Charge {
    principal: Option<String>,
    bytes: u64,
}
#[derive(Default)]
struct Ledger {
    files: BTreeMap<PathBuf, Charge>,
    uncertain: bool,
    total: u64,
    unknown: u64,
    accounts: BTreeMap<String, u64>,
}
impl Ledger {
    fn rebuild(&mut self) -> Result<(), ApiError> {
        self.total = 0;
        self.unknown = 0;
        self.accounts.clear();
        for entry in self.files.values() {
            self.total = self
                .total
                .checked_add(entry.bytes)
                .ok_or_else(|| invalid("SMB accounting overflow"))?;
            let account = match &entry.principal {
                Some(principal) => self.accounts.entry(principal.clone()).or_default(),
                None => &mut self.unknown,
            };
            *account = account
                .checked_add(entry.bytes)
                .ok_or_else(|| invalid("SMB account overflow"))?;
        }
        Ok(())
    }
    fn replace(&mut self, path: PathBuf, next: Option<Charge>) -> Result<(), ApiError> {
        let previous = self.files.get(&path);
        let principal = next
            .as_ref()
            .or(previous)
            .and_then(|entry| entry.principal.clone());
        if previous
            .zip(next.as_ref())
            .is_some_and(|(old, new)| old.principal != new.principal)
        {
            return Err(invalid("SMB reservation owner changed"));
        }
        let old = previous.map_or(0, |entry| entry.bytes);
        let new = next.as_ref().map_or(0, |entry| entry.bytes);
        let update = |value: u64| {
            value
                .checked_sub(old)
                .and_then(|value| value.checked_add(new))
                .ok_or_else(|| invalid("SMB accounting overflow"))
        };
        let total = update(self.total)?;
        let account = update(principal.as_ref().map_or(self.unknown, |owner| {
            self.accounts.get(owner).copied().unwrap_or(0)
        }))?;
        self.total = total;
        if let Some(principal) = principal {
            if account == 0 {
                self.accounts.remove(&principal);
            } else {
                self.accounts.insert(principal, account);
            }
        } else {
            self.unknown = account;
        }
        if let Some(next) = next {
            self.files.insert(path, next);
        } else {
            self.files.remove(&path);
        }
        Ok(())
    }
}
pub(super) struct Budget {
    root: PathBuf,
    limits: SpoolLimits,
    ledger: Mutex<Ledger>,
    _lock: AdvisoryLock,
}
pub(super) struct Lease {
    budget: Arc<Budget>,
    source: PathBuf,
    max_length: u64,
    principal: String,
}
fn invalid(message: &str) -> ApiError {
    ApiError::new(
        ::http::StatusCode::SERVICE_UNAVAILABLE,
        storage_contract::ErrorCode::SmbSpoolUnavailable,
        message,
    )
}
fn record_path(source: &Path) -> PathBuf {
    source.with_extension("budget.json")
}
fn charge(length: u64) -> Result<u64, ApiError> {
    length
        .checked_add(OVERHEAD)
        .ok_or_else(|| invalid("SMB spool accounting overflow"))
}
fn source_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '.'])
}
impl Budget {
    pub(super) fn recover(root: &Path, limits: SpoolLimits) -> Result<Arc<Self>, ApiError> {
        if limits.bytes_per_account < OVERHEAD
            || limits.bytes_total < limits.bytes_per_account
            || limits.recovery_reserve_bytes == 0
        {
            return Err(ApiError::invalid(
                "SMB spool limits require account >= 65536, total >= account, and positive recovery reserve",
            ));
        }
        let directory = PrivateDirectory::open_trusted(root)
            .map_err(|source| invalid("SMB spool directory is unsafe").with_source(source))?;
        let lock = AdvisoryLock::acquire(
            &directory,
            &RelativePath::new(".budget.lock")
                .expect("invariant: static SMB budget lock filename is valid"),
        )
        .map_err(|source| {
            invalid("SMB spool lock is unsafe or already has an owner").with_source(source)
        })?;
        let mut inventory = BTreeMap::new();
        for entry in fs::read_dir(root).map_err(ApiError::io)? {
            let entry = entry.map_err(ApiError::io)?;
            let info = fs::symlink_metadata(entry.path()).map_err(ApiError::io)?;
            if entry.file_name() == "empty-directory" && info.is_dir() {
                continue;
            }
            if !info.is_file() || info.nlink() != 1 {
                return Err(invalid("SMB spool contains an aliased or non-regular file"));
            }
            if entry.file_name() != ".budget.lock" {
                inventory.insert(entry.path(), info.len());
            }
        }
        let mut ledger = Ledger::default();
        for (path, size) in &inventory {
            if !path.to_string_lossy().ends_with(".budget.json") {
                continue;
            }
            if *size > RECORD_LIMIT {
                return Err(invalid("SMB budget record is too large"));
            }
            let name = EntryName::new(path.file_name().unwrap())
                .map_err(|source| invalid("SMB budget filename is invalid").with_source(source))?;
            let bytes = directory
                .read_bounded(&name, RECORD_LIMIT as usize)
                .map_err(|source| {
                    invalid("SMB budget record is unsafe or unavailable").with_source(source)
                })?;
            let owner: Owner = storage_contract::decode_strict(&bytes)
                .map_err(|source| invalid("SMB budget record is invalid").with_source(source))?;
            if !source_name(&owner.source)
                || owner.principal.is_empty()
                || owner.principal.len() > 1024
            {
                return Err(invalid("SMB budget ownership is invalid"));
            }
            let source = root.join(&owner.source);
            if record_path(&source) != *path {
                return Err(invalid("SMB budget filename mismatch"));
            }
            let physical = inventory.get(&source).copied().unwrap_or(0);
            let metadata = inventory
                .get(&source.with_extension("work.json"))
                .copied()
                .unwrap_or(0)
                .saturating_add(
                    inventory
                        .get(&source.with_extension("namespace.json"))
                        .copied()
                        .unwrap_or(0),
                )
                .checked_add(*size)
                .ok_or_else(|| invalid("SMB metadata accounting overflow"))?;
            let bytes = owner
                .admitted_end
                .max(physical)
                .checked_add(OVERHEAD.max(metadata))
                .ok_or_else(|| invalid("SMB spool accounting overflow"))?;
            ledger.files.insert(
                source,
                Charge {
                    principal: Some(owner.principal),
                    bytes,
                },
            );
        }
        // Legacy and orphaned files count against every account as well as the
        // global limit. No inventory step deletes or trusts their ownership.
        let mut covered = BTreeSet::new();
        for source in ledger.files.keys() {
            covered.extend([
                source.clone(),
                record_path(source),
                source.with_extension("work.json"),
                source.with_extension("namespace.json"),
            ]);
        }
        for (path, size) in inventory {
            if !covered.contains(&path) {
                ledger.files.insert(
                    path,
                    Charge {
                        principal: None,
                        bytes: charge(size)?,
                    },
                );
            }
        }
        ledger.rebuild()?;
        Ok(Arc::new(Self {
            root: root.to_owned(),
            limits,
            ledger: Mutex::new(ledger),
            _lock: lock,
        }))
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        principal: &str,
        source: &Path,
        admitted_end: u64,
        max_length: u64,
    ) -> Result<Lease, ApiError> {
        let available = self.available()?;
        self.reserve_available(principal, source, admitted_end, max_length, available)
    }
    fn reserve_available(
        self: &Arc<Self>,
        principal: &str,
        source: &Path,
        admitted_end: u64,
        max_length: u64,
        available: u64,
    ) -> Result<Lease, ApiError> {
        if admitted_end > max_length {
            return Err(ApiError::limit("file exceeds SMB maximum length"));
        }
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| source_name(name))
            .ok_or_else(|| invalid("invalid SMB source name"))?;
        if source.parent() != Some(self.root.as_path())
            || principal.is_empty()
            || principal.len() > 1024
        {
            return Err(invalid("invalid SMB reservation identity"));
        }
        let mut ledger = self.ledger.lock().expect("service state lock poisoned");
        if ledger.uncertain || ledger.files.contains_key(source) {
            return Err(invalid("SMB spool needs reconciliation"));
        }
        self.check_admission(&ledger, principal, source, admitted_end, available)?;
        ledger.replace(
            source.to_owned(),
            Some(Charge {
                principal: Some(principal.to_owned()),
                bytes: charge(admitted_end)?,
            }),
        )?;
        let persist = (|| {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(record_path(source))
                .map_err(ApiError::io)?;
            let value = Owner {
                principal: principal.into(),
                source: name.into(),
                admitted_end,
            };
            file.write_all(
                &json_codec::to_vec(&value)
                    .expect("invariant: internal JSON serialization succeeds"),
            )
            .map_err(ApiError::io)?;
            file.sync_all().map_err(ApiError::io)?;
            File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(ApiError::io)
        })();
        if let Err(error) = persist {
            ledger.uncertain = true;
            return Err(error);
        }
        Ok(Lease {
            budget: self.clone(),
            source: source.to_owned(),
            max_length,
            principal: principal.to_owned(),
        })
    }
    fn check_admission(
        &self,
        ledger: &Ledger,
        principal: &str,
        source: &Path,
        admitted_end: u64,
        available: u64,
    ) -> Result<(), ApiError> {
        let baseline = ledger.files.get(source).map_or(0, |entry| entry.bytes);
        let requested = charge(admitted_end)?;
        let total = ledger
            .total
            .checked_sub(baseline)
            .and_then(|value| value.checked_add(requested))
            .ok_or_else(|| invalid("SMB accounting overflow"))?;
        let account = ledger
            .accounts
            .get(principal)
            .copied()
            .unwrap_or(0)
            .checked_add(ledger.unknown)
            .and_then(|value| value.checked_sub(baseline))
            .and_then(|value| value.checked_add(requested))
            .ok_or_else(|| invalid("SMB accounting overflow"))?;
        if total > self.limits.bytes_total || account > self.limits.bytes_per_account {
            return Err(ApiError::limit(
                "SMB retained copies and reservations exceed spool byte budgets",
            ));
        }
        // Count full retained admitted extents conservatively even when partly written.
        if available < total.saturating_add(self.limits.recovery_reserve_bytes) {
            return Err(ApiError::limit("SMB spool must preserve recovery headroom"));
        }
        Ok(())
    }
    fn available(&self) -> Result<u64, ApiError> {
        let info = rustix::fs::statvfs(&self.root)
            .map_err(io::Error::from)
            .map_err(ApiError::io)?;
        Ok(info.f_bavail.saturating_mul(info.f_frsize))
    }
}
impl Lease {
    pub(super) fn before_write(&self, end: u64, bytes: u64) -> Result<(), ApiError> {
        self.before_write_available(end, bytes, self.budget.available()?)
    }
    fn before_write_available(&self, end: u64, bytes: u64, available: u64) -> Result<(), ApiError> {
        if end > self.max_length {
            return Err(ApiError::limit("write exceeds SMB byte reservation"));
        }
        if available
            < self
                .budget
                .limits
                .recovery_reserve_bytes
                .saturating_add(bytes)
        {
            return Err(ApiError::limit("SMB write must preserve recovery headroom"));
        }
        let mut ledger = self
            .budget
            .ledger
            .lock()
            .expect("service state lock poisoned");
        if ledger.uncertain {
            return Err(invalid("SMB spool needs reconciliation"));
        }
        let previous = ledger
            .files
            .get(&self.source)
            .ok_or_else(|| invalid("SMB reservation missing"))?;
        if charge(end)? > previous.bytes {
            self.budget
                .check_admission(&ledger, &self.principal, &self.source, end, available)?;
            ledger.replace(
                self.source.clone(),
                Some(Charge {
                    principal: Some(self.principal.clone()),
                    bytes: charge(end)?,
                }),
            )?;
            // Persist the enlarged extent before acknowledging permission to
            // mutate the FD. Unknown metadata outcomes block further admission.
            let persist = (|| {
                let mut temporary = tempfile::Builder::new()
                    .prefix(".budget-grow-")
                    .tempfile_in(&self.budget.root)
                    .map_err(ApiError::io)?;
                let value = Owner {
                    principal: self.principal.clone(),
                    source: self.source.file_name().unwrap().to_str().unwrap().into(),
                    admitted_end: end,
                };
                temporary
                    .write_all(
                        &json_codec::to_vec(&value)
                            .expect("invariant: internal JSON serialization succeeds"),
                    )
                    .map_err(ApiError::io)?;
                temporary.as_file().sync_all().map_err(ApiError::io)?;
                temporary
                    .persist(record_path(&self.source))
                    .map_err(|error| ApiError::io(error.error))?;
                File::open(&self.budget.root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(ApiError::io)
            })();
            if let Err(error) = persist {
                ledger.uncertain = true;
                return Err(error);
            }
        }
        Ok(())
    }
    fn evidence_absent(&self) -> bool {
        [
            self.source.clone(),
            self.source.with_extension("work.json"),
            self.source.with_extension("namespace.json"),
        ]
        .iter()
        .all(|path| absent(path))
    }
    pub(super) fn cleanup_record(&self) {
        // Keep ownership metadata if cleanup of any data/recovery file failed.
        if self.evidence_absent() {
            let _ = fs::remove_file(record_path(&self.source));
        }
    }
}
fn absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cleanup_record();
        if self.evidence_absent()
            && absent(&record_path(&self.source))
            && let Ok(mut ledger) = self.budget.ledger.lock()
            && ledger.replace(self.source.clone(), None).is_err()
        {
            ledger.uncertain = true;
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/budget_tests.rs"]
mod tests;
