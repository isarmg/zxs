//! Ordered native frames retain their real core source; no mutable BODY FD is sent.
use super::{Handle, OpenResult, OwnedFd, Session, handles, path_reservation};
use crate::{ApiError, SmbNativeEdit, SmbNativeHandle, SmbNativeOpen};
use smb_protocol::protocol::{FileMtime, OpenIo, WorkingPosition, WorkingStat, WorkingWriteResult};
use std::fs::File;

pub(super) struct NativeHandle {
    pub(super) file: SmbNativeHandle,
    pub(super) writable: bool,
    pub(super) write_through: bool,
    pub(super) _paths: Option<path_reservation::PathReservation>,
}

impl Session {
    pub(super) fn open_native(
        &mut self, path: &name_policy::LookupPath, writable: bool, create: bool,
        truncate: bool, exclusive: bool, write_through: bool,
    ) -> Result<Option<(OpenResult, OwnedFd)>, ApiError> {
        let paths = if writable || create {
            Some(path_reservation::PathReservation::acquire(
                &self.shared, [(self.branch.as_str(), path.key())],
            )?)
        } else { None };
        let Some(file) = self.shared.backend.smb_native_open(&self.identity, SmbNativeOpen {
            branch: &self.branch, path, writable, create, truncate, exclusive,
        })? else { return Ok(None); };
        let size = self.shared.backend.smb_native_size(&self.identity, &file)?;
        // The same real non-content placeholder as verified reads. A peer that
        // ignores the negotiated mode cannot write container bytes through it.
        let descriptor = OwnedFd::from(File::open(
            super::manual::layout::read_placeholder(&self.shared.spool),
        ).map_err(ApiError::io)?);
        let handle = self.next_handle;
        self.next_handle += 1;
        let info = OpenResult {
            data_io: OpenIo::NativeWorking, entry_id: file.target.entry.to_string(),
            kind: storage_contract::NamespaceEntryKind::File,
            path: file.target.path.as_str().into(), handle, size,
            max_file_bytes: self.max_file_bytes,
            version_id: None, workspace_id: None, mtime: FileMtime::Unpublished {},
        };
        self.handles.insert(handle, Handle::Native(NativeHandle {
            file, writable, write_through, _paths: paths,
        }));
        Ok(Some((info, descriptor)))
    }

    pub(super) fn working_stat(&self, handle: u64) -> Result<WorkingStat, ApiError> {
        let Some(Handle::Native(opened)) = self.handles.get(&handle) else {
            return Err(handles::missing_handle());
        };
        Ok(WorkingStat { size: self.shared.backend.smb_native_size(&self.identity, &opened.file)? })
    }

    pub(super) fn working_write(
        &mut self, handle: u64, position: WorkingPosition, bytes: &[u8],
    ) -> Result<WorkingWriteResult, ApiError> {
        if bytes.len() > smb_protocol::protocol::MAX_READ_BYTES {
            return Err(ApiError::limit("native working write exceeds frame budget"));
        }
        let Some(Handle::Native(opened)) = self.handles.get_mut(&handle) else {
            return Err(handles::missing_handle());
        };
        if !opened.writable { return Err(ApiError::forbidden()); }
        let start = match position {
            WorkingPosition::At { offset } => offset,
            WorkingPosition::Append {} => {
                self.shared.backend.smb_native_size(&self.identity, &opened.file)?
            }
        };
        if start.checked_add(bytes.len() as u64).is_none_or(|end| end > self.max_file_bytes) {
            return Err(ApiError::limit("native working file exceeds handle byte budget"));
        }
        let edit = match position {
            WorkingPosition::At { offset } => SmbNativeEdit::Write { offset, bytes },
            WorkingPosition::Append {} => SmbNativeEdit::Append { bytes },
        };
        let written = self.shared.backend.smb_native_edit(&self.identity, &mut opened.file, edit)?;
        if opened.write_through {
            self.shared.backend.smb_native_flush(&self.identity, &mut opened.file)?;
        }
        Ok(WorkingWriteResult { written: written as u32 })
    }

    pub(super) fn working_resize(&mut self, handle: u64, length: u64) -> Result<(), ApiError> {
        let Some(Handle::Native(opened)) = self.handles.get_mut(&handle) else {
            return Err(handles::missing_handle());
        };
        if !opened.writable { return Err(ApiError::forbidden()); }
        if length > self.max_file_bytes {
            return Err(ApiError::limit("native working file exceeds handle byte budget"));
        }
        self.shared.backend.smb_native_edit(
            &self.identity, &mut opened.file, SmbNativeEdit::Resize { length },
        )?;
        if opened.write_through {
            self.shared.backend.smb_native_flush(&self.identity, &mut opened.file)?;
        }
        Ok(())
    }
}
