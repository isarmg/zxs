//! Worktree routing is an admitted mount union, not a `.zxs` pathname shortcut.
//! Existing snapshot files use an empty replacement or a supported CoW clone.
//! Added files reuse native working ranges; neither path copies a full body as fallback.
use crate::smb_rpc::{Handle, Session, handles, path_reservation};
use crate::{
    ApiError, SmbWorktreeAccess, SmbWorktreeCursor, SmbWorktreeEntryContent,
    SmbWorktreeFile, SmbWorktreeLocation,
};
use name_policy::{LookupName, LookupPath, PortablePath};
use platform_io::fs_safety::{PayloadOpenMode, PrivateDirectory};
use smb_protocol::protocol::{
    DirectoryEntry, DirectoryPage, FileMtime, OpenIo, OpenResult, StatResult, worktree_entry_id,
};
use std::{fs::File, os::fd::OwnedFd, time::{Duration, Instant}};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeStatus};
use storage_contract::{ContentHash, ErrorCode, NamespaceEntryKind, OperationId};

mod layout;
mod native;
mod writer;
pub(super) use writer::WorktreeWriter;

pub(super) struct WorktreeDirectory {
    pub(super) target: WorktreeHandle,
    mount: LookupName,
    parent: Option<LookupPath>,
    pending: Option<crate::SmbWorktreePage>,
    cursor: Option<SmbWorktreeCursor>,
    exhausted: bool,
    pub(super) file: File,
}

pub(super) fn display_path(mount: &LookupName, path: Option<&PortablePath>)
    -> Result<PortablePath, ApiError>
{
    let text = match path {
        Some(path) => format!("{}/{}", mount.as_str(), path.as_str()),
        None => mount.as_str().to_owned(),
    };
    PortablePath::parse(&text).map_err(ApiError::invalid_source)
}

pub(super) fn mount_entry(status: &WorktreeStatus) -> Result<DirectoryEntry, ApiError> {
    Ok(DirectoryEntry::Worktree {
        target: status.target, relative_path: None,
        path: display_path(&status.name, None)?, kind: NamespaceEntryKind::Directory, size: 0,
    })
}

