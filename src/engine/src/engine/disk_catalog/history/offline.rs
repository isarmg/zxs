//! Recover the *local inventory representation* without a SYSTEM root or live
//! Engine. Self-consistent SQLite is not authenticated publication evidence and
//! never authorizes restore, registration, content access, or authority selection.
use super::*;
use crate::disk::{DiskRoot, HeldRegular, MAX_METADATA};
use std::{fs::File, path::PathBuf};

/// Explicitly selecting one intact file permits salvage when the other is
/// corrupt. An unselected file is not read, classified as absent, or repaired.
#[derive(Clone, Copy, Debug, Serialize)]
#[serialization(rename_all = "kebab-case")]
pub enum DiskDatabaseReadScope {
    Both,
    Current,
    History,
}
impl DiskDatabaseReadScope {
    fn current(self) -> bool {
        matches!(self, Self::Both | Self::Current)
    }
    fn history(self) -> bool {
        matches!(self, Self::Both | Self::History)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case")]
pub enum DiskDatabaseRelation {
    CurrentOnly,
    HistoryOnly,
    /// The current rows equal folding the stored history through that current
    /// file's declared sequence. Neither sequence is independently authenticated.
    CurrentCoveredByHistory,
    /// No statement about changes after the history boundary can be made.
    HistoryBehindCurrent,
}

/// A frozen read result, not an admission capability. Optional members preserve
/// absence; an absent file is never synthesized and written back to the disk.
pub struct DiskDatabaseMaterial {
    scope: DiskDatabaseReadScope,
    identity: DiskRegistrationIdentity,
    current: Option<DiskCatalogSnapshot>,
    history: Option<DiskHistorySnapshot>,
    history_inventory: Option<DiskCatalogSnapshot>,
    relation: DiskDatabaseRelation,
}
impl DiskDatabaseMaterial {
    pub fn scope(&self) -> DiskDatabaseReadScope {
        self.scope
    }
    pub fn identity(&self) -> &DiskRegistrationIdentity {
        &self.identity
    }
    pub fn current(&self) -> Option<&DiskCatalogSnapshot> {
        self.current.as_ref()
    }
    pub fn history(&self) -> Option<&DiskHistorySnapshot> {
        self.history.as_ref()
    }
    /// Derived in memory by exact Entered/Changed/Departed replay from empty.
    /// This can recover a missing current inventory *representation*, not a
    /// missing authority or proof of the latest state/content availability.
    pub fn history_inventory(&self) -> Option<&DiskCatalogSnapshot> {
        self.history_inventory.as_ref()
    }
    pub fn relation(&self) -> &DiskDatabaseRelation {
        &self.relation
    }
}

/// Inspect only the selected current-format disk, with all writers stopped by
/// the caller. The expected identity is explicit; neither a path nor a stored
/// SQLite scope selects which pool/registration the caller intended to inspect.
/// Reads remain on the supplied descriptor. No SYSTEM root, other disk, journal
/// writer, lock sidecar, installer, directory creation or repair is opened.
pub fn inspect_disk_database_material(
    directory: File,
    path: PathBuf,
    expected: &DiskRegistrationIdentity,
    scope: DiskDatabaseReadScope,
) -> Result<DiskDatabaseMaterial> {
    let root = DiskRoot::from_directory(path, directory)?;
    let mut seal = HeldRegular::capture(&root, "disk.json", MAX_METADATA)?
        .ok_or_else(|| invalid("disk database inspection requires an existing disk seal"))?;
    seal.release_bytes();
    let recognized =
        crate::disk::recognize_disk(root.directory(root.path())?, root.path().to_path_buf())?;
    if recognized.identity() != expected {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "selected disk differs from the explicit database inspection identity",
        ));
    }
    let mut current_file = if scope.current() {
        HeldRegular::capture(&root, DISK_DATABASE_NAME, MAX_DATABASE_BYTES)?
    } else {
        None
    };
    let current = current_file
        .as_ref()
        .map(|file| super::super::database::read(file.bytes(), expected))
        .transpose()?;
    if let Some(file) = &mut current_file {
        file.release_bytes();
    }
    let mut history_file = if scope.history() {
        HeldRegular::capture(&root, DISK_HISTORY_DATABASE_NAME, MAX_DATABASE_BYTES)?
    } else {
        None
    };
    let history = history_file
        .as_ref()
        .map(|file| database::read(file.bytes(), expected))
        .transpose()?;
    if let Some(file) = &mut history_file {
        file.release_bytes();
    }
    if current.is_none() && history.is_none() {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "no selected disk database is present; absence is not an empty recovered inventory",
        ));
    }
    let history_inventory = history
        .as_ref()
        .map(|history| fold(history, current.as_ref()))
        .transpose()?;
    let relation = match (&current, &history) {
        (Some(_), None) => DiskDatabaseRelation::CurrentOnly,
        (None, Some(_)) => DiskDatabaseRelation::HistoryOnly,
        (Some(current), Some(history))
            if current.source_sequence <= history.header.through_sequence =>
        {
            DiskDatabaseRelation::CurrentCoveredByHistory
        }
        (Some(_), Some(_)) => DiskDatabaseRelation::HistoryBehindCurrent,
        (None, None) => unreachable!("invariant: empty material rejected above"),
    };
    // Keep the original FDs alive while checking names, bytes, and absence. A
    // replaced named file cannot recycle its inode into the previous stamp.
    recheck(&root, "disk.json", Some(&seal), MAX_METADATA)?;
    if scope.current() {
        recheck(
            &root,
            DISK_DATABASE_NAME,
            current_file.as_ref(),
            MAX_DATABASE_BYTES,
        )?;
    }
    if scope.history() {
        recheck(
            &root,
            DISK_HISTORY_DATABASE_NAME,
            history_file.as_ref(),
            MAX_DATABASE_BYTES,
        )?;
    }
    let after =
        crate::disk::recognize_disk(root.directory(root.path())?, root.path().to_path_buf())?;
    if after != recognized {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "selected disk identity or local configuration changed during database inspection",
        ));
    }
    Ok(DiskDatabaseMaterial {
        scope,
        identity: expected.clone(),
        current,
        history,
        history_inventory,
        relation,
    })
}

