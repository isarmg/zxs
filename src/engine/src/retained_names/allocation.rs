//! Fresh-name selection through the actual held retained parent. The selected
//! leaf is frozen before native publication; retries never invoke this allocator.
use super::{Result, RetainedDiskRegistry, RetainedNameError, utc_stamp};
use crate::disk::DiskRoot;
use crate::name_budget::NameBudget;
use name_policy::Name;
use std::collections::BTreeSet;
use storage_contract::file_container::retained::{RetainedAllocation, RetainedKind, RetainedName};
use storage_contract::{DiskId, OperationId, Timestamp};

pub(crate) fn allocate_file<'a>(
    registry: &RetainedDiskRegistry,
    parent: &DiskRoot,
    disk: DiskId,
    original: OperationId,
    name: &Name,
    reserved: impl IntoIterator<Item = &'a RetainedName>,
    maximum_entries: usize,
) -> Result<RetainedAllocation> {
    let registration = registry.registration(disk, original)?;
    let maximum = parent
        .component_name_maximum()
        .map_err(|source| RetainedNameError::Target {
            source: Box::new(source),
        })?;
    NameBudget::from_maximum(maximum)
        .map_err(|source| RetainedNameError::Budget { source })?
        .component(name.as_str(), name.as_str())
        .map_err(|source| RetainedNameError::Budget { source })?;
    let mut occupied = BTreeSet::new();
    let entries = parent
        .entries(parent.path())
        .map_err(|source| RetainedNameError::Target {
            source: Box::new(source),
        })?;
    for (count, entry) in entries.enumerate() {
        if count == maximum_entries {
            return Err(RetainedNameError::InventoryBudget);
        }
        let entry = entry.map_err(|source| RetainedNameError::Entry { source })?;
        let display = entry
            .as_os_str()
            .to_str()
            .ok_or(RetainedNameError::NonUtf8)?;
        occupied.insert(name_policy::lookup_key(display));
    }
    for (count, name) in reserved.into_iter().enumerate() {
        if count == maximum_entries {
            return Err(RetainedNameError::InventoryBudget);
        }
        occupied.insert(name_policy::lookup_key(name.as_str()));
        if occupied.len() > maximum_entries {
            return Err(RetainedNameError::InventoryBudget);
        }
    }
    let created_at = Timestamp::from_unix_seconds(crate::business::now().map_err(|source| {
        RetainedNameError::Target {
            source: Box::new(source),
        }
    })?);
    let stamp = utc_stamp(created_at)?;
    for _ in 0..64 {
        let candidate = RetainedName::sample(registration.number, RetainedKind::File, &stamp, name)
            .map_err(|source| RetainedNameError::Name { source })?;
        if !occupied.contains(&name_policy::lookup_key(candidate.as_str())) {
            return Ok(RetainedAllocation {
                registration,
                created_at,
                name: candidate,
            });
        }
    }
    Err(RetainedNameError::NamesExhausted)
}
