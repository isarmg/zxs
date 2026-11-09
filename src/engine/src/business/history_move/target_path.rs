//! Ordinary destination directories are namespace parents, never new directory
//! repository authority. Both live capture and exact candidate replay use this.
use super::*;

pub(in crate::business) fn ancestors<'a>(
    entries: &'a [Entry],
    target: &name_policy::PortablePath,
) -> Result<Vec<&'a Entry>> {
    let mut parents = Vec::new();
    let mut key = target.key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        let entry = namespace::find_unordered_key(entries, parent).ok_or_else(|| {
            Error::new(ErrorCode::DirectoryNotFound, "move destination parent is absent")
        })?;
        if entry.kind != NamespaceEntryKind::Directory {
            return Err(Error::new(
                ErrorCode::NotDirectory,
                "move destination ancestor is not a directory",
            ));
        }
        parents.push(entry);
        key = parent;
    }
    Ok(parents)
}

pub(in crate::business) fn require_parent(
    state: &State,
    entries: &[Entry],
    target: &name_policy::PortablePath,
) -> Result<()> {
    for entry in ancestors(entries, target)? {
        if state.directory_git_repositories.contains_key(&entry.entry_id) {
            return Err(Error::new(
                ErrorCode::DirectoryScopeRequired,
                "move destination is inside an initialized directory repository",
            ));
        }
    }
    Ok(())
}

/// A FILE snapshot keeps only this file and its exact existing parent chain,
/// never siblings from the destination namespace.
pub(in crate::business) fn file_snapshot(parents: &[Entry], file: Entry) -> Result<Vec<Entry>> {
    let mut entries: Vec<_> = parents
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && namespace::is_descendant(file.lookup_key(), entry.lookup_key())
        })
        .cloned()
        .collect();
    entries.push(file);
    namespace::validate_entries(&mut entries, usize::MAX)?;
    Ok(entries)
}