impl Session {
    pub(super) fn open_worktree(
        &mut self, location: SmbWorktreeLocation, writable: bool, create: bool,
        truncate: bool, exclusive: bool, write_through: bool,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let status = location.status;
        let target = status.target;
        let entry = match location.path.as_ref() {
            Some(path) => match self.shared.backend.smb_worktree_entry(
                &self.identity, target, path,
            ) {
                Ok(entry) => Some(entry),
                Err(error) if create && error.code == ErrorCode::NotFound => {
                    return self.create_worktree_working(
                        &status, path, writable, truncate, write_through,
                    );
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        if create && exclusive {
            return Err(ApiError::conflict(ErrorCode::NameConflict, "entry already exists"));
        }
        let path = display_path(&status.name, entry.as_ref().map(|entry| &entry.path))?;
        if let Some(entry) = &entry
            && let SmbWorktreeEntryContent::Working { target, .. } = &entry.content
        {
            return self.open_worktree_working(
                target.clone(), &path, writable, truncate, write_through,
            );
        }
        let (entry_id, version_id) = match entry.as_ref().map(|entry| &entry.content) {
            Some(SmbWorktreeEntryContent::RetainedFile { target, version, .. }) => {
                (target.entry.to_string(), Some(*version))
            }
            Some(SmbWorktreeEntryContent::Working { .. }
                | SmbWorktreeEntryContent::Directory | SmbWorktreeEntryContent::File { .. })
                | None => {
                    (worktree_entry_id(target, entry.as_ref().map(|entry| &entry.path)), None)
                }
        };
        let mtime = entry.as_ref().map_or(FileMtime::NotFile {}, |entry| entry.mtime());
        let handle = self.next_handle;
        self.next_handle += 1;
        let directory = entry.as_ref().is_none_or(|entry| {
            entry.kind() == NamespaceEntryKind::Directory
        });
        let (opened, descriptor, size, data_io) = if directory {
            if writable || create || truncate {
                return Err(ApiError::conflict(ErrorCode::IsDirectory,
                    "worktree directory data is not writable"));
            }
            let page = self.shared.backend.smb_worktree_page(
                &self.identity, target, location.path.as_ref(), None,
            )?;
            let file = File::open(crate::smb_rpc::manual::layout::read_placeholder(
                &self.shared.spool,
            )).map_err(ApiError::io)?;
            let descriptor = OwnedFd::from(file.try_clone().map_err(ApiError::io)?);
            (Handle::WorktreeDirectory(WorktreeDirectory {
                target, mount: status.name, parent: location.path,
                pending: Some(page), cursor: None, exhausted: false, file,
            }), descriptor, 0, OpenIo::Directory)
        } else {
            let entry = entry.expect("invariant: a non-directory worktree entry is present");
            let selection = SmbWorktreeFile {
                target, path: entry.path, revision: status.revision, mtime,
            };
            if writable {
                self.open_worktree_writer(selection, &path, truncate, write_through)?
            } else {
                let content = self.shared.backend.smb_worktree_content(
                    &self.identity, &selection, Instant::now() + Duration::from_secs(300),
                )?;
                let manifest = content.checksum_manifest.as_ref().ok_or_else(|| {
                    ApiError::invalid("worktree content has no bounded range proof")
                })?;
                crate::verified_chunk::validate_manifest(manifest, content.size)?;
                let size = content.size;
                let descriptor = OwnedFd::from(File::open(
                    crate::smb_rpc::manual::layout::read_placeholder(&self.shared.spool),
                ).map_err(ApiError::io)?);
                (Handle::ReadWorktree(content, selection), descriptor, size, OpenIo::VerifiedRanges)
            }
        };
        self.handles.insert(handle, opened);
        Ok((OpenResult {
            data_io, entry_id, kind: if directory { NamespaceEntryKind::Directory }
                else { NamespaceEntryKind::File },
            path: path.into_string(), handle, size, max_file_bytes: self.max_file_bytes,
            version_id: version_id.map(|version| version.to_string()), workspace_id: None, mtime,
        }, descriptor))
    }

    fn open_worktree_writer(
        &self, selection: SmbWorktreeFile, path: &PortablePath, truncate: bool, write_through: bool,
    ) -> Result<(Handle, OwnedFd, u64, OpenIo), ApiError> {
        let paths = path_reservation::PathReservation::acquire(
            &self.shared, [(self.branch.as_str(), path.key())],
        )?;
        self.shared.backend.authorize_smb_worktree(&self.identity, selection.target,
            SmbWorktreeAccess::Write { path: &selection.path, expected: selection.revision })?;
        let operation = OperationId::generate().map_err(ApiError::contract_input)?;
        let directory = PrivateDirectory::open_trusted(&self.shared.spool)
            .map_err(crate::upload_fs_error)?;
        let name = layout::source(operation)?;
        let content = if truncate { None } else {
            Some(self.shared.backend.smb_worktree_content(
                &self.identity, &selection, Instant::now() + Duration::from_secs(300),
            )?.fully_verified()?)
        };
        let size = content.as_ref().map_or(0, |content| content.size);
        let hash = match &content {
            Some(content) => content.hash.parse().map_err(ApiError::invalid_source)?,
            None => ContentHash::from_digest(blake3::hash(&[])),
        };
        let reservation = self.shared.budget.reserve(
            &self.identity.principal, &directory.path().join(name.as_path()), size,
            self.max_file_bytes,
        )?;
        let file = directory.open_payload(&name, PayloadOpenMode::CreateNew)
            .map_err(crate::upload_fs_error)?;
        if let Some(mut content) = content {
            directory.verify_payload(&name, &file).map_err(crate::upload_fs_error)?;
            if !content.file.try_clone_working_file(&file, size).map_err(ApiError::io)? {
                // The clone primitive proved this target is still empty. No
                // descriptor or publication escaped, so remove only our held
                // private source; Lease releases its charge once evidence is absent.
                directory.verify_payload(&name, &file).map_err(crate::upload_fs_error)?;
                directory.remove_file(&name).map_err(crate::upload_fs_error)?;
                return Err(ApiError::unsupported());
            }
            content.check_running()?;
        }
        file.sync_all().map_err(ApiError::io)?;
        self.shared.backend.authorize_smb_worktree(&self.identity, selection.target,
            SmbWorktreeAccess::Write { path: &selection.path, expected: selection.revision })?;
        let descriptor = OwnedFd::from(file.try_clone().map_err(ApiError::io)?);
        let writer = WorktreeWriter::new(writer::Preparation {
            selection, operation, reservation, file, directory, name, hash, size,
            truncated: truncate, write_through,
        }, &self.identity)?;
        Ok((Handle::WorktreeWrite(Box::new(writer), paths), descriptor, size, OpenIo::Descriptor))
    }

    pub(super) fn stat_worktree(&self, location: SmbWorktreeLocation)
        -> Result<StatResult, ApiError>
    {
        let entry = location.path.as_ref().map(|path| {
            self.shared.backend.smb_worktree_entry(&self.identity, location.status.target, path)
        }).transpose()?;
        let relative = entry.as_ref().map(|entry| &entry.path);
        let path = display_path(&location.status.name, relative)?;
        let (kind, size) = entry.as_ref().map_or((NamespaceEntryKind::Directory, 0), |entry| {
            (entry.kind(), entry.size())
        });
        let entry_id = match entry.as_ref().map(|entry| &entry.content) {
            Some(SmbWorktreeEntryContent::Working { target, .. }
                | SmbWorktreeEntryContent::RetainedFile { target, .. }) => target.entry.to_string(),
            Some(SmbWorktreeEntryContent::Directory | SmbWorktreeEntryContent::File { .. })
                | None => worktree_entry_id(location.status.target, relative),
        };
        Ok(StatResult {
            entry_id,
            name: path.file_name().into(), path: path.into_string(), kind, size,
            mtime: entry.as_ref().map_or(FileMtime::NotFile {}, |entry| entry.mtime()),
        })
    }

    pub(super) fn read_worktree_directory(&mut self, handle: u64, restart: bool)
        -> Result<DirectoryPage, ApiError>
    {
        let Some(Handle::WorktreeDirectory(directory)) = self.handles.get_mut(&handle) else {
            return Err(handles::missing_handle());
        };
        self.shared.backend.authorize_smb_worktree(
            &self.identity, directory.target, SmbWorktreeAccess::Read,
        )?;
        if restart || directory.pending.is_none() {
            if !restart && directory.exhausted {
                return Ok(DirectoryPage { entries: Vec::new(), more: false });
            }
            let page = self.shared.backend.smb_worktree_page(
                &self.identity, directory.target, directory.parent.as_ref(),
                if restart { None } else { directory.cursor.as_ref() },
            )?;
            directory.pending = Some(page);
        }
        // Keep the core cursor and complete unreturned page on every encoding
        // or wire-budget error. An oversized wire envelope cannot drop entries.
        let page = directory.pending.as_ref()
            .expect("invariant: the pending page was captured above");
        self.shared.backend.authorize_smb_worktree_page(
            &self.identity, directory.target, directory.parent.as_ref(), page,
        )?;
        let mut entries = Vec::new();
        let mut bytes = 128usize;
        for entry in &page.entries {
            let path = display_path(&directory.mount, Some(&entry.path))?;
            let wire = match &entry.content {
                SmbWorktreeEntryContent::Working { target, working } => {
                    DirectoryEntry::WorktreeWorking {
                        target: target.clone(), path, logical_size: working.logical_size(),
                    }
                }
                SmbWorktreeEntryContent::RetainedFile { target, version, size, mtime } => {
                    DirectoryEntry::WorktreeRetained {
                        target: target.clone(), path, version: *version, size: *size, mtime: *mtime,
                    }
                }
                SmbWorktreeEntryContent::Directory | SmbWorktreeEntryContent::File { .. } => {
                    DirectoryEntry::Worktree {
                        target: directory.target, path, relative_path: Some(entry.path.clone()),
                        kind: entry.kind(), size: entry.size(),
                    }
                }
            };
            let extra = json_codec::to_vec(&wire).map_err(ApiError::invalid_source)?.len() + 1;
            if bytes + extra > smb_protocol::protocol::MAX_DIRECTORY_PAGE_BYTES {
                if entries.is_empty() {
                    return Err(ApiError::limit("worktree entry exceeds SMB frame budget"));
                }
                break;
            }
            bytes += extra;
            entries.push(wire);
        }
        let mut page = directory.pending.take()
            .expect("invariant: encoding borrowed but did not remove the pending page");
        if entries.len() < page.entries.len() {
            page.entries.drain(..entries.len());
            directory.pending = Some(page);
            directory.exhausted = false;
        } else {
            directory.cursor = page.next;
            directory.exhausted = directory.cursor.is_none();
        }
        Ok(DirectoryPage { entries, more: !directory.exhausted })
    }
}
