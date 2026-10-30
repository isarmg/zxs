//! Bounded, handle-owned enumeration. Clients cannot select historical commits.
use super::*;
use smb_protocol::protocol::DirectoryPage;

impl Session {
    pub(super) fn read_directory(&mut self, handle: u64, restart: bool) -> Result<Value, ApiError> {
        let (entry_id, path, cursor) = match self.handles.get(&handle) {
            Some(Handle::Directory(directory)) => (
                directory.entry_id.clone(),
                directory.path.clone(),
                (!restart).then(|| directory.cursor.clone()).flatten(),
            ),
            Some(_) => {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::NotDirectory,
                    "handle is not a directory",
                ));
            }
            None => {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "SMB handle missing",
                ));
            }
        };
        // The cursor comes only from this connection's handle, never from the peer.
        let snapshot = self.shared.backend.read_directory(
            &self.identity,
            crate::ReadDirectory {
                branch: &self.branch,
                entry_id: &entry_id,
                path: path.as_ref(),
                selection: crate::DirectorySelection::Page {
                    commit: cursor.as_ref().map(|cursor| cursor.commit_id.as_str()),
                    after: cursor.as_ref().and_then(|cursor| cursor.after.as_deref()),
                },
            },
        )?;
        let crate::DirectorySnapshot {
            commit_id,
            entries,
            more,
        } = snapshot;
        let page = DirectoryPage { entries, more };
        let after = page
            .entries
            .last()
            .map(|node| node.entry.lookup_key().to_owned())
            .or_else(|| cursor.and_then(|cursor| cursor.after));
        if let Some(Handle::Directory(directory)) = self.handles.get_mut(&handle) {
            directory.cursor = Some(DirectoryCursor { commit_id, after });
        }
        Ok(json_codec::to_value(page).expect("invariant: directory page serializes"))
    }
}
