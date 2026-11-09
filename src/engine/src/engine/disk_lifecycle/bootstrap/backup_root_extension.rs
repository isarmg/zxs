//! Backup-only full-root probes. A locator limits names and held identities;
//! native Backup archives, journals and publications are verified by startup.
use super::*;
use crate::{DiskConfig, Error, ErrorCode, ValidatedConfig, disk::DiskRoot};
use runtime::management_error;

/// Inspect the whole fixed set, never a live subset or a replacement Config
/// address. Backup cannot use this to erase stops or acquire readmission epochs.
pub(in crate::engine) fn open_fixed_roots(
    locator: &LoadedLocator,
    config: &ValidatedConfig,
) -> crate::Result<Vec<(DiskConfig, DiskRoot)>> {
    if locator.record.pool_id != config.pool_id
        || locator.record.roots.len() != config.disks.len()
        || !locator.record.stops.is_empty()
        || !locator.record.readmissions.is_empty()
        || !locator.record.recovered_readmissions.is_empty()
        || locator
            .record
            .retained
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            != config
                .metadata_members()
                .map(|disk| &disk.disk_id)
                .collect()
    {
        return Err(closed(
            "Backup probes change the fixed roots, original voters or stopped epochs",
        ));
    }
    // Finish every address/binding check before the first storage stat.
    let records = config
        .disks
        .iter()
        .map(|disk| {
            let record = locator
                .record
                .roots
                .iter()
                .find(|record| record.disk_id == disk.disk_id)
                .ok_or_else(|| closed("Backup root is absent from its exact locator"))?;
            if record.slot_id != disk.slot_id
                || record.binding_generation != disk.binding_generation
                || Path::new(&record.path) != disk.root
                || record.metadata_member != disk.metadata_member
            {
                return Err(closed(
                    "Backup projection attempts to relocate or rebind a recorded root",
                ));
            }
            Ok((disk, record))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let mut held = Vec::with_capacity(records.len());
    let mut identities = std::collections::BTreeSet::new();
    for (disk, record) in records {
        let named = std::fs::symlink_metadata(&record.path)?;
        let identity = FileIdentity::from_metadata(&named);
        if !named.is_dir()
            || identity.device != record.device
            || identity.inode != record.inode
            || !identities.insert((identity.device, identity.inode))
        {
            return Err(closed(
                "Backup root no longer has its exact distinct recorded identity",
            ));
        }
        let root = DiskRoot::open(Path::new(&record.path))?;
        let actual = FileIdentity::from_fd(&root.directory(root.path())?)?;
        if actual != identity {
            return Err(closed(
                "Backup root changed while obtaining its held descriptor",
            ));
        }
        held.push((disk.clone(), root));
    }
    Ok(held)
}

pub(in crate::engine) fn observe_fixed_roots(
    config: &ValidatedConfig,
    roots: &[(DiskConfig, DiskRoot)],
) -> crate::Result<Vec<RootObservation>> {
    if roots.len() != config.disks.len() {
        return Err(closed(
            "Backup expansion requires every original and selected root",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    roots
        .iter()
        .map(|(disk, root)| {
            if !config.disks.iter().any(|expected| {
                expected.disk_id == disk.disk_id
                    && expected.slot_id == disk.slot_id
                    && expected.binding_generation == disk.binding_generation
                    && expected.root == disk.root
                    && expected.metadata_member == disk.metadata_member
            }) || root.path() != disk.root
                || !seen.insert(&disk.disk_id)
            {
                return Err(closed("Backup expansion contains another root binding"));
            }
            RootObservation::capture(
                &DiskRegistrationIdentity {
                    pool_id: config.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                root.path(),
                &root.directory(root.path())?,
                disk.metadata_member,
            )
            .map_err(management_error)
        })
        .collect()
}
fn closed(message: &'static str) -> Error {
    Error::new(ErrorCode::RecoveryRequired, message)
}
