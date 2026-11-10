//! Bounded, handle-owned enumeration. Clients cannot select historical commits.
use super::*;
use smb_protocol::protocol::DirectoryPage;

impl Session {
    pub(super) fn read_directory(&mut self, handle: u64, restart: bool) -> Result<Value, ApiError> {
        let directory = match self.handles.get_mut(&handle) {
            Some(Handle::Directory(directory)) => directory,
            Some(_) => {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::NotDirectory,
                    "handle is not a directory",
                ));
            }
            None => return Err(handles::missing_handle()),
        };
        let cursor = if restart {
            None
        } else {
            directory.cursor.as_ref()
        };
        // The cursor comes only from this connection's handle, never from the peer.
        let snapshot = self.shared.backend.read_directory(
            &self.identity,
            crate::ReadDirectory {
                branch: &self.branch,
                entry_id: &directory.entry_id,
                path: directory.path.as_ref(),
                selection: crate::DirectorySelection::Page {
                    snapshot: cursor.map(|cursor| &cursor.snapshot),
                    after: cursor.and_then(|cursor| cursor.after.as_deref()),
                },
            },
        )?;
        let crate::DirectorySnapshot {
            commit_id: _,
            snapshot,
            entries,
            more,
        } = snapshot;
        let page = DirectoryPage { entries, more };
        let after = page
            .entries
            .last()
            .map(|node| node.entry.lookup_key().to_owned())
            .or_else(|| cursor.and_then(|cursor| cursor.after.clone()));
        directory.cursor = Some(DirectoryCursor { snapshot, after });
        Ok(json_codec::to_value(page).expect("invariant: directory page serializes"))
    }
}
