//! Known-entry cleanup of a single unfinished recycle copy.
use super::invalid;
use crate::{container_store::stage::SealedStage, fs::HeldDir};
use std::io;

pub(in crate::container_store) struct CopyTree {
    parent: HeldDir,
    directory: HeldDir,
    entries: Vec<SealedStage>,
}

impl CopyTree {
    pub(in crate::container_store) fn new(parent: &HeldDir) -> io::Result<Self> {
        let parent = HeldDir {
            file: parent.file.try_clone()?,
            path: parent.path.clone(),
            entity: parent.entity,
        };
        let directory = parent.new_child_directory(c"container")?;
        Ok(Self {
            parent,
            directory,
            entries: Vec::new(),
        })
    }

    pub(super) fn directory(&self) -> &HeldDir {
        &self.directory
    }

    pub(in crate::container_store) fn require_named(&self) -> io::Result<()> {
        let named = self
            .parent
            .child_directory(c"container")?
            .ok_or_else(|| invalid("recycle copy directory lost its name"))?;
        if named.entity != self.directory.entity || !named.same_mount(&self.directory)? {
            return Err(invalid("recycle copy directory differs from the original"));
        }
        Ok(())
    }

    pub(super) fn reserve_entry(&mut self) -> io::Result<()> {
        self.require_named()?;
        self.entries
            .try_reserve(1)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))
    }

    pub(super) fn record_entry(&mut self, entry: SealedStage) {
        self.entries.push(entry);
    }
}

impl Drop for CopyTree {
    fn drop(&mut self) {
        // After publication the original stage parent name has moved; never
        // clean the arrived copy, even if a later sync or confirmation fails.
        for entry in self.entries.iter().rev() {
            if self.require_named().is_err() {
                return;
            }
            let _ = entry.clean(&self.directory);
        }
        let _ = self
            .parent
            .unlink_empty_opaque_original(c"container", &self.directory);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/recycle_staged_tree.rs"]
mod tests;
