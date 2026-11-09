//! Validate the captured occurrence against the transaction's actual namespace.
use super::*;
use storage_contract::file_relocation::FileRelocation;

pub(crate) fn require(
    state: &State,
    request: &FileRelocation,
    head: &str,
    entries: &[Entry],
) -> Result<()> {
    if head != request.expected_head.as_str() {
        return Err(Error::new(
            ErrorCode::ListingChanged,
            "the captured namespace head changed; refresh and review the original file",
        ));
    }
    let source = entries.iter().find(|entry| {
        entry.lookup_key() == request.source.key()
            && entry.kind == NamespaceEntryKind::File
            && entry.entry_id == request.entry_id.as_str()
            && entry.file_id == request.file_id.as_str()
            && entry.path() == request.source.as_str()
    });
    if source.is_none() {
        return Err(Error::new(
            ErrorCode::ListingChanged,
            "the captured file occurrence no longer occupies the original path",
        ));
    }
    if request.source == request.destination {
        return Err(Error::new(
            ErrorCode::InvalidMove,
            "source and destination are identical",
        ));
    }
    if state
        .directory_git_repositories
        .contains_key(request.repository_id.as_str())
        || entries.iter().any(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && state
                    .directory_git_repositories
                    .contains_key(&entry.entry_id)
                && [request.source.key(), request.destination.key()]
                    .iter()
                    .any(|path| super::namespace::is_descendant(path, entry.lookup_key()))
        })
    {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "initialized directory repositories require their explicit branch operation",
        ));
    }
    if let Some(parent) = request.destination.parent()
        && !entries.iter().any(|entry| {
            entry.kind == NamespaceEntryKind::Directory && entry.lookup_key() == parent.key()
        })
    {
        return Err(Error::new(
            ErrorCode::ParentNotDirectory,
            "the destination parent must already exist",
        ));
    }
    Ok(())
}
