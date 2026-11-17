//! Complete future serving inventory for one captured target. Distinct held
//! FILE containers are separate scopes; the history metadata root is shared.
//! Never use the planned path or its type as evidence that it exists.
use super::{AtomicBool, Failure, FileId, Key, MAX_ENTITIES, Result, layout, preparation_checkpoint};
use std::{collections::{BTreeMap, btree_map::Entry}, path::{Path, PathBuf}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind { Directory, File }
pub(super) type Paths = BTreeMap<PathBuf, Kind>;

pub(super) struct Expected {
    pub(super) management: Paths,
    pub(super) files: BTreeMap<FileId, Paths>,
    entries: usize,
}
impl Expected {
    pub(super) fn new<T>(files: &BTreeMap<FileId, T>, stop: &AtomicBool) -> Result<Self> {
        let mut value = Self { management: Paths::new(), files: BTreeMap::new(), entries: 0 };
        for &file in files.keys() {
            preparation_checkpoint(stop)?;
            let mut paths = Paths::new();
            include(&mut paths, &mut value.entries, layout::VERSIONS.as_path(), Kind::Directory)?;
            value.files.insert(file, paths);
        }
        Ok(value)
    }

    pub(super) fn add(&mut self, key: &Key, relative: &Path) -> Result<()> {
        let paths = match key {
            Key::Metadata { .. } => &mut self.management,
            Key::Owner { file } | Key::Database { file } | Key::Body { file, .. } => {
                self.files.get_mut(file).ok_or(Failure::BackupLayoutEvidence)?
            }
        };
        for parent in relative.ancestors().skip(1).filter(|p| !p.as_os_str().is_empty()) {
            include(paths, &mut self.entries, parent, Kind::Directory)?;
        }
        include(paths, &mut self.entries, relative, Kind::File)
    }

    pub(super) fn require_room_for(&self, retained_temporaries: usize) -> Result<()> {
        if retained_temporaries > MAX_ENTITIES - self.entries {
            return Err(Failure::ContainerBudget.into());
        }
        Ok(())
    }
}

fn include(paths: &mut Paths, entries: &mut usize, path: &Path, kind: Kind) -> Result<()> {
    match paths.entry(path.to_owned()) {
        // Only a directory prefix may be shared by multiple expected files.
        Entry::Occupied(previous) if kind == Kind::Directory && *previous.get() == kind => Ok(()),
        Entry::Occupied(_) => Err(Failure::BackupLayoutEvidence.into()),
        Entry::Vacant(entry) => {
            if *entries == MAX_ENTITIES {
                return Err(Failure::ContainerBudget.into());
            }
            entry.insert(kind);
            *entries += 1;
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/container_storage/backup_serving_paths_tests.rs"]
mod tests;
