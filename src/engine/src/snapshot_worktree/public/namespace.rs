//! Read-only logical mounts and bounded pages from independently completed
//! worktree receipts. No physical path or fabricated ordinary EntryId is exposed.
use crate::directory_worktree::io::{
    Cursor, Entry, EntryContent, HeldReadView, ViewError, ViewStamp,
};
use crate::directory_worktree::session::ReadMaterial;
use crate::snapshot_worktree::{RegistrationError, SnapshotWorktreeError};
use crate::working_tree::snapshot::{SnapshotEntryContent, SnapshotFileSource};
use crate::{Engine, NativeCurrentEntry};
use name_policy::{LookupName, LookupPath, PortablePath};
use std::num::NonZeroU16;
use storage_contract::snapshot_worktree::{
    NamespaceDirectoryId, NamespaceHostAddress, WorktreeHandle, WorktreeRevision, WorktreeStatus,
};
use storage_contract::{ContentHash, FileId, RepositoryId, VersionId};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;
use storage_contract::file_draft::UnbornWorking;

type Result<T> = std::result::Result<T, SnapshotWorktreeError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeEntryContent {
    RetainedFile {
        target: WorktreeFileTarget,
        file: FileId,
        version: VersionId,
        size: u64,
        content_hash: ContentHash,
    },
    Working { target: WorktreeFileTarget, working: UnbornWorking },
    Directory,
    File {
        file: FileId,
        version: VersionId,
        size: u64,
        content_hash: ContentHash,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeEntry {
    /// Issuing occurrence for I/O and idempotent writes. Only the SMB-specific
    /// queries additionally carry the exact node's projected public pathname.
    pub smb_path: Option<PortablePath>,
    pub path: PortablePath,
    pub content: WorktreeEntryContent,
}

/// The consumer can retain or clone this cursor, but cannot manufacture a
/// different checkpoint, parent, original material or position within it.
#[derive(Clone)]
pub struct WorktreeCursor {
    inner: Cursor,
}

/// An unopened cached page keeps its entire admitted view, including the
/// publication checkpoint/material that can change without a revision change.
#[derive(Clone)]
pub struct WorktreePageStamp {
    view: ViewStamp,
    parent: Option<LookupPath>,
}

pub struct WorktreePage {
    pub stamp: WorktreePageStamp,
    pub revision: WorktreeRevision,
    pub entries: Vec<WorktreeEntry>,
    pub next: Option<WorktreeCursor>,
}

impl From<Entry> for WorktreeEntry {
    fn from(entry: Entry) -> Self {
        let content = match entry.content {
            EntryContent::Directory => WorktreeEntryContent::Directory,
            EntryContent::Working { target, working } => {
                WorktreeEntryContent::Working { target, working }
            }
            EntryContent::RetainedFile { target, material } => {
                WorktreeEntryContent::RetainedFile {
                    target,
                    file: material.file_id,
                    version: material.version_id,
                    size: material.size,
                    content_hash: material.content_hash,
                }
            }
            EntryContent::File { material } => WorktreeEntryContent::File {
                file: material.file_id,
                version: material.version_id,
                size: material.size,
                content_hash: material.content_hash,
            },
        };
        Self {
            path: entry.path, smb_path:None,
            content,
        }
    }
}

pub(super) fn with_view<T>(
    engine: &Engine,
    target: WorktreeHandle,
    read: impl FnOnce(&HeldReadView<'_>) -> Result<T>,
) -> Result<T> {
    let index = engine.snapshot_receipts()?;
    let registered = index.worktree(target)?;
    let original = index.reopen_node(engine, registered.source().handle())?;
    let session = registered.reopen(&original)?;
    let published = registered
        .published_node()
        .map(|handle| index.reopen_node(engine, handle))
        .transpose()?;
    let material = match &published {
        Some(node) => ReadMaterial::Published(node),
        None => ReadMaterial::Original,
    };
    let view = session
        .read(material)
        .map_err(RegistrationError::ReadView)?;
    read(&view)
}

impl Engine {
    fn worktree_mounts(
        &self,
        repository: RepositoryId,
        name: Option<&LookupName>,
        maximum: usize,
    ) -> Result<Vec<WorktreeStatus>> {
        let index = self.snapshot_receipts()?;
        // Ordinary SMB paths also ask this projection. No original reservation
        // means no mount, not a demand to initialize a snapshot or physical root.
        if !index.has_mount(repository, name) {
            return Ok(Vec::new());
        }
        let context = self.snapshot_root_context(repository)?;
        let namespace = NamespaceHostAddress {
            repository_id: repository,
            branch_id: context.source.branch_id,
            directory: NamespaceDirectoryId::Repository(repository),
        };
        let root = self
            .namespace_host_root(repository, namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        let handles =
            index.mount_handles(namespace, root.binding(), name, maximum)?;
        if handles.is_empty() {
            return Ok(Vec::new());
        }
        // Ordinary creation has its own owner. Never overlay an ordinary name
        // introduced after this worktree reservation, including an unborn file.
        let ordinary = self
            .native_current_entries(repository)
            .map_err(RegistrationError::Publication)?;
        let mut mounts = Vec::with_capacity(handles.len());
        for handle in handles {
            let status = index.status(self, handle)?;
            let mount_key = status.name.key();
            let owner = self
                .state
                .repositories
                .get(&repository.to_string())
                .ok_or(RegistrationError::Source)?;
            let named_collision = self.state.repositories.values().any(|candidate| {
                !candidate.file_manager_root
                    && candidate.primary_slot_id == owner.primary_slot_id
                    && name_policy::lookup_key(&candidate.name) == mount_key
            });
            if named_collision
                || ordinary.iter().any(|entry| {
                    let path = match entry {
                        NativeCurrentEntry::Published(entry) => &entry.path,
                        NativeCurrentEntry::Unborn { target, .. } => &target.path,
                    };
                    path.key().split('/').next() == Some(mount_key.as_str())
                })
            {
                return Err(RegistrationError::Mount.into());
            }
            mounts.push(status);
        }
        mounts.sort_by(|left, right| left.name.key().cmp(&right.name.key()));
        Ok(mounts)
    }

    /// Bounded by the configured namespace-entry budget. A pending mount or a
    /// collision rejects the projection rather than disappearing into a create fallback.
    pub fn directory_worktree_mounts(
        &self,
        repository: RepositoryId,
    ) -> Result<Vec<WorktreeStatus>> {
        self.worktree_mounts(repository, None, self.config.max_entries)
    }

    /// A consumer may impose a smaller complete-result budget before any
    /// instance is reopened. Over-budget inventories fail rather than returning
    /// a silently truncated list; ordinary SMB retains its configured maximum.
    pub fn directory_worktree_mounts_bounded(
        &self,
        repository: RepositoryId,
        maximum: NonZeroU16,
    ) -> Result<Vec<WorktreeStatus>> {
        self.worktree_mounts(
            repository,
            None,
            self.config.max_entries.min(usize::from(maximum.get())),
        )
    }

    pub fn directory_worktree_mount(
        &self,
        repository: RepositoryId,
        name: &LookupName,
    ) -> Result<Option<WorktreeStatus>> {
        Ok(self
            .worktree_mounts(repository, Some(name), self.config.max_entries)?
            .into_iter()
            .next())
    }

    /// Metadata preserves canonical spelling without returning or copying a
    /// payload. Existing snapshot admission still verifies the original bytes.
    pub fn directory_worktree_entry(
        &self,
        target: WorktreeHandle,
        path: &LookupPath,
    ) -> Result<WorktreeEntry> {
        with_view(self, target, |view| {
            view.entry(path)
                .map(Into::into)
                .map_err(RegistrationError::ReadView)
                .map_err(Into::into)
        })
    }

    pub fn directory_worktree_smb_entry(&self,target:WorktreeHandle,path:&LookupPath) -> Result<WorktreeEntry> {
        with_view(self,target,|view| {
            let (display,entry)=view.smb_entry(self.state(),path,self.config().max_entries).map_err(RegistrationError::ReadView)?;
            let mut entry=WorktreeEntry::from(entry); entry.smb_path=Some(display); Ok(entry)
        })
    }
    pub fn directory_worktree_smb_entries(&self,target:WorktreeHandle,parent:Option<&LookupPath>,cursor:Option<&WorktreeCursor>)
        -> Result<WorktreePage> {
        with_view(self,target,|view| {
            let (rows,next)=view.smb_directory(self.state(),parent,cursor.map(|c|&c.inner),self.config().max_entries)
                .map_err(RegistrationError::ReadView)?;
            let entries=rows.into_iter().map(|(display,row)| {
                let mut entry=WorktreeEntry::from(row); entry.smb_path=Some(display); entry
            }).collect();
            Ok(WorktreePage {stamp:WorktreePageStamp {view:view.stamp(),parent:parent.cloned()},
                revision:view.stamp().revision,entries,next:next.map(|inner|WorktreeCursor {inner})})
        })
    }
    /// Under the SMB writer, a retry must reach its original result before
    /// present-day name admission. Only a fresh creation asks this question.
    pub fn require_vacant_worktree_smb_name(&self,target:WorktreeHandle,path:&PortablePath) -> Result<()> {
        with_view(self,target,|view| match view.smb_entry(self.state(),&LookupPath::from(path),self.config().max_entries) {
            Ok(_) => Err(RegistrationError::Mount.into()),
            Err(crate::directory_worktree::io::ViewError::Missing) => Ok(()),
            Err(error) => Err(RegistrationError::ReadView(error).into()),
        })
    }

    pub fn directory_worktree_entries(
        &self,
        target: WorktreeHandle,
        parent: Option<&LookupPath>,
        cursor: Option<&WorktreeCursor>,
    ) -> Result<WorktreePage> {
        with_view(self, target, |view| {
            let page = view
                .directory(
                    parent,
                    cursor.map(|cursor| &cursor.inner),
                    NonZeroU16::new(256).expect("invariant: the worktree page limit is nonzero"),
                )
                .map_err(RegistrationError::ReadView)?;
            Ok(WorktreePage {
                stamp: WorktreePageStamp {
                    view: view.stamp(),
                    parent: parent.cloned(),
                },
                revision: view.stamp().revision,
                entries: page.entries.into_iter().map(Into::into).collect(),
                next: page.next.map(|inner| WorktreeCursor { inner }),
            })
        })
    }

    /// A buffered page/suffix must be checked at delivery, not only when its
    /// next-page cursor is used. Reopening admits the same physical view again.
    pub fn check_directory_worktree_page(
        &self,
        target: WorktreeHandle,
        parent: Option<&LookupPath>,
        stamp: &WorktreePageStamp,
    ) -> Result<()> {
        with_view(self, target, |view| {
            if view.stamp() != stamp.view || stamp.parent.as_ref() != parent {
                return Err(RegistrationError::ReadView(ViewError::Cursor).into());
            }
            Ok(())
        })
    }

    /// Preliminary writable-handle admission shares the final edit's exact
    /// scope gate. It grants no later save: EditWorktree revalidates under its writer.
    pub fn check_directory_worktree_edit(
        &self,
        target: WorktreeHandle,
        path: &PortablePath,
        expected: WorktreeRevision,
    ) -> Result<()> {
        if self
            .admitted_current_namespaces()
            .map_err(RegistrationError::Publication)?
            .is_some()
            || self.config.generation().is_some()
        {
            return Err(RegistrationError::InitialOnly.into());
        }
        let index = self.snapshot_receipts()?;
        index.require_edit_scope(target, path, expected)?;
        let registered = index.worktree(target)?;
        let source = index.reopen_node(self, registered.source().handle())?;
        registered.reopen(&source)?;
        let original = source
            .manifest()
            .entries()
            .iter()
            .find(|entry| entry.path == *path)
            .ok_or(RegistrationError::Source)?;
        if !matches!(
            original.content,
            SnapshotEntryContent::File {
                source: SnapshotFileSource::Container { .. },
                ..
            }
        ) {
            return Err(RegistrationError::EditScope.into());
        }
        crate::snapshot_worktree::edit::require_mount(self, registered.reservation())
            .map_err(Into::into)
    }
}
