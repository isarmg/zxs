//! Stable physical-owner ancestry for a current ordinary FILE. Current display
//! paths, FileId, and snapshot host admission each prove different things.
use super::DraftError as Failure;
use crate::{Error, NamespaceEntryKind, Result, State};
use name_policy::PortablePath;
use storage_contract::file_container::{FileContainerRecord, NamespaceDirectoryParent};
use storage_contract::file_draft::DraftLocation;
use storage_contract::working_tree::BindingLifecycle;
use storage_contract::{EntryId, FileId};

pub(crate) struct OrdinaryOccurrence {
    pub(crate) target: DraftLocation,
    // Root to parent, excluding the repository root (addressed by RepositoryId).
    pub(crate) parents: Vec<(EntryId, PortablePath)>,
}

impl OrdinaryOccurrence {
    pub(crate) fn capture(
        state: &State, file: FileId, record: &FileContainerRecord,
    ) -> Result<Self> {
        let missing = || Error::FileDraft(Failure::Selection);
        let unsupported = || Error::FileDraft(Failure::UnsupportedScope);
        let namespace = &record.namespace;
        let mut parent = namespace.parent.ordinary().ok_or_else(unsupported)?;
        let mut reverse = Vec::new();
        // PortablePath includes its leaf in the 32-component format limit.
        while let NamespaceDirectoryParent::Directory(id) = parent {
            if reverse.len() == 31 { return Err(unsupported()); }
            let key = id.to_string();
            let row = state.namespace_directories.get(&key).ok_or_else(missing)?;
            if row.entry != id || row.repository != namespace.repository
                || row.branch != namespace.branch || row.owner_disk != record.owner_disk
                || row.replicas.len() != 1 || !row.replicas.get(&record.owner_disk)
                    .is_some_and(|replica| replica.lifecycle == BindingLifecycle::Current)
                || state.directory_git_repositories.contains_key(&key)
            {
                return Err(unsupported());
            }
            reverse.push((id, &row.name));
            parent = row.parent;
        }
        let mut parents = Vec::with_capacity(reverse.len());
        let mut path = String::new();
        for (id, name) in reverse.into_iter().rev() {
            path.push_str(name.as_str());
            parents.push((id, PortablePath::parse(&path)?));
            path.push('/');
        }
        path.push_str(namespace.name.as_str());
        let path = PortablePath::parse(&path)?;
        if !crate::persist::layout::namespace::ordinary_path(&path) {
            return Err(unsupported());
        }
        Ok(Self {
            target: DraftLocation {
                repository: namespace.repository, path, entry: namespace.entry, file,
            },
            parents,
        })
    }

    pub(crate) fn require_target(&self, target: &DraftLocation) -> Result<()> {
        if &self.target != target { return Err(Error::FileDraft(Failure::Selection)); }
        Ok(())
    }

    /// The published current namespace, not just registered names, must
    /// resolve every parent to exactly its current entry and display spelling.
    pub(crate) fn require_visible<'a>(
        &self, lookup: impl Fn(&PortablePath) -> Option<&'a crate::Entry>,
    ) -> Result<()> {
        for (id, path) in &self.parents {
            let entry = lookup(path).ok_or(Error::FileDraft(Failure::Selection))?;
            if entry.kind != NamespaceEntryKind::Directory || &entry.path != path
                || entry.entry_id != id.to_string()
            {
                return Err(Error::FileDraft(Failure::Selection));
            }
        }
        Ok(())
    }
}
