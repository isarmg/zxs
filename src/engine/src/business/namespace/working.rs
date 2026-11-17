//! A single current namespace projection, not a second writable namespace.
//!
//! Published entries retain their historical wire representation. Unborn rows
//! borrow the authoritative container record; no synthetic Entry/Version exists.
use crate::business::file_draft::DraftError as Failure;
use crate::{BranchState, Entry, Error, NamespaceEntryKind, Result, State};
use name_policy::{LookupPath, Name, PortablePath};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::file_container::{
    ContainerNamespace, ContainerParent, FileContainerRecord, NamespaceDirectoryParent,
};
use storage_contract::{BranchId, EntryId, FileId, HistoryTreeId, RepositoryId};

pub(crate) enum WorkingEntry<'a> {
    Published(Entry),
    Unborn {
        file: FileId,
        record: &'a FileContainerRecord,
        path: PortablePath,
    },
}

impl WorkingEntry<'_> {
    pub(crate) fn path(&self) -> &PortablePath {
        match self {
            Self::Published(entry) => &entry.path,
            Self::Unborn { path, .. } => path,
        }
    }

    pub(crate) fn entry_id(&self) -> Result<EntryId> {
        match self {
            Self::Published(entry) => Ok(entry.entry_id.parse()?),
            Self::Unborn { record, .. } => Ok(record.namespace.entry),
        }
    }

    pub(crate) fn published(&self) -> Result<&Entry> {
        match self {
            Self::Published(entry) => Ok(entry),
            Self::Unborn { .. } => Err(Error::FileDraft(Failure::UnbornRequired)),
        }
    }

    pub(crate) fn is_directory(&self) -> bool {
        matches!(self, Self::Published(entry) if entry.kind == NamespaceEntryKind::Directory)
    }
}

pub(crate) struct WorkingNamespace<'a> {
    state: &'a State,
    repository: RepositoryId,
    branch: BranchId,
    history: HistoryTreeId,
    entries: Vec<WorkingEntry<'a>>,
}

fn invalid() -> Error {
    Error::FileDraft(Failure::WorkingNamespace)
}

impl<'a> WorkingNamespace<'a> {
    /// Pure projection only. Callers retain the actual admitted State and the
    /// original namespace/root proof for every I/O or mutation decision.
    pub(crate) fn from_main_state(
        state: &'a State,
        repository: RepositoryId,
        maximum: usize,
    ) -> Result<Self> {
        let repo = state.repositories.get(&repository.to_string())
            .filter(|repo| repo.file_manager_root).ok_or_else(invalid)?;
        let branch = state.branches.get(&repo.main_branch_id).filter(|branch| {
            branch.file_id.is_none() && branch.state == BranchState::Active
                && branch.history_tree_id == repo.history_tree_id
        }).ok_or_else(invalid)?;
        let commit = state.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
        let branch_id = branch.branch_id.parse::<BranchId>()?;
        let history = repo.history_tree_id.parse::<HistoryTreeId>()?;
        if commit.history_tree_id != repo.history_tree_id || commit.entries.len() > maximum {
            return Err(invalid());
        }
        // Use the existing canonical current spelling kernel. Historical
        // commits are never rewritten to include these names or unborn rows.
        let mut published = commit.entries.clone();
        let names = super::container_name::current_names(state, &published)?;
        super::container_name::apply_file_names(&mut published, &names)?;
        let mut directory_by_id = BTreeMap::new();
        let mut occupied_ids = BTreeSet::new();
        for entry in &published {
            let id = entry.entry_id.parse::<EntryId>()?;
            if !occupied_ids.insert(id) { return Err(invalid()); }
            if entry.kind == NamespaceEntryKind::Directory {
                directory_by_id.insert(id, entry);
            }
        }
        let visible = super::NamespaceTree::from_current_state(
            state, &repo.repository_id, &published,
        );
        let mut entries: Vec<_> = visible.visible_entries()
            .cloned().map(WorkingEntry::Published).collect();
        // One pass, not one full retained-graph scan per current unborn file.
        // These identities have genuinely entered history and cannot be
        // re-labelled Unborn after their last visible path disappears.
        let historical_files: BTreeSet<FileId> = state.versions.values()
            .map(|version| version.file_id.as_str())
            .chain(state.branches.values().filter_map(|branch| branch.file_id.as_deref()))
            .chain(state.file_metadata_nodes.values().map(|node| node.file_id.as_str()))
            .chain(state.file_branch_merges.values().map(|node| node.file_id.as_str()))
            .chain(state.file_deletions.keys().map(String::as_str))
            .map(str::parse).collect::<std::result::Result<_, _>>()?;
        for (file, record) in state.file_containers.iter() {
            if record.namespace.repository != repository || record.working.unborn().is_none()
                || record.namespace.parent.is_worktree() {
                continue;
            }
            let file_key = file.to_string();
            if entries.len() == maximum || record.history_tree_id != history
                || record.namespace.branch != branch_id
                || !occupied_ids.insert(record.namespace.entry)
                || historical_files.contains(file)
                || state.file_selections.contains_key(&file_key)
                || state.file_locations.contains_key(&file_key)
                || state.file_names.contains_key(&file_key)
                || state.file_notes.contains_key(&file_key)
                || state.file_backup_paths.contains_key(&file_key)
                || state.file_backup_policies.contains_key(&file_key)
                || state.file_write_origins.contains_key(&file_key)
                || record.working.revision().generation().get() == 0
                || state.container_tags.get(file)
                    != Some(&storage_contract::file_container_tags::ContainerTags::empty())
            {
                return Err(invalid());
            }
            let name = Name::parse(record.namespace.name.as_str())?;
            let path = match &record.namespace.parent {
                ContainerParent::Worktree { .. } => continue,
                ContainerParent::Root => PortablePath::from_name(&name)?,
                ContainerParent::Directory(parent) => {
                    let entry = directory_by_id.get(parent).ok_or_else(invalid)?;
                    let binding = state.namespace_directories.get(&parent.to_string())
                        .ok_or_else(invalid)?;
                    if binding.repository != repository || binding.branch != branch_id
                        || binding.entry != *parent || binding.owner_disk != record.owner_disk
                        || binding.name.as_str() != entry.name()
                        || visible.get_key(entry.lookup_key()).is_none()
                    {
                        return Err(invalid());
                    }
                    entry.path.join_name(&name)?
                }
            };
            if !crate::container_storage::namespace_host::ordinary_path(&path) {
                return Err(invalid());
            }
            entries.push(WorkingEntry::Unborn { file: *file, record, path });
        }
        entries.sort_by(|left, right| left.path().key().cmp(right.path().key()));
        if entries.windows(2).any(|pair| pair[0].path().key() == pair[1].path().key()) {
            return Err(invalid());
        }
        let result = Self { state, repository, branch: branch_id, history, entries };
        for entry in &result.entries {
            if let Some(parent) = entry.path().parent()
                && !result.get_key(parent.key()).is_some_and(WorkingEntry::is_directory)
            {
                return Err(invalid());
            }
        }
        Ok(result)
    }

