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
                    409,
                    storage_contract::ErrorCode::NotDirectory,
                    "handle is not a directory",
                ));
            }
            None => {
                return Err(ApiError::new(
                    404,
                    storage_contract::ErrorCode::NotFound,
                    "SMB handle missing",
                ));
            }
        };
        let mut input = json!({"branch_id":self.branch,"entry_id":entry_id,"path":path});
        if let Some(cursor) = &cursor {
            input["commit_id"] = json!(cursor.commit_id);
            input["after"] = json!(cursor.after);
        }
        // Backend revalidates the account and binding under the engine lock,
        // then copies only one page from the immutable commit. The client never
        // supplies a snapshot id, path alias or cursor key for an open handle.
        let mut result = self.call(crate::BackendAction::SmbReadDirectory, input)?;
        let commit_id = field(&result, "commit_id")?.to_owned();
        result
            .as_object_mut()
            .ok_or_else(|| ApiError::invalid("invalid directory page"))?
            .remove("commit_id");
        let page: DirectoryPage =
            json_codec::from_value(result.clone()).map_err(ApiError::invalid)?;
        if page.more && page.entries.is_empty() {
            return Err(ApiError::invalid("nonterminal directory page is empty"));
        }
        let after = page
            .entries
            .last()
            .map(|entry| entry.lookup_key.clone())
            .or_else(|| cursor.and_then(|cursor| cursor.after));
        if let Some(Handle::Directory(directory)) = self.handles.get_mut(&handle) {
            directory.cursor = Some(DirectoryCursor { commit_id, after });
        }
        Ok(result)
    }
}
