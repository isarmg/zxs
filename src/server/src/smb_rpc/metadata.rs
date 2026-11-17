//! Saved-version metadata only. No payload opens, copies or inode timestamps.
use super::*;
use smb_protocol::protocol::{AuthorizedMetadata, FileMtime, StatResult};

impl Session {
    pub(super) fn version_mtime(&self, version: &str) -> Result<FileMtime, ApiError> {
        let snapshot = self
            .shared
            .backend
            .version_snapshot(&self.identity, version)?;
        crate::backend_read::version_mtime(&snapshot.version)
    }

    pub(super) fn stat(&self, path: &str) -> Result<StatResult, ApiError> {
        let path = if path.is_empty() || path == "." {
            SmbRelativePath::Root
        } else {
            SmbRelativePath::Entry(
                name_policy::LookupPath::parse(path).map_err(|source| contract(source.into()))?,
            )
        };
        self.stat_typed(&path)
    }

    pub(super) fn stat_typed(&self, path: &SmbRelativePath) -> Result<StatResult, ApiError> {
        let result = match path {
            SmbRelativePath::Root => {
                self.authorize_binding()?;
                StatResult {
                    entry_id: self.branch.clone(),
                    kind: storage_contract::NamespaceEntryKind::Directory,
                    path: String::new(),
                    name: String::new(),
                    size: 0,
                    mtime: FileMtime::NotFile {},
                }
            }
            SmbRelativePath::Entry(path) => {
                if let Some(location) = self.shared.backend.smb_worktree_locate(
                    &self.identity, &self.branch, path,
                )? {
                    return self.stat_worktree(location);
                }
                if let Some(native) = self.shared.backend.smb_native_stat(
                    &self.identity, &self.branch, path,
                )? {
                    return Ok(native);
                }
                let mut entry = self.find(path)?.ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "entry missing",
                    )
                })?;
                if entry.kind == storage_contract::NamespaceEntryKind::File
                    && let Some(file) = self.shared.backend.smb_file_working(
                        &self.identity, &self.branch, &entry,
                    )?
                    && let Some((_, payload)) = file.working.draft()
                {
                    entry.size = payload.logical_size();
                }
                let mtime = match entry.kind {
                    storage_contract::NamespaceEntryKind::File => {
                        self.version_mtime(&entry.version_id)?
                    }
                    storage_contract::NamespaceEntryKind::Directory => FileMtime::NotFile {},
                };
                StatResult {
                    name: entry.name().to_owned(),
                    path: entry.path.into_string(),
                    entry_id: entry.entry_id,
                    kind: entry.kind,
                    size: entry.size,
                    mtime,
                }
            }
        };
        Ok(result)
    }

    /// last_head advances only through the existing publication confirmation.
    /// A dirty successor keeps that saved time; a new unpublished file has none.
    pub(super) fn handle_metadata(&self, opened: &Handle) -> Result<AuthorizedMetadata, ApiError> {
        let mtime = match opened {
            Handle::Native(opened) => opened.file.mtime(),
            Handle::WorktreeWrite(_, _) => FileMtime::Unpublished {},
            Handle::ReadWorktree(_, file) => file.mtime,
            Handle::WorktreeDirectory(_) => FileMtime::NotFile {},
            Handle::Read(_, version) => self.version_mtime(version)?,
            Handle::ReadDraft(_, file) => {
                self.version_mtime(&file.base.version_id().to_string())?
            }
            Handle::ManualWrite(writer, _) => {
                self.version_mtime(&writer.selection().base.version_id().to_string())?
            }
            Handle::Directory(_) => FileMtime::NotFile {},
            Handle::Write(writer, _) => {
                let entry = self.shared.backend.lookup_at(
                    &self.identity,
                    &self.branch,
                    &writer.last_head,
                    &name_policy::LookupPath::from(&writer.path),
                )?;
                match entry {
                    Some(entry) if entry.entry_id == writer.item_id => {
                        self.version_mtime(&entry.version_id)?
                    }
                    None => FileMtime::Unpublished {},
                    Some(_) => {
                        return Err(ApiError::conflict(
                            storage_contract::ErrorCode::FileVersionConflict,
                            "saved handle identity differs from its captured publication",
                        ));
                    }
                }
            }
        };
        Ok(AuthorizedMetadata { mtime })
    }
}
