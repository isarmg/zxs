//! Temporary evidence from opening a configured root. Descendant I/O needs
//! only the held directory, not a copy of its enrollment path history.
use super::{Entity, HeldDir, check_path, child, entity, invalid, local_uid};
use std::{
    ffi::CString,
    fs::File,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
};

pub(crate) struct TrustedRoot {
    pub(crate) held: HeldDir,
    ancestors: Vec<Entity>,
}

impl TrustedRoot {
    /// A shared ordinary ancestor is fine. It is a selected root itself
    /// appearing on the other root's visible walk that makes them overlap.
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.held.path.starts_with(&other.held.path)
            || other.held.path.starts_with(&self.held.path)
            || self.held.entity == other.held.entity
            || self.ancestors.contains(&other.held.entity)
            || other.ancestors.contains(&self.held.entity)
    }

    /// Each canonical path component must resist untrusted rename, not
    /// just the selected directory.
    /// A trusted-owned sticky ancestor (e.g. /tmp) protects its existing,
    /// trusted-owned child. Symlink traversal is never allowed.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        check_path(path)?;
        let uid = local_uid();
        if uid == 0 || uid == u32::MAX {
            return Err(invalid("storage requires a real non-root service account"));
        }
        let mut ancestors = Vec::new();
        let mut file = File::open("/")?;
        for component in path.components() {
            if let Component::Normal(name) = component {
                let p = file.metadata()?;
                if !(p.uid() == 0 || p.uid() == uid)
                    || (p.mode() & 0o022 != 0 && p.mode() & 0o1000 == 0)
                {
                    return Err(invalid("untrusted storage ancestor"));
                }
                // Use the metadata already observed on this opened component.
                ancestors.push(Entity {
                    device: p.dev(),
                    inode: p.ino(),
                });
                file = child(
                    &file,
                    &CString::new(name.as_bytes())
                        .map_err(|_| invalid("invalid storage component"))?,
                )?;
            }
        }
        let m = file.metadata()?;
        if m.uid() != uid || m.mode() & 0o7777 != 0o700 {
            return Err(invalid(
                "selected storage requires its exact private directory",
            ));
        }
        Ok(Self {
            held: HeldDir {
                entity: entity(&file)?,
                file,
                path: path.to_owned(),
            },
            ancestors,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/trusted_roles.rs"]
mod tests;
