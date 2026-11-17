//! The tail may fill only missing common receipts. Earlier sightings belong
//! to the exact participant, including SYSTEM and each original root, even
//! though target writes and original writes proceed in a different order.
use super::{COMPLETE, DiskRoot, Result, invalid, require_completion};
use crate::engine::backup_preparation::capacity::Census;
use crate::history_package::{install_preparation_bytes, sync_existing_preparation_bytes};
use platform_io::fs_safety::linux::FileIdentity;
use std::{collections::{BTreeMap, btree_map::Entry}, os::unix::fs::MetadataExt};

#[derive(Default)]
pub(super) struct Completions {
    // In-memory continuity only. A cold invocation must re-admit all actual
    // material and content; it cannot infer a historical inode from this map.
    roots: BTreeMap<(u64, u64), Option<FileIdentity>>,
}
fn identity(root: &DiskRoot) -> Result<(u64, u64)> {
    let id = root.directory_identity(root.path())?;
    Ok((id.device, id.inode))
}
fn existing(root: &DiskRoot) -> Result<Option<FileIdentity>> {
    let Some(metadata) = root.optional_metadata(&root.path().join(COMPLETE))? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("ordinary Backup set completion is not one regular participant record"));
    }
    Ok(Some(FileIdentity::from_metadata(&metadata)))
}
fn require_named(root: &DiskRoot, expected: FileIdentity) -> Result<()> {
    if existing(root)? != Some(expected) {
        return Err(invalid("previously observed ordinary Backup set completion changed or disappeared"));
    }
    Ok(())
}
impl Completions {
    pub(super) fn observe(&mut self, root: &DiskRoot) -> Result<()> {
        match self.roots.entry(identity(root)?) {
            Entry::Vacant(slot) => { slot.insert(existing(root)?); Ok(()) }
            Entry::Occupied(_) => Err(invalid("ordinary Backup completion participant root occurs more than once")),
        }
    }
    pub(super) fn any(&self) -> bool {
        self.roots.values().any(Option::is_some)
    }

    pub(super) fn review(&mut self, root: &DiskRoot, bytes: &[u8]) -> Result<()> {
        let seen = self.roots.get_mut(&identity(root)?)
            .ok_or_else(|| invalid("ordinary Backup completion participant changed"))?;
        let mut now = existing(root)?;
        if seen.is_some_and(|original| now != Some(original)) {
            return Err(invalid("previously observed ordinary Backup set completion changed or disappeared"));
        }
        if now.is_none() {
            // The common record's actual bytes are now known. Only a
            // missing record needs a new inode or rounded allocation.
            let mut census = Census::new(root)?;
            if census.file_present(COMPLETE.into(), bytes.len() as u64)? {
                now = Some(existing(root)?.ok_or_else(|| invalid("ordinary Backup set completion changed during capacity check"))?);
            } else {
                census.require_with_reserve(0)?;
            }
        }
        if let Some(id) = now {
            require_completion(root, bytes)?;
            require_named(root, id)?;
        }
        *seen = now;
        Ok(())
    }

    pub(super) fn finish(&mut self, root: &DiskRoot, bytes: &[u8]) -> Result<()> {
        let path = root.path().join(COMPLETE);
        let seen = self.roots.get_mut(&identity(root)?)
            .ok_or_else(|| invalid("ordinary Backup completion participant changed"))?;
        if let Some(original) = *seen {
            require_named(root, original)?;
            sync_existing_preparation_bytes(root, &path, bytes)?;
            require_named(root, original)?;
        } else {
            install_preparation_bytes(root, &path, bytes)?;
            *seen = Some(existing(root)?.ok_or_else(|| invalid("new ordinary Backup set completion disappeared"))?);
        }
        Ok(())
    }

    pub(super) fn verify(&self, root: &DiskRoot, bytes: &[u8]) -> Result<()> {
        let expected = self.roots.get(&identity(root)?).copied().flatten()
            .ok_or_else(|| invalid("ordinary Backup participant has no observed completion"))?;
        require_named(root, expected)?;
        require_completion(root, bytes)?;
        require_named(root, expected)
    }
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_ordinary_tail_tests.rs"]
mod tests;