    pub(crate) fn unborn_file(&self, file: FileId) -> Result<&FileContainerRecord> {
        self.entries.iter().find_map(|entry| match entry {
            WorkingEntry::Unborn { file: id, record, .. } if *id == file => Some(*record),
            WorkingEntry::Unborn { .. } | WorkingEntry::Published(_) => None,
        }).ok_or_else(invalid)
    }

    pub(crate) fn state(&self) -> &'a State { self.state }
    pub(crate) fn repository(&self) -> RepositoryId { self.repository }
    pub(crate) fn branch(&self) -> BranchId { self.branch }
    pub(crate) fn history(&self) -> HistoryTreeId { self.history }
    pub(crate) fn entries(&self) -> &[WorkingEntry<'a>] { &self.entries }

    fn get_key(&self, key: &str) -> Option<&WorkingEntry<'a>> {
        self.entries.binary_search_by(|entry| entry.path().key().cmp(key))
            .ok().map(|index| &self.entries[index])
    }

    pub(crate) fn get(&self, path: &LookupPath) -> Option<&WorkingEntry<'a>> {
        self.get_key(path.key())
    }

    /// All create/batch/Mkdir/import paths must consume this same check. An
    /// unborn row is occupied even though no historical Entry mentions it.
    pub(crate) fn vacant_parent(&self, path: &PortablePath) -> Result<NamespaceDirectoryParent> {
        if self.get_key(path.key()).is_some() {
            return Err(invalid());
        }
        match path.parent() {
            None => Ok(NamespaceDirectoryParent::Root),
            Some(parent) => {
                let entry = self.get_key(parent.key()).filter(|entry| entry.is_directory())
                    .ok_or_else(invalid)?;
                Ok(NamespaceDirectoryParent::Directory(entry.entry_id()?))
            }
        }
    }

    pub(crate) fn namespace_for_birth(
        &self, path: &PortablePath, entry: EntryId,
    ) -> Result<ContainerNamespace> {
        let parent = self.vacant_parent(path)?;
        Ok(ContainerNamespace {
            repository: self.repository,
            branch: self.branch,
            entry,
            parent: parent.into(),
            name: name_policy::LookupName::parse(path.file_name())?,
        })
    }

    /// Until a legacy mutator consumes typed Unborn transitions it must reject
    /// its complete affected subtree, not call a history-only missing lookup.
    pub(crate) fn require_published_subtree(&self, path: Option<&LookupPath>) -> Result<()> {
        for entry in &self.entries {
            let selected = path.is_none_or(|path| {
                entry.path().key() == path.key()
                    || super::is_descendant(entry.path().key(), path.key())
            });
            if selected && matches!(entry, WorkingEntry::Unborn { .. }) {
                return Err(Error::FileDraft(Failure::UnbornRequired));
            }
        }
        Ok(())
    }
}