fn recheck(
    root: &DiskRoot,
    leaf: &str,
    original: Option<&HeldRegular>,
    maximum: usize,
) -> Result<()> {
    let current = HeldRegular::capture(root, leaf, maximum)?;
    if current.as_ref().map(HeldRegular::stamp) != original.map(HeldRegular::stamp) {
        return Err(invalid(
            "disk database material changed during the frozen offline read",
        ));
    }
    Ok(())
}

fn validate_header(history: &DiskHistorySnapshot) -> Result<()> {
    let header = &history.header;
    match (
        &header.through_snapshot,
        header.through_sequence,
        header.publications,
    ) {
        (None, 0, 0) if history.changes.is_empty() => (),
        (Some(hash), sequence, publications) if sequence > 0 && publications > 0 => {
            hash.parse::<storage_contract::ContentHash>()
                .map_err(|source| Error::from(std::io::Error::other(source)))?;
            if u64::from(publications) > sequence {
                return Err(invalid(
                    "history has more publications than possible sequence slots",
                ));
            }
        }
        _ => {
            return Err(invalid(
                "disk history has inconsistent genesis/coverage fields",
            ));
        }
    }
    Ok(())
}

/// Checks the *complete* supplied event log, not just the last row for each ID.
/// A locally recomputed digest cannot hide a broken predecessor transition.
pub(in crate::engine::disk_catalog) fn fold(
    history: &DiskHistorySnapshot,
    current: Option<&DiskCatalogSnapshot>,
) -> Result<DiskCatalogSnapshot> {
    validate_header(history)?;
    let covered_current =
        current.filter(|current| current.source_sequence <= history.header.through_sequence);
    let mut compared = false;
    let mut active = BTreeMap::<&str, &DiskCatalogVersion>::new();
    let mut previous = None::<&DiskHistoryRecord>;
    let mut operations = std::collections::BTreeSet::new();
    let mut groups = 0u32;
    for record in &history.changes {
        for hash in [&record.candidate_hash, &record.snapshot_hash] {
            hash.parse::<storage_contract::ContentHash>()
                .map_err(|source| Error::from(std::io::Error::other(source)))?;
        }
        if !compared
            && let Some(current) = covered_current
            && record.sequence > current.source_sequence
        {
            compare_current(&active, current)?;
            compared = true;
        }
        match previous {
            Some(previous) if previous.sequence == record.sequence => {
                if previous.operation_id != record.operation_id
                    || previous.candidate_hash != record.candidate_hash
                    || previous.snapshot_hash != record.snapshot_hash
                {
                    return Err(invalid(
                        "one history sequence claims different publication identities",
                    ));
                }
            }
            _ => {
                if !operations.insert(record.operation_id) {
                    return Err(invalid(
                        "one operation appears at different history sequence slots",
                    ));
                }
                groups += 1; // bounded by MAX_CHANGES during SQLite decode
            }
        }
        let id = record.change.version_id();
        match &record.change {
            DiskHistoryChange::Entered { after } => {
                if active.insert(id, after).is_some() {
                    return Err(invalid("disk history enters an already-present version"));
                }
            }
            DiskHistoryChange::Changed { before, after } => {
                if active.get(id).copied() != Some(before) {
                    return Err(invalid(
                        "disk history change has no exact retained predecessor",
                    ));
                }
                active.insert(id, after);
            }
            DiskHistoryChange::Departed { before } => {
                if active.remove(id) != Some(before) {
                    return Err(invalid(
                        "disk history departure has no exact retained predecessor",
                    ));
                }
            }
        }
        if active.len() > MAX_ROWS {
            return Err(budget());
        }
        previous = Some(record);
    }
    if groups > history.header.publications {
        return Err(invalid(
            "disk history change groups exceed the declared publication count",
        ));
    }
    if let Some(last) = previous
        && last.sequence == history.header.through_sequence
        && history.header.through_snapshot.as_ref() != Some(&last.snapshot_hash)
    {
        return Err(invalid(
            "disk history final change differs from its coverage snapshot",
        ));
    }
    if !compared && let Some(current) = covered_current {
        compare_current(&active, current)?;
    }
    let mut bytes = 0;
    let mut versions = Vec::with_capacity(active.len());
    for row in active.into_values() {
        let length = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?.len();
        if length > MAX_RECORD_BYTES - bytes {
            return Err(budget());
        }
        bytes += length;
        versions.push(row.clone());
    }
    Ok(DiskCatalogSnapshot {
        identity: history.header.identity.clone(),
        source_sequence: history.header.through_sequence,
        versions,
    })
}

fn compare_current(
    active: &BTreeMap<&str, &DiskCatalogVersion>,
    current: &DiskCatalogSnapshot,
) -> Result<()> {
    if !current.versions.iter().eq(active.values().copied()) {
        return Err(invalid(
            "current disk catalog differs from its complete local historical fold",
        ));
    }
    Ok(())
}
