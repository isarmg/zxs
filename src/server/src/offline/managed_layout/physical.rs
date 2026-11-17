//! A role name or different mount ID is not a different storage directory.
//! This deny-only check supplements, never replaces, the native mount-overlap,
//! stopped-root, original selection and freshness checks.
use super::{BTreeSet, File, FileIdentity, OpenAt2Root, Result};

#[derive(Debug, PartialEq, Eq, error_definition::Error)]
enum AliasedRoot {
    #[error("two fixed helper control roles refer to the same directory; no role was changed")]
    Control,
    #[error("a selected disk root refers to a fixed helper control directory; no root was adopted")]
    StorageControl,
    #[error("two selected disk roots refer to the same directory; no root was adopted")]
    Storage,
}

pub(super) struct DistinctRoots {
    control: [FileIdentity; 4],
    // Only explicitly required active/selected entries observed during this
    // inventory. Historical stopped paths must not be opened to populate it.
    storage: BTreeSet<(u64, u64)>,
}

impl DistinctRoots {
    pub(super) fn from_held(control: [&OpenAt2Root; 4]) -> Result<Self> {
        let control = control.map(OpenAt2Root::identity);
        for (index, identity) in control.iter().enumerate() {
            if control[..index].contains(identity) { return Err(AliasedRoot::Control.into()); }
        }
        Ok(Self { control, storage: BTreeSet::new() })
    }

    pub(super) fn storage(&mut self, directory: &File) -> Result<()> {
        let identity = FileIdentity::from_fd(directory)?;
        if self.control.contains(&identity) { return Err(AliasedRoot::StorageControl.into()); }
        if !self.storage.insert((identity.device, identity.inode)) {
            return Err(AliasedRoot::Storage.into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/managed_layout_physical.rs"]
mod tests;
