//! Held read and listing projection for an explicitly admitted worktree view.
//!
//! Server authorization, live session fencing and global namespace registration
//! remain the caller's responsibility. This adapter never scans disk names to
//! identify a worktree or exports a physical path/descriptor to SMB.
use super::ledger::{Checkpoint, HeldWorktreeLedger, LedgerError};
use super::lifecycle::{Phase, PublicationScope, Revision, WorktreeId};
use super::projection::{PublishedFileSource, ProjectionError, ReadRoute, SparseProjection};
use super::store::{HeldSparseWorktree, WorktreeStoreError};
use crate::working_tree::snapshot::{
    HeldSnapshotNode, SnapshotEntryContent, SnapshotFile, SnapshotReference, SnapshotStoreError,
};
use data_serialization::Serialize;
use name_policy::{LookupPath, PortablePath};
use std::num::NonZeroU16;

const MAX_PAGE_ENTRIES: u16 = 256;
const MAX_PAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ViewStamp {
    pub(crate) worktree: WorktreeId,
    pub(crate) revision: Revision,
    pub(crate) checkpoint: Checkpoint,
    pub(crate) material: SnapshotReference,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(crate) enum EntryContent {
    Directory,
    File { material: SnapshotFile },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Entry {
    pub(crate) path: PortablePath,
    pub(crate) content: EntryContent,
}
#[derive(Clone)]
pub(crate) struct Cursor {
    stamp: ViewStamp,
    parent: Option<LookupPath>,
    after: LookupPath,
}
pub(crate) struct Page {
    pub(crate) entries: Vec<Entry>,
    pub(crate) next: Option<Cursor>,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum ViewError {
    #[error("worktree view is not active or its saved material needs another read basis")]
    Phase,
    #[error("worktree view differs from the admitted instance or publication")]
    Binding,
    #[error("worktree read projection is invalid")]
    Projection(#[source] ProjectionError),
    #[error("worktree ledger projection is invalid")]
    Ledger(#[source] LedgerError),
    #[error("worktree edit read failed")]
    Worktree(#[source] WorktreeStoreError),
    #[error("worktree immutable snapshot read failed")]
    Snapshot(#[source] SnapshotStoreError),
    #[error("worktree entry was not found in the admitted logical view")]
    Missing,
    #[error("worktree entry is a directory, not file content")]
    IsDirectory,
    #[error("worktree entry is not a directory")]
    NotDirectory,
    #[error("worktree directory cursor belongs to another view or parent")]
    Cursor,
    #[error("worktree directory page exceeds the entry or byte budget")]
    Budget,
    #[error("worktree directory entry encoding failed")]
    Encoding(#[source] storage_contract::Error),
}

enum Backing<'a> {
    Sparse {
        source: &'a HeldSnapshotNode,
        instance: HeldSparseWorktree,
        projection: SparseProjection<'a>,
    },
    Published { node: &'a HeldSnapshotNode },
}
pub(crate) struct HeldReadView<'a> {
    stamp: ViewStamp,
    ledger: &'a HeldWorktreeLedger<'a>,
    backing: Backing<'a>,
}
impl<'a> HeldReadView<'a> {
    /// Initial bytes are read from the original immutable node. Only explicitly
    /// sealed edited occurrences open sparse worktree bodies.
    pub(crate) fn sparse(
        source: &'a HeldSnapshotNode,
        ledger: &'a HeldWorktreeLedger<'a>,
    ) -> Result<Self, ViewError> {
        ledger.admit_read_view().map_err(ViewError::Ledger)?;
        let Phase::Active { saved } = ledger.worktree().phase() else {
            return Err(ViewError::Phase);
        };
        if saved.is_some_and(|saved| matches!(saved.scope, PublicationScope::Directory { .. })) {
            return Err(ViewError::Phase);
        }
        if source.manifest().reference() != ledger.instance().source() {
            return Err(ViewError::Binding);
        }
        let instance = HeldSparseWorktree::reopen(source, ledger.instance())
            .map_err(ViewError::Worktree)?;
        Ok(Self {
            stamp: stamp(ledger, source.manifest().reference()), ledger,
            backing: Backing::Sparse {
                source, instance, projection: ledger.projection().map_err(ViewError::Ledger)?,
            },
        })
    }

    /// A completed directory publication routes to its exact new node. Validate
    /// the full logical view first; never refill the sparse directory for SMB.
    pub(crate) fn published(
        node: &'a HeldSnapshotNode,
        ledger: &'a HeldWorktreeLedger<'a>,
    ) -> Result<Self, ViewError> {
        ledger.admit_read_view().map_err(ViewError::Ledger)?;
        let Phase::Active { saved: Some(saved) } = ledger.worktree().phase() else {
            return Err(ViewError::Phase);
        };
        let PublicationScope::Directory { node: published, .. } = saved.scope else {
            return Err(ViewError::Phase);
        };
        if saved.intent.revision != ledger.worktree().revision()
            || published != node.manifest().node_id()
        {
            return Err(ViewError::Binding);
        }
        let sources = ledger.file_results().iter().map(|result| PublishedFileSource {
            path: result.path.clone(), source: result.source,
        }).collect::<Vec<_>>();
        ledger.projection().map_err(ViewError::Ledger)?
            .published_view(node.manifest(), &sources).map_err(ViewError::Projection)?;
        Ok(Self {
            stamp: stamp(ledger, node.manifest().reference()), ledger,
            backing: Backing::Published { node },
        })
    }

    pub(crate) fn stamp(&self) -> ViewStamp {
        self.stamp
    }
    pub(crate) fn open_file(&self, path: &LookupPath) -> Result<crate::ReadFile, ViewError> {
        self.ledger.admit_read_view().map_err(ViewError::Ledger)?;
        match &self.backing {
            Backing::Sparse { source, instance, projection } => {
                match projection.route(path).ok_or(ViewError::Missing)? {
                    ReadRoute::Directory => Err(ViewError::IsDirectory),
                    ReadRoute::Original { file } => {
                        source.open_payload(file.version_id).map_err(ViewError::Snapshot)
                    }
                    ReadRoute::Working { material } => {
                        instance.open_material(material).map_err(ViewError::Worktree)
                    }
                }
            }
            Backing::Published { node } => match self.admitted_content(path)? {
                EntryContent::Directory => Err(ViewError::IsDirectory),
                EntryContent::File { material } => {
                    node.open_payload(material.version_id).map_err(ViewError::Snapshot)
                }
            },
        }
    }
    pub(crate) fn content(&self, path: &LookupPath) -> Result<EntryContent, ViewError> {
        self.ledger.admit_read_view().map_err(ViewError::Ledger)?;
        self.admitted_content(path)
    }
    fn admitted_content(&self, path: &LookupPath) -> Result<EntryContent, ViewError> {
        match &self.backing {
            Backing::Sparse { projection, .. } => {
                Ok(match projection.route(path).ok_or(ViewError::Missing)? {
                    ReadRoute::Directory => EntryContent::Directory,
                    ReadRoute::Original { file } => EntryContent::File { material: file },
                    ReadRoute::Working { material } => {
                        EntryContent::File { material: material.file }
                    }
                })
            }
            Backing::Published { node } => {
                let entries = node.manifest().entries();
                let index = entries.binary_search_by(|entry| entry.path.key().cmp(path.key()))
                    .map_err(|_| ViewError::Missing)?;
                Ok(match entries[index].content {
                    SnapshotEntryContent::Directory {} => EntryContent::Directory,
                    SnapshotEntryContent::File { material, .. } => EntryContent::File { material },
                })
            }
        }
    }

    pub(crate) fn directory(
        &self,
        parent: Option<&LookupPath>,
        cursor: Option<&Cursor>,
        limit: NonZeroU16,
    ) -> Result<Page, ViewError> {
        self.ledger.admit_read_view().map_err(ViewError::Ledger)?;
        if limit.get() > MAX_PAGE_ENTRIES {
            return Err(ViewError::Budget);
        }
        if let Some(parent) = parent {
            if !matches!(self.admitted_content(parent)?, EntryContent::Directory) {
                return Err(ViewError::NotDirectory);
            }
        }
        if let Some(cursor) = cursor {
            if cursor.stamp != self.stamp || cursor.parent.as_ref() != parent {
                return Err(ViewError::Cursor);
            }
        }
        match &self.backing {
            Backing::Sparse { projection, .. } => {
                self.page(parent, cursor, limit, projection.paths())
            }
            Backing::Published { node } => {
                let paths = node.manifest().entries().iter().map(|entry| &entry.path);
                self.page(parent, cursor, limit, paths)
            }
        }
    }

    fn page<'p>(
        &self,
        parent: Option<&LookupPath>,
        cursor: Option<&Cursor>,
        limit: NonZeroU16,
        paths: impl Iterator<Item = &'p PortablePath>,
    ) -> Result<Page, ViewError> {
        let mut selected = Vec::new();
        let mut bytes = 2usize;
        for path in paths {
            if path.parent().as_ref().map(|path| path.key()) != parent.map(|path| path.key())
                || cursor.is_some_and(|cursor| path.key() <= cursor.after.key())
            {
                continue;
            }
            let item = Entry {
                path: path.clone(), content: self.admitted_content(&LookupPath::from(path))?,
            };
            let encoded = storage_contract::canonical_json(&item).map_err(ViewError::Encoding)?;
            let next_bytes = bytes.checked_add(encoded.len()).and_then(|total| total.checked_add(1))
                .ok_or(ViewError::Budget)?;
            if selected.len() == usize::from(limit.get()) || next_bytes > MAX_PAGE_BYTES {
                let last: &Entry = selected.last().ok_or(ViewError::Budget)?;
                return Ok(Page {
                    next: Some(Cursor {
                        stamp: self.stamp,
                        parent: parent.cloned(),
                        after: LookupPath::from(&last.path),
                    }),
                    entries: selected,
                });
            }
            bytes = next_bytes;
            selected.push(item);
        }
        Ok(Page { entries: selected, next: None })
    }
}
fn stamp(ledger: &HeldWorktreeLedger<'_>, material: SnapshotReference) -> ViewStamp {
    ViewStamp {
        worktree: ledger.worktree().id(), revision: ledger.worktree().revision(),
        checkpoint: ledger.checkpoint(), material,
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/io.rs"]
mod tests;
