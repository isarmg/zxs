//! Structural admission of the ordinary registered disk roots. This is not a
//! decision, a check of the configured topology, or permission to open a path.
use platform_io::fs_safety::linux::FileIdentity;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{DiskId, PoolId};
use storage_contract::namespace_root::{
    DirectoryId, LocalEntityIdentity, RootBinding, RootId, StorageLayout,
};

const MAX_ROOTS: usize = 256;
pub(crate) const MAX_SOURCE_CHAIN: usize = 32;
pub(crate) const MAX_DIRECTORY_DEPTH: usize = 64;

pub(crate) fn local_entity(identity: FileIdentity) -> LocalEntityIdentity {
    LocalEntityIdentity::new(identity.device, identity.inode)
}
pub(crate) fn file_identity(identity: LocalEntityIdentity) -> FileIdentity {
    FileIdentity {
        device: identity.device(),
        inode: identity.inode(),
    }
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum Error {
    #[error("too many registered roots or upstream registrations")]
    Budget,
    #[error("registered root has the wrong storage layout")]
    Format,
    #[error("registered roots do not belong to one pool")]
    Registration,
    #[error("invalid pool or disk registration identifier")]
    Identifier(#[source] storage_contract::Error),
    #[error("invalid disk slot in root registration")]
    Slot(#[source] name_policy::Error),
    #[error("root does not retain its immediate upstream's exact registration and source chain")]
    SourceChain,
    #[error("registered root, disk, slot or area identity is repeated")]
    DuplicateIdentity,
    #[error("registered root areas alias another entity or are not on the original device")]
    PhysicalIdentity,
}

/// Includes every management and ordinary area. A caller with additional
/// records must disallow cross-kind aliases against these same sets.
pub(crate) struct Structure<'a> {
    pub(crate) by_id: BTreeMap<RootId, &'a RootBinding>,
    pub(crate) entities: BTreeSet<LocalEntityIdentity>,
    pub(crate) area_ids: BTreeSet<DirectoryId>,
}

/// Inspect the actual bindings, independent of collection order. Once the
/// shape checks succeed, the strings are fixed IDs or <=64-byte ASCII slots.
/// Even budgeting 4 KiB per root plus 512 bytes per registration, 256 roots
/// with at most 32 ancestors each are under 6 MiB; no second serialized
/// inventory is needed to bound these rows.
pub(crate) fn inspect<'a>(
    layout: StorageLayout,
    bindings: impl ExactSizeIterator<Item = &'a RootBinding>,
) -> Result<Structure<'a>, Error> {
    if bindings.len() > MAX_ROOTS {
        return Err(Error::Budget);
    }
    let mut by_id = BTreeMap::new();
    let mut by_disk = BTreeMap::new();
    let mut slots = BTreeSet::new();
    let mut entities = BTreeSet::new();
    let mut area_ids = BTreeSet::new();
    let mut pool = None;
    for root in bindings {
        if root.layout != layout {
            return Err(Error::Format);
        }
        if root.source_chain.len() > MAX_SOURCE_CHAIN {
            return Err(Error::Budget);
        }
        let registration = &root.registration;
        let pool_id = registration.pool_id.parse::<PoolId>().map_err(Error::Identifier)?;
        let disk_id = registration.disk_id.parse::<DiskId>().map_err(Error::Identifier)?;
        let slot = name_policy::SlotId::parse(&registration.slot_id).map_err(Error::Slot)?;
        if pool.replace(pool_id).is_some_and(|prior| prior != pool_id) {
            return Err(Error::Registration);
        }
        if by_id.insert(root.root_id, root).is_some()
            || by_disk.insert(disk_id, root).is_some()
            || !slots.insert(slot)
        {
            return Err(Error::DuplicateIdentity);
        }
        if !entities.insert(root.root_entity) {
            return Err(Error::PhysicalIdentity);
        }
        for area in [
            &root.roots.common_data,
            &root.roots.deleted_data,
            &root.roots.zxs_data,
            &root.roots.zxs_lifecycles.common_data,
            &root.roots.zxs_lifecycles.deleted_data,
        ] {
            if !area_ids.insert(area.directory_id) {
                return Err(Error::DuplicateIdentity);
            }
            if area.entity.inode() == 0 || area.entity.device() != root.root_entity.device()
                || !entities.insert(area.entity)
            {
                return Err(Error::PhysicalIdentity);
            }
        }
    }
    for root in by_id.values() {
        if let Some((first, rest)) = root.source_chain.split_first() {
            let disk_id = first.disk_id.parse::<DiskId>().map_err(Error::Identifier)?;
            let upstream = by_disk.get(&disk_id).ok_or(Error::SourceChain)?;
            // Equality includes slot, pool, and binding generation. Every link
            // consumes exactly one registration, so a missing intermediate,
            // repeated disk, truncated tail or cycle cannot terminate at a primary.
            if upstream.registration != *first || upstream.source_chain.as_slice() != rest {
                return Err(Error::SourceChain);
            }
        }
    }
    Ok(Structure { by_id, entities, area_ids })
}

#[cfg(test)]
#[path = "../../../test/engine/unit/namespace_roots_tests.rs"]
mod tests;
