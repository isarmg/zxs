//! Logical namespace edits share the sparse projection used by reads and plans.
//!
//! The ledger does not persist these operations yet. File additions wait for an
//! independently admitted import result; this module cannot invent a container.
use crate::directory_worktree::projection::SparseProjection;
use crate::working_tree::snapshot::{
    SnapshotEntry, SnapshotEntryContent, SnapshotFile, SnapshotFileSource, SnapshotManifestError,
};
use name_policy::{LookupPath, PortablePath};

// Bound per-request path rebasing and sorting independently from the snapshot
// inventory bound. At most 32 new directories supplement the admitted source;
// final admission enforces the snapshot's entry, path-byte and payload budgets.
const MAX_CHANGES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceChange {
    AddDirectory { path: PortablePath },
    RemoveSubtree { path: LookupPath },
    Rename { source: LookupPath, target: PortablePath },
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum NamespaceError {
    #[error("worktree namespace batch exceeds its operation budget")]
    Budget,
    #[error("worktree namespace source does not exist")]
    Missing,
    #[error("worktree namespace destination is already occupied")]
    Occupied,
    #[error("worktree directory cannot be moved into its own descendant")]
    Descendant,
    #[error("worktree renamed path exceeds naming limits")]
    Path(#[source] name_policy::Error),
    #[error("worktree complete namespace or material violates snapshot admission")]
    Material(#[source] SnapshotManifestError),
}

#[derive(Clone, Copy)]
pub(super) enum Content {
    Directory,
    File { original: usize, material: SnapshotFile, source: SnapshotFileSource },
}

pub(super) struct Entry {
    pub(super) path: PortablePath,
    pub(super) content: Content,
}

impl<'a> SparseProjection<'a> {
    /// Apply one ordered atomic batch to the same view consumed by route, plan
    /// and publication checks. Removed sparse bodies stay owned by this view;
    /// logical deletion never authorizes physical cleanup or FILE publication.
    pub(crate) fn namespace(mut self, changes: &[NamespaceChange]) -> Result<Self, NamespaceError> {
        if changes.len() > MAX_CHANGES {
            return Err(NamespaceError::Budget);
        }
        for change in changes {
            apply(&mut self.entries, change)?;
        }
        let material = self.entries.iter().map(|entry| SnapshotEntry {
            path: entry.path.clone(),
            content: match entry.content {
                Content::Directory => SnapshotEntryContent::Directory {},
                Content::File { original, material, source } => SnapshotEntryContent::File {
                    material: match self.edits.get(&original) {
                        Some(edited) => edited.file,
                        None => material,
                    },
                    source,
                },
            },
        }).collect();
        self.source.validate_material_entries(material).map_err(NamespaceError::Material)?;
        Ok(self)
    }
}

fn position(entries: &[Entry], path: &LookupPath) -> Result<usize, NamespaceError> {
    entries.binary_search_by(|entry| entry.path.key().cmp(path.key()))
        .map_err(|_| NamespaceError::Missing)
}

fn apply(entries: &mut Vec<Entry>, change: &NamespaceChange) -> Result<(), NamespaceError> {
    match change {
        NamespaceChange::AddDirectory { path } => {
            match entries.binary_search_by(|entry| entry.path.key().cmp(path.key())) {
                Ok(_) => Err(NamespaceError::Occupied),
                Err(index) => {
                    entries.insert(index, Entry {
                        path: path.clone(), content: Content::Directory,
                    });
                    Ok(())
                }
            }
        }
        NamespaceChange::RemoveSubtree { path } => {
            position(entries, path)?;
            entries.retain(|entry| !within(&entry.path, path));
            Ok(())
        }
        NamespaceChange::Rename { source, target } => rename(entries, source, target),
    }
}

fn rename(entries: &mut [Entry], source: &LookupPath, target: &PortablePath)
    -> Result<(), NamespaceError>
{
    let index = position(entries, source)?;
    if source.key() != target.key() {
        if within(target, source) {
            return Err(NamespaceError::Descendant);
        }
        if entries.binary_search_by(|entry| entry.path.key().cmp(target.key())).is_ok() {
            return Err(NamespaceError::Occupied);
        }
    }
    let depth = entries[index].path.as_str().split('/').count();
    for entry in entries.iter_mut() {
        if entry.path.key() == source.key() {
            entry.path = target.clone();
        } else if within(&entry.path, source) {
            entry.path = entry.path.rebase(depth, target).map_err(NamespaceError::Path)?;
        }
    }
    entries.sort_by(|left, right| left.path.key().cmp(right.path.key()));
    if entries.windows(2).any(|pair| pair[0].path.key() == pair[1].path.key()) {
        return Err(NamespaceError::Occupied);
    }
    Ok(())
}

fn within(path: &PortablePath, parent: &LookupPath) -> bool {
    path.key() == parent.key()
        || path.key().strip_prefix(parent.key()).is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/directory_worktree/namespace.rs"]
mod tests;
