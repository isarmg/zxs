//! Scoped logical history projection and closure validation, without storage I/O.
use crate::business::{validate_branch_name, validate_entries};
use crate::{Error, ErrorCode, Result};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::NamespaceEntryKind;
use storage_contract::{
    Branch, Commit, ContentState, DirectoryBranch, DirectoryTree, Entry, FileBranchMerge,
    FileContext, FileMetadataNode, FilePathNode, FileVersion, HistoryBoundary, HistoryTree,
    MergeLink, Placement, PlacementRule, ProtectionPolicy, Repository,
    SUPPORTED_CHECKSUM_CHUNK_BYTES, SharedRecords, State, validate_id,
};
use storage_contract::{BranchKind, BranchState, FileMetadataKind};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryManifest {
    pub repository: Repository,
    pub history: HistoryTree,
    pub branches: BTreeMap<String, Branch>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_selections: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_branches: BTreeMap<String, DirectoryBranch>,
    #[serialization(default, skip_serializing_if = "SharedRecords::is_empty")]
    pub directory_trees: SharedRecords<DirectoryTree>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_write_origins: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_contexts: BTreeMap<String, FileContext>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_path_nodes: BTreeMap<String, FilePathNode>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_metadata_nodes: BTreeMap<String, FileMetadataNode>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_branch_merges: BTreeMap<String, FileBranchMerge>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_branch_aliases: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_node_parents: BTreeMap<String, Vec<String>>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_path_notes: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_tree_selections: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_locations: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_backup_paths: BTreeMap<String, BTreeMap<String, String>>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_backup_policies: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_selections: BTreeMap<String, String>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "storage_contract::u64_string_map"
    )]
    pub directory_selection_generations: BTreeMap<String, u64>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "storage_contract::u64_string_map"
    )]
    pub file_selection_generations: BTreeMap<String, u64>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_notes: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub version_notes: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_names: BTreeMap<String, String>,
    pub commits: SharedRecords<Commit>,
    pub versions: BTreeMap<String, FileVersion>,
    pub placements: BTreeMap<String, Placement>,
    pub merge_links: BTreeMap<String, MergeLink>,
    pub boundary_marks: BTreeMap<String, HistoryBoundary>,
    pub content_states: BTreeMap<String, ContentState>,
    pub placement_rules: BTreeMap<String, PlacementRule>,
    pub protection_policies: BTreeMap<String, ProtectionPolicy>,
}

impl HistoryManifest {
    fn validate_entry_envelope(entry: &Entry) -> Result<()> {
        validate_id(&entry.entry_id, "entry")?;
        match entry.kind {
            NamespaceEntryKind::File => {
                validate_id(&entry.file_id, "file")?;
                validate_id(&entry.version_id, "ver")?;
            }
            NamespaceEntryKind::Directory => {
                if !entry.file_id.is_empty() || !entry.version_id.is_empty() || entry.size != 0 {
                    return Err(Error::new(
                        storage_contract::ErrorCode::HistoryPackageInvalid,
                        "namespace directory payload is invalid",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn from_state(state: &State, history_id: &str) -> Result<Self> {
        let manifest = Self::project_state(state, history_id)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Build the history projection; reference validation belongs to its caller.
    pub(super) fn project_state(state: &State, history_id: &str) -> Result<Self> {
        let history = state.histories.get(history_id).cloned().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::HistoryNotFound,
                "history does not exist",
            )
        })?;
        let repository = state
            .repositories
            .get(&history.repository_id)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "history repository missing",
                )
            })?;
        let repository_id = history.repository_id.clone();
        let directory_ids = state
            .commits
            .values()
            .filter(|commit| commit.history_tree_id == history_id)
            .flat_map(|commit| {
                commit
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                    .map(|entry| entry.entry_id.as_str())
            })
            .chain(std::iter::once(repository_id.as_str()))
            .collect::<BTreeSet<_>>();
        let versions = state
            .versions
            .iter()
            .filter(|(_, v)| v.history_tree_id == history_id)
            .map(|(id, v)| (id.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>();
        let file_ids = versions
            .values()
            .map(|version| version.file_id.as_str())
            .collect::<BTreeSet<_>>();
        let manifest = Self {
            repository,
            history,
            file_names: state
                .file_names
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, name)| (file.clone(), name.clone()))
                .collect(),
            file_locations: state
                .file_locations
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, path)| (file.clone(), path.clone()))
                .collect(),
            file_write_origins: state
                .file_write_origins
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, origin)| (file.clone(), origin.clone()))
                .collect(),
            file_contexts: state
                .file_contexts
                .iter()
                .filter(|(key, _)| key.starts_with(&format!("{repository_id}/")))
                .map(|(key, context)| (key.clone(), context.clone()))
                .collect(),
            file_path_nodes: state
                .file_path_nodes
                .iter()
                .filter(|(_, node)| node.history_tree_id == history_id)
                .map(|(id, node)| (id.clone(), node.clone()))
                .collect(),
            file_metadata_nodes: state
                .file_metadata_nodes
                .iter()
                .filter(|(_, node)| node.history_tree_id == history_id)
                .map(|(id, node)| (id.clone(), node.clone()))
                .collect(),
            file_branch_merges: state
                .file_branch_merges
                .iter()
                .filter(|(_, event)| event.history_tree_id == history_id)
                .map(|(id, event)| (id.clone(), event.clone()))
                .collect(),
            file_branch_aliases: state
                .file_branch_aliases
                .iter()
                .filter(|(id, _)| {
                    state
                        .branches
                        .get(*id)
                        .is_some_and(|branch| branch.history_tree_id == history_id)
                })
                .map(|(id, target)| (id.clone(), target.clone()))
                .collect(),
            file_backup_paths: state
                .file_backup_paths
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, paths)| (file.clone(), paths.clone()))
                .collect(),
            file_backup_policies: state
                .file_backup_policies
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, policy)| (file.clone(), policy.clone()))
                .collect(),
            file_node_parents: state
                .file_node_parents
                .iter()
                .filter(|(version, _)| versions.contains_key(*version))
                .map(|(version, parents)| (version.clone(), parents.clone()))
                .collect(),
            file_path_notes: state
                .file_path_notes
                .iter()
                .filter(|(id, _)| {
                    state
                        .file_path_nodes
                        .get(*id)
                        .is_some_and(|node| node.history_tree_id == history_id)
                        || state
                            .file_metadata_nodes
                            .get(*id)
                            .is_some_and(|node| node.history_tree_id == history_id)
                })
                .map(|(id, note)| (id.clone(), note.clone()))
                .collect(),
            file_selection_generations: state
                .file_selection_generations
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, generation)| (file.clone(), *generation))
                .collect(),
            directory_tree_selections: state
                .directory_tree_selections
                .iter()
                .filter(|(directory, _)| directory_ids.contains(directory.as_str()))
                .map(|(directory, tree)| (directory.clone(), tree.clone()))
                .collect(),
            file_notes: state
                .file_notes
                .iter()
                .filter(|(file, _)| file_ids.contains(file.as_str()))
                .map(|(file, note)| (file.clone(), note.clone()))
                .collect(),
            version_notes: state
                .version_notes
                .iter()
                .filter(|(version, _)| versions.contains_key(*version))
                .map(|(version, note)| (version.clone(), note.clone()))
                .collect(),
            file_selections: state
                .file_selections
                .iter()
                .filter(|(_, id)| {
                    state
                        .branches
                        .get(*id)
                        .is_some_and(|b| b.history_tree_id == history_id)
                })
                .map(|(file, branch)| (file.clone(), branch.clone()))
                .collect(),
            directory_branches: state
                .directory_branches
                .iter()
                .filter(|(_, branch)| branch.repository_id == repository_id)
                .map(|(id, branch)| (id.clone(), branch.clone()))
                .collect(),
            directory_trees: state.directory_trees.subset(|_, tree| {
                state
                    .directory_branches
                    .get(&tree.directory_branch_id)
                    .is_some_and(|branch| branch.repository_id == repository_id)
            }),
            directory_selections: state
                .directory_selections
                .iter()
                .filter(|(_, selected)| {
                    state
                        .directory_branches
                        .get(*selected)
                        .is_some_and(|branch| branch.repository_id == repository_id)
                })
                .map(|(directory, selected)| (directory.clone(), selected.clone()))
                .collect(),
            directory_selection_generations: state
                .directory_selection_generations
                .iter()
                .filter(|(directory, _)| directory_ids.contains(directory.as_str()))
                .map(|(directory, generation)| (directory.clone(), *generation))
                .collect(),
            branches: state
                .branches
                .iter()
                .filter(|(_, b)| b.history_tree_id == history_id)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
            commits: state
                .commits
                .subset(|_, commit| commit.history_tree_id == history_id),
            placements: state
                .placements
                .iter()
                .filter(|(id, _)| versions.contains_key(*id))
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
            content_states: versions
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        state
                            .content_states
                            .get(id)
                            .copied()
                            .unwrap_or(ContentState::Available),
                    )
                })
                .collect(),
            versions,
            merge_links: state
                .merge_links
                .iter()
                .filter(|(_, l)| l.history_tree_id == history_id)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
            boundary_marks: state
                .history_boundaries
                .iter()
                .filter(|(_, b)| b.history_tree_id == history_id)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
            placement_rules: state
                .placement_rules
                .iter()
                .filter(|(_, p)| p.repository_id == repository_id)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
            protection_policies: state
                .protection_policies
                .iter()
                .filter(|(_, p)| p.repository_id == repository_id)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect(),
        };
        Ok(manifest)
    }
    fn validate_directory_snapshot(&self, branch: &DirectoryBranch) -> Result<()> {
        let invalid =
            |message| Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message);
        let history = &self.history.history_tree_id;
        let commit = self
            .commits
            .get(&branch.commit_id)
            .ok_or_else(|| invalid("directory composition commit is outside the history"))?;
        let scope_key = name_policy::path_key(&branch.path);
        if branch.path.is_empty() {
            if branch.directory_id != self.repository.repository_id {
                return Err(invalid(
                    "root directory identity differs from its repository",
                ));
            }
        } else {
            name_policy::validate_path(&branch.path)?;
            validate_id(&branch.directory_id, "entry")?;
            if !commit.entries.iter().any(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && entry.entry_id == branch.directory_id
                    && entry.lookup_key() == scope_key
            }) {
                return Err(invalid(
                    "directory composition stable identity is absent from its snapshot",
                ));
            }
        }
        let prefix = format!("{scope_key}/");
        let mut paths = BTreeSet::new();
        let mut entries = BTreeMap::<&str, &Entry>::new();
        let mut files = BTreeSet::new();
        for entry in &branch.entries {
            Self::validate_entry_envelope(entry)?;
            if (!scope_key.is_empty() && !entry.lookup_key().starts_with(&prefix))
                || entry.entry_id == branch.directory_id
                || !paths.insert(entry.lookup_key())
            {
                return Err(invalid(
                    "directory composition entry escapes its scope or normalized identity",
                ));
            }
            if let Some(previous) = entries.insert(entry.entry_id.as_str(), entry)
                && (entry.kind != NamespaceEntryKind::File
                    || previous.kind != NamespaceEntryKind::File
                    || previous.file_id != entry.file_id)
            {
                return Err(invalid(
                    "directory composition entry identity is reused by different entities",
                ));
            }
            if entry.kind == NamespaceEntryKind::File {
                let version = self
                    .versions
                    .get(&entry.version_id)
                    .ok_or_else(|| invalid("directory composition version is missing"))?;
                let shared = branch
                    .file_branches
                    .get(&entry.file_id)
                    .and_then(|id| self.branches.get(id))
                    .ok_or_else(|| {
                        invalid("directory composition shared file branch is missing")
                    })?;
                files.insert(entry.file_id.as_str());
                if version.history_tree_id != *history
                    || version.file_id != entry.file_id
                    || version.logical_size != entry.size
                    || shared.history_tree_id != *history
                    || shared.branch_kind != BranchKind::File
                    || shared.file_id.as_deref() != Some(entry.file_id.as_str())
                {
                    return Err(invalid(
                        "directory composition file, version or shared branch differs",
                    ));
                }
            }
        }
        if branch.file_branches.len() != files.len() {
            return Err(invalid(
                "directory composition file branch coverage differs",
            ));
        }
        let file_entries = branch
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .collect::<Vec<_>>();
        if branch.file_contexts.len() != file_entries.len() {
            return Err(invalid("directory path context coverage differs"));
        }
        for entry in file_entries {
            let context = branch
                .file_contexts
                .get(entry.lookup_key())
                .ok_or_else(|| invalid("directory path write context is missing"))?;
            self.validate_file_context(context, &entry.file_id, &entry.version_id)?;
        }
        for (file, origin) in &branch.inherited_files {
            if !branch.file_branches.contains_key(file)
                || !self
                    .directory_branches
                    .get(origin)
                    .is_some_and(|origin| origin.repository_id == branch.repository_id)
            {
                return Err(invalid(
                    "inherited write intent escapes its directory history or file scope",
                ));
            }
        }
        for (directory, reference) in &branch.directory_refs {
            let selected = self
                .directory_branches
                .get(&reference.directory_branch_id)
                .ok_or_else(|| invalid("directory composition child branch is missing"))?;
            let tree = self
                .directory_trees
                .get(&reference.tree_id)
                .ok_or_else(|| invalid("directory composition child tree is missing"))?;
            let owner = self
                .directory_branches
                .get(&tree.directory_branch_id)
                .ok_or_else(|| invalid("directory composition child tree owner is missing"))?;
            if !branch.entries.iter().any(|entry| {
                entry.kind == NamespaceEntryKind::Directory && &entry.entry_id == directory
            }) || selected.directory_id != *directory
                || owner.directory_id != *directory
                || selected.repository_id != branch.repository_id
                || owner.repository_id != branch.repository_id
                || !self.directory_branch_inherits(
                    &reference.directory_branch_id,
                    &tree.directory_branch_id,
                )
            {
                return Err(invalid(
                    "directory composition child reference escapes its stable scope",
                ));
            }
        }
        Ok(())
    }
    fn directory_branch_inherits(&self, selected: &str, ancestor: &str) -> bool {
        let mut cursor = Some(selected);
        let mut seen = BTreeSet::new();
        while let Some(id) = cursor {
            if id == ancestor {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            cursor = self
                .directory_branches
                .get(id)
                .and_then(|branch| branch.parent_branch_id.as_deref());
        }
        false
    }
    fn validate_file_context(
        &self,
        context: &FileContext,
        file_id: &str,
        version_id: &str,
    ) -> Result<()> {
        let invalid =
            |message| Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message);
        let branch = self
            .branches
            .get(&context.branch_id)
            .ok_or_else(|| invalid("file path context branch is missing"))?;
        if branch.branch_kind != BranchKind::File
            || branch.file_id.as_deref() != Some(file_id)
            || branch.history_tree_id != self.history.history_tree_id
        {
            return Err(invalid(
                "file path context belongs to another file or history",
            ));
        }
        if context
            .inherited_from
            .as_ref()
            .is_some_and(|origin| !self.directory_branches.contains_key(origin))
        {
            return Err(invalid("file path inherited context origin is missing"));
        }
        if let Some(node) = &context.node_id {
            let body = self.file_node_version(node)?;
            if body.file_id != file_id || body.version_id != version_id {
                return Err(invalid(
                    "file path node differs from its frozen content reference",
                ));
            }
        }
        Ok(())
    }
    fn file_node_version(&self, node_id: &str) -> Result<&FileVersion> {
        self.versions
            .get(node_id)
            .or_else(|| {
                self.file_path_nodes
                    .get(node_id)
                    .and_then(|node| self.versions.get(&node.version_id))
            })
            .or_else(|| {
                self.file_metadata_nodes
                    .get(node_id)
                    .and_then(|node| self.versions.get(&node.version_id))
            })
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::HistoryPackageInvalid,
                    "file history node or its body reference is missing",
                )
            })
    }
    fn validate_file_nodes(&self) -> Result<()> {
        let invalid =
            |message| Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message);
        let mut graph = BTreeMap::<&str, Vec<String>>::new();
        for (id, version) in &self.versions {
            graph.insert(
                id,
                self.file_node_parents
                    .get(id)
                    .unwrap_or(&version.source_version_ids)
                    .clone(),
            );
        }
        for (id, node) in &self.file_path_nodes {
            validate_id(id, "fnode")?;

            name_policy::validate_path(&node.path)?;
            name_policy::validate_path(&node.previous_path)?;
            let body = self
                .versions
                .get(&node.version_id)
                .ok_or_else(|| invalid("path event body version is missing"))?;
            let branch = self
                .branches
                .get(&node.branch_id)
                .ok_or_else(|| invalid("path event public branch is missing"))?;
            if node.node_id != *id
                || node.history_tree_id != self.history.history_tree_id
                || body.file_id != node.file_id
                || body.history_tree_id != node.history_tree_id
                || branch.file_id.as_deref() != Some(node.file_id.as_str())
                || branch.branch_kind != BranchKind::File
                || branch.history_tree_id != node.history_tree_id
                || node.author.is_empty()
                || node.path == node.previous_path
                || node.parent_node_ids.is_empty()
            {
                return Err(invalid(
                    "file path event identity, scope or metadata differs",
                ));
            }
            for parent in &node.parent_node_ids {
                let parent = self.file_node_version(parent)?;
                if parent.version_id != node.version_id {
                    return Err(invalid("rename event changes its body content"));
                }
            }
            graph.insert(id, node.parent_node_ids.clone());
        }
        for (id, node) in &self.file_metadata_nodes {
            validate_id(id, "fmeta")?;

            let body = self
                .versions
                .get(&node.version_id)
                .ok_or_else(|| invalid("file metadata body version is missing"))?;
            let branch = self
                .branches
                .get(&node.branch_id)
                .ok_or_else(|| invalid("file metadata public branch is missing"))?;
            if node.node_id != *id
                || node.history_tree_id != self.history.history_tree_id
                || body.file_id != node.file_id
                || body.history_tree_id != node.history_tree_id
                || branch.file_id.as_deref() != Some(node.file_id.as_str())
                || branch.branch_kind != BranchKind::File
                || branch.history_tree_id != node.history_tree_id
                || node.author.is_empty()
                || node.parent_node_ids.is_empty()
                || node.tags.len() > storage_contract::MAX_FILE_TAGS
                || node
                    .tags
                    .iter()
                    .any(|tag| storage_contract::validate_tag(tag).is_err())
            {
                return Err(invalid("file metadata identity, scope or labels differ"));
            }
            if node.kind == FileMetadataKind::Tags
                && (node.parent_node_ids.len() != 1
                    || self.file_node_version(&node.parent_node_ids[0])?.version_id
                        != node.version_id)
            {
                return Err(invalid("tag event changes its body content"));
            }
            graph.insert(id, node.parent_node_ids.clone());
        }
        for (id, event) in &self.file_branch_merges {
            validate_id(id, "fmerge")?;

            let result = self.file_node_version(&event.result_node_id)?;
            let chosen = self.file_node_version(&event.chosen_node_id)?;
            let inputs = event.input_branch_ids.iter().collect::<BTreeSet<_>>();
            if event.merge_id != *id
                || event.history_tree_id != self.history.history_tree_id
                || result.file_id != event.file_id
                || result.history_tree_id != event.history_tree_id
                || chosen.version_id != result.version_id
                || inputs.len() < 2
                || inputs.len() != event.input_branch_ids.len()
                || event.input_branch_ids.len() != event.input_node_ids.len()
                || !inputs.contains(&event.surviving_branch_id)
                || event.reused_node != event.input_node_ids.contains(&event.chosen_node_id)
                || event.reused_node != (event.result_node_id == event.chosen_node_id)
            {
                return Err(invalid("file merge provenance identity or result differs"));
            }
            if !event.reused_node && !event.parent_node_ids.contains(&event.chosen_node_id) {
                return Err(invalid(
                    "file merge chosen source is absent from its ancestry",
                ));
            }
            for (branch, node) in event.input_branch_ids.iter().zip(&event.input_node_ids) {
                let branch = self
                    .branches
                    .get(branch)
                    .ok_or_else(|| invalid("file merge input branch is missing"))?;
                let node = self.file_node_version(node)?;
                if branch.branch_kind != BranchKind::File
                    || branch.file_id.as_deref() != Some(event.file_id.as_str())
                    || branch.history_tree_id != event.history_tree_id
                    || node.file_id != event.file_id
                    || node.history_tree_id != event.history_tree_id
                {
                    return Err(invalid("file merge input escapes its file or history"));
                }
            }
            if !event.reused_node
                && !self
                    .file_metadata_nodes
                    .get(&event.result_node_id)
                    .is_some_and(|node| {
                        node.kind == FileMetadataKind::Merge
                            && node.parent_node_ids == event.parent_node_ids
                            && node.branch_id == event.surviving_branch_id
                    })
            {
                return Err(invalid("file merge revision metadata differs"));
            }
            let permitted = event
                .input_node_ids
                .iter()
                .chain(std::iter::once(&event.chosen_node_id))
                .collect::<BTreeSet<_>>();
            let mut unique = BTreeSet::new();
            for parent in &event.parent_node_ids {
                if !permitted.contains(parent)
                    || parent == &event.result_node_id
                    || !unique.insert(parent)
                {
                    return Err(invalid("file merge graph edge is not a unique input"));
                }
                let parents = graph
                    .get_mut(event.result_node_id.as_str())
                    .ok_or_else(|| invalid("file merge result graph node is missing"))?;
                if !parents.contains(parent) {
                    parents.push(parent.clone());
                }
            }
        }
        for (source, target) in &self.file_branch_aliases {
            let original = self
                .branches
                .get(source)
                .ok_or_else(|| invalid("merged file alias source is missing"))?;
            if original.branch_kind != BranchKind::File
                || original.state != BranchState::Merged
                || source == target
            {
                return Err(invalid("merged file alias source is invalid"));
            }
            let mut cursor = target;
            let mut seen = BTreeSet::from([source]);
            loop {
                if !seen.insert(cursor) {
                    return Err(invalid("merged file aliases contain a cycle"));
                }
                let branch = self
                    .branches
                    .get(cursor)
                    .ok_or_else(|| invalid("merged file alias target is missing"))?;
                if branch.branch_kind != BranchKind::File
                    || branch.file_id != original.file_id
                    || branch.history_tree_id != original.history_tree_id
                {
                    return Err(invalid("merged file alias escapes its file or history"));
                }
                match self.file_branch_aliases.get(cursor) {
                    Some(next) => cursor = next,
                    None => {
                        if branch.state != BranchState::Active {
                            return Err(invalid("merged file alias final target is not active"));
                        }
                        break;
                    }
                }
            }
        }
        for (version, parents) in &self.file_node_parents {
            let body = self
                .versions
                .get(version)
                .ok_or_else(|| invalid("file node ancestry override has no real version"))?;
            let resolved = parents
                .iter()
                .map(|parent| {
                    self.file_node_version(parent)
                        .map(|version| version.version_id.as_str())
                })
                .collect::<Result<BTreeSet<_>>>()?;
            if resolved != body.source_version_ids.iter().map(String::as_str).collect() {
                return Err(invalid(
                    "file node ancestry override differs from immutable body sources",
                ));
            }
        }
        let mut dependencies = BTreeMap::new();
        let mut children = BTreeMap::<&str, Vec<&str>>::new();
        let mut ready = VecDeque::new();
        for (id, parents) in &graph {
            let body = self.file_node_version(id)?;
            let mut unique = BTreeSet::new();
            for parent in parents {
                let previous = self.file_node_version(parent)?;
                if !unique.insert(parent.as_str())
                    || previous.file_id != body.file_id
                    || previous.history_tree_id != body.history_tree_id
                    || !graph.contains_key(parent.as_str())
                {
                    return Err(invalid(
                        "file history parent repeats or escapes its stable file/history",
                    ));
                }
                children.entry(parent).or_default().push(id);
            }
            dependencies.insert(*id, parents.len());
            if parents.is_empty() {
                ready.push_back(*id);
            }
        }
        let mut resolved = 0usize;
        while let Some(id) = ready.pop_front() {
            resolved += 1;
            for child in children.get(id).into_iter().flatten() {
                let remaining = dependencies.get_mut(child).expect("graph child validated");
                *remaining -= 1;
                if *remaining == 0 {
                    ready.push_back(child);
                }
            }
        }
        if resolved != graph.len() {
            return Err(invalid(
                "file history contains a mixed content/path node cycle",
            ));
        }
        if self.file_path_notes.iter().any(|(id, note)| {
            (!self.file_path_nodes.contains_key(id) && !self.file_metadata_nodes.contains_key(id))
                || note.len() > storage_contract::MAX_NOTE_BYTES
                || note.chars().any(char::is_control)
        }) {
            return Err(invalid(
                "file path notes escape their node scope or exceed the byte limit",
            ));
        }
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<()> {
        let invalid =
            |message| Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message);
        let history = &self.history.history_tree_id;
        validate_id(history, "history")?;
        validate_id(&self.repository.repository_id, "repo")?;
        if self.history.repository_id != self.repository.repository_id
            || self.repository.history_tree_id != *history
            || !self.commits.contains_key(&self.history.root_commit_id)
        {
            return Err(invalid("manifest root or repository binding differs"));
        }
        name_policy::validate_name(&self.repository.name)?;
        if self.repository.name_policy != name_policy::NAME_POLICY
            || self.repository.unicode_version != name_policy::UNICODE_VERSION
        {
            return Err(invalid("manifest uses an unsupported naming policy"));
        }
        for (id, commit) in self.commits.iter() {
            validate_id(id, "commit")?;
            if commit.commit_id != *id
                || commit.history_tree_id != *history
                || commit
                    .parent_commit_ids
                    .iter()
                    .any(|p| !self.commits.contains_key(p))
            {
                return Err(invalid("commit identity or parent closure differs"));
            }
            if (id == &self.history.root_commit_id) != commit.parent_commit_ids.is_empty()
                || commit.parent_commit_ids.len() > 1
            {
                return Err(invalid(
                    "history must have one root and single-parent commits",
                ));
            }
            if let Some(deletions) = &commit.namespace_deletions {
                // Operation facts are not a namespace snapshot: parents can remain
                // live, and sequential edits can delete the same path more than once.
                if deletions.paths.is_empty() {
                    return Err(invalid("namespace deletion facts are empty"));
                }
                crate::disk::encode_metadata(deletions, crate::disk::MAX_METADATA)?;
                let timestamp = i64::try_from(deletions.timestamp).map_err(|source| {
                    Error::from(source).context(
                        ErrorCode::HistoryPackageInvalid,
                        "namespace deletion timestamp is invalid",
                    )
                })?;
                date_time::OffsetDateTime::from_unix_timestamp(timestamp).map_err(|source| {
                    Error::from(source).context(
                        ErrorCode::HistoryPackageInvalid,
                        "namespace deletion timestamp is invalid",
                    )
                })?;
                for deletion in &deletions.paths {
                    name_policy::validate_path(&deletion.path).map_err(|source| {
                        Error::from(source).context(
                            ErrorCode::HistoryPackageInvalid,
                            "namespace deletion path is invalid",
                        )
                    })?;
                }
            }
            let mut entries = commit.entries.clone();
            validate_entries(&mut entries, 1_000_000)?;
            if entries != commit.entries {
                return Err(invalid(
                    "commit entries are not in canonical namespace form",
                ));
            }
            for entry in &commit.entries {
                Self::validate_entry_envelope(entry)?;
                if entry.kind != NamespaceEntryKind::File {
                    continue;
                }
                let version = self
                    .versions
                    .get(&entry.version_id)
                    .ok_or_else(|| invalid("entry version missing"))?;
                if version.file_id != entry.file_id || version.logical_size != entry.size {
                    return Err(invalid("entry version identity or size differs"));
                }
            }
            for (file, source) in &commit.pending_file_versions {
                validate_id(file, "file")?;
                validate_id(source, "ver")?;
                let version = self
                    .versions
                    .get(source)
                    .ok_or_else(|| invalid("pending snapshot source version is missing"))?;
                if version.file_id != *file
                    || version.history_tree_id != *history
                    || !commit.entries.iter().any(|entry| {
                        entry.kind == NamespaceEntryKind::File
                            && entry.file_id == *file
                            && entry.version_id == *source
                    })
                {
                    return Err(invalid(
                        "pending snapshot source escapes its file or entries",
                    ));
                }
            }
        }
        // Reachability also rejects cycles and detached subgraphs.
        let reached = crate::business::descendant_commits(
            self.commits.values(),
            &self.history.root_commit_id,
        );
        if reached.len() != self.commits.len() {
            return Err(invalid("history contains a cycle or detached commits"));
        }
        for (id, branch) in &self.branches {
            validate_id(id, "br")?;
            validate_branch_name(&branch.name)?;
            if branch.branch_id != *id
                || branch.history_tree_id != *history
                || !self.commits.contains_key(&branch.head_commit_id)
                || !self.commits.contains_key(&branch.fork_commit_id)
            {
                return Err(invalid("branch has an external live commit reference"));
            }
            if let Some(file) = branch.file_id.as_deref() {
                let head = &self.commits[&branch.head_commit_id];
                if branch.branch_kind != BranchKind::File
                    || head
                        .entries
                        .iter()
                        .filter(|e| e.kind == NamespaceEntryKind::File)
                        .count()
                        != 1
                    || !head
                        .entries
                        .iter()
                        .any(|e| e.kind == NamespaceEntryKind::File && e.file_id == file)
                {
                    return Err(invalid("file branch head escapes its stable file identity"));
                }
            }
            if let Some(node) = &branch.file_node_id {
                let body = self.file_node_version(node)?;
                if branch.file_id.as_deref() != Some(body.file_id.as_str())
                    || !self.commits[&branch.head_commit_id]
                        .entries
                        .iter()
                        .any(|entry| {
                            entry.kind == NamespaceEntryKind::File
                                && entry.version_id == body.version_id
                        })
                {
                    return Err(invalid("file branch node differs from its HEAD content"));
                }
            }
            for (file, source) in &branch.pending_file_versions {
                validate_id(file, "file")?;
                validate_id(source, "ver")?;
                let version = self
                    .versions
                    .get(source)
                    .ok_or_else(|| invalid("pending branch source version is missing"))?;
                if version.file_id != *file
                    || version.history_tree_id != *history
                    || branch.file_id.as_ref().is_some_and(|id| id != file)
                    || !self.commits[&branch.head_commit_id]
                        .entries
                        .iter()
                        .any(|entry| {
                            entry.kind == NamespaceEntryKind::File
                                && entry.file_id == *file
                                && entry.version_id == *source
                        })
                {
                    return Err(invalid(
                        "pending branch source escapes its file or HEAD snapshot",
                    ));
                }
            }
        }
        for (file, branch) in &self.file_selections {
            if !self.branches.get(branch).is_some_and(|b| {
                b.file_id.as_deref() == Some(file.as_str()) && b.state == BranchState::Active
            }) {
                return Err(invalid(
                    "selected file branch is missing, inactive or belongs to another file",
                ));
            }
        }
        for (id, branch) in &self.directory_branches {
            validate_id(id, "dbr")?;
            validate_branch_name(&branch.name)?;
            if branch.directory_branch_id != *id
                || branch.repository_id != self.repository.repository_id
            {
                return Err(invalid(
                    "directory composition identity or repository differs",
                ));
            }
            // Parents are composition records within the same stable directory;
            // their historical paths may differ after a directory rename.
            if let Some(parent_id) = branch.parent_branch_id.as_deref() {
                let previous = self
                    .directory_branches
                    .get(parent_id)
                    .ok_or_else(|| invalid("directory composition parent is missing"))?;
                if previous.repository_id != branch.repository_id
                    || previous.directory_id != branch.directory_id
                {
                    return Err(invalid(
                        "directory composition parent escapes its stable directory",
                    ));
                }
            }
        }
        // Resolve each edge once rather than walking every ancestor again for
        // every retained composition in a long directory history.
        let mut resolved = BTreeSet::new();
        for id in self.directory_branches.keys() {
            let mut path = BTreeSet::new();
            let mut cursor = Some(id.as_str());
            while let Some(id) = cursor {
                if resolved.contains(id) {
                    break;
                }
                if !path.insert(id) {
                    return Err(invalid("directory composition parent cycle"));
                }
                cursor = self.directory_branches[id].parent_branch_id.as_deref();
            }
            resolved.extend(path);
        }
        for (id, tree) in self.directory_trees.iter() {
            validate_id(id, "dtree")?;
            let branch = self
                .directory_branches
                .get(&tree.directory_branch_id)
                .ok_or_else(|| invalid("directory tree owner branch is missing"))?;
            if tree.tree_id != *id {
                return Err(invalid("directory tree identity differs"));
            }
            let mut snapshot = branch.clone();
            snapshot.commit_id = tree.commit_id.clone();
            snapshot.path = tree.path.clone();
            snapshot.entries = tree.entries.clone();
            snapshot.file_branches = tree.file_branches.clone();
            snapshot.file_contexts = tree.file_contexts.clone();
            snapshot.inherited_files = tree.inherited_files.clone();
            snapshot.directory_refs = tree.directory_refs.clone();
            self.validate_directory_snapshot(&snapshot)?;
            if let Some(parent) = &tree.parent_tree_id {
                let previous = self
                    .directory_trees
                    .get(parent)
                    .and_then(|tree| self.directory_branches.get(&tree.directory_branch_id))
                    .ok_or_else(|| invalid("directory tree parent or its branch is missing"))?;
                if previous.repository_id != branch.repository_id
                    || previous.directory_id != branch.directory_id
                {
                    return Err(invalid(
                        "directory tree parent escapes its stable directory",
                    ));
                }
            }
        }
        let mut resolved = BTreeSet::new();
        for id in self.directory_trees.keys() {
            let mut path = BTreeSet::new();
            let mut cursor = Some(id.as_str());
            while let Some(id) = cursor {
                if resolved.contains(id) {
                    break;
                }
                if !path.insert(id) {
                    return Err(invalid("directory tree parent cycle"));
                }
                cursor = self.directory_trees[id].parent_tree_id.as_deref();
            }
            resolved.extend(path);
        }
        for (id, branch) in &self.directory_branches {
            let head = self
                .directory_trees
                .get(&branch.head_tree_id)
                .ok_or_else(|| invalid("directory branch HEAD tree is missing"))?;
            let owner = &self.directory_branches[&head.directory_branch_id];
            if owner.directory_id != branch.directory_id
                || owner.repository_id != branch.repository_id
                || branch.commit_id != head.commit_id
                || branch.path != head.path
                || branch.file_branches != head.file_branches
                || branch.directory_refs != head.directory_refs
                || branch.entries != head.entries
            {
                return Err(invalid(
                    "directory branch projection differs from its HEAD tree",
                ));
            }
            if branch.materialized {
                if head.directory_branch_id != *id
                    || branch.file_contexts != head.file_contexts
                    || branch.sequence != head.sequence
                    || branch.inherited_files != head.inherited_files
                {
                    return Err(invalid(
                        "completed directory branch HEAD belongs to another branch",
                    ));
                }
            } else {
                self.validate_directory_snapshot(branch)?;
                if branch.sequence < head.sequence {
                    return Err(invalid(
                        "directory branch registration precedes its inherited tree",
                    ));
                }
                let mut parent = branch.parent_branch_id.as_deref();
                let mut inherited = false;
                while let Some(parent_id) = parent {
                    if parent_id == head.directory_branch_id {
                        inherited = true;
                        break;
                    }
                    parent = self.directory_branches[parent_id]
                        .parent_branch_id
                        .as_deref();
                }
                if !inherited {
                    return Err(invalid(
                        "unwritten directory branch HEAD is not inherited from its parent",
                    ));
                }
            }
        }
        for (directory, selected) in &self.directory_selections {
            if !self
                .directory_branches
                .get(selected)
                .is_some_and(|branch| branch.directory_id == *directory)
            {
                return Err(invalid(
                    "selected directory composition is missing or belongs to another directory",
                ));
            }
        }
        for (directory, tree) in &self.directory_tree_selections {
            let tree = self
                .directory_trees
                .get(tree)
                .ok_or_else(|| invalid("selected directory tree is missing"))?;
            if !self
                .directory_branches
                .get(&tree.directory_branch_id)
                .is_some_and(|branch| &branch.directory_id == directory)
                || !self.directory_selections.contains_key(directory)
                || !self.directory_branch_inherits(
                    &self.directory_selections[directory],
                    &tree.directory_branch_id,
                )
            {
                return Err(invalid(
                    "selected directory tree escapes its directory scope",
                ));
            }
        }
        let directory_ids = self
            .commits
            .values()
            .flat_map(|commit| {
                commit
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                    .map(|entry| entry.entry_id.as_str())
            })
            .collect::<BTreeSet<_>>();
        for directory in self.directory_selection_generations.keys() {
            if directory == &self.repository.repository_id {
                continue;
            }
            validate_id(directory, "entry")?;
            if !directory_ids.contains(directory.as_str()) {
                return Err(invalid(
                    "directory selection generation is outside the history",
                ));
            }
        }
        let file_ids = self
            .versions
            .values()
            .map(|version| version.file_id.as_str())
            .collect::<BTreeSet<_>>();
        for (file, origin) in &self.file_write_origins {
            if !file_ids.contains(file.as_str()) || !self.directory_branches.contains_key(origin) {
                return Err(invalid("current inherited file intent escapes the history"));
            }
        }
        let prefix = format!("{}/", self.repository.repository_id);
        let live = self
            .branches
            .get(&self.repository.main_branch_id)
            .and_then(|branch| self.commits.get(&branch.head_commit_id))
            .ok_or_else(|| invalid("current namespace branch or commit is missing"))?;
        for (key, context) in &self.file_contexts {
            let path = key
                .strip_prefix(&prefix)
                .ok_or_else(|| invalid("file path context escapes its repository"))?;
            name_policy::validate_path(path)?;
            let entry = live
                .entries
                .iter()
                .find(|entry| entry.kind == NamespaceEntryKind::File && entry.lookup_key() == path)
                .ok_or_else(|| invalid("current file path context has no namespace entry"))?;
            self.validate_file_context(context, &entry.file_id, &entry.version_id)?;
        }
        if self
            .file_selection_generations
            .keys()
            .any(|file| !file_ids.contains(file.as_str()))
        {
            return Err(invalid("file selection generation escapes the history"));
        }
        for (file, location) in &self.file_locations {
            if !file_ids.contains(file.as_str()) {
                return Err(invalid("canonical file location escapes the history"));
            }
            name_policy::validate_path(location)?;
            if self.file_names.get(file).map(String::as_str) != location.rsplit('/').next() {
                return Err(invalid(
                    "canonical file name differs from its physical location",
                ));
            }
        }
        if self.file_notes.iter().any(|(file, note)| {
            storage_contract::validate_note(note).is_err() || !file_ids.contains(file.as_str())
        }) || self.version_notes.iter().any(|(version, note)| {
            storage_contract::validate_note(note).is_err() || !self.versions.contains_key(version)
        }) {
            return Err(invalid(
                "file or version notes escape the history or violate the current 128-byte single-line format",
            ));
        }
        for (file, name) in &self.file_names {
            if !file_ids.contains(file.as_str()) {
                return Err(invalid(
                    "container name references a file outside the history",
                ));
            }
            name_policy::validate_name(name)?;
        }
        if !self.branches.contains_key(&self.repository.main_branch_id)
            || self
                .history
                .main_branch_id
                .as_ref()
                .is_some_and(|id| !self.branches.contains_key(id))
        {
            return Err(invalid("main branch missing"));
        }
        if self.branches[&self.repository.main_branch_id]
            .file_id
            .is_some()
        {
            return Err(invalid("directory main branch is scoped to a file"));
        }
        if self.versions.len() != self.placements.len()
            || self.versions.len() != self.content_states.len()
        {
            return Err(invalid("version state or placement coverage differs"));
        }
        for (id, version) in &self.versions {
            // Tombstones retain identity even without a payload to trigger
            // content_path validation. Reject malformed retained records before
            // import can reach a decision or post-decision name projection.
            crate::container_storage::validate_version(version).map_err(|error| {
                let message = format!("invalid version storage identity: {error}");
                error.context(ErrorCode::HistoryPackageInvalid, message)
            })?;
            validate_id(id, "ver")?;
            validate_id(&version.file_id, "file")?;
            name_policy::validate_name(&version.creation_name)?;
            name_policy::validate_name(&version.storage_name)?;
            let placement = self
                .placements
                .get(id)
                .ok_or_else(|| invalid("version placement missing"))?;
            if version.version_id != *id
                || version.history_tree_id != *history
                || placement.version_id != *id
                || !self.content_states.contains_key(id)
            {
                return Err(invalid("version, placement or tombstone identity differs"));
            }
            if !SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&version.checksum_chunk_bytes)
                || version.checksum_manifest_hash.len() != 64
                || !version
                    .checksum_manifest_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid("version checksum metadata is invalid"));
            }
        }
        self.validate_file_nodes()?;
        for (id, link) in &self.merge_links {
            validate_id(id, "link")?;
            if link.merge_link_id != *id
                || link.history_tree_id != *history
                || !self.branches.contains_key(&link.source_branch_id)
                || !self.branches.contains_key(&link.target_branch_id)
                || !self.commits.contains_key(&link.source_commit_id)
                || !self.commits.contains_key(&link.target_commit_id)
            {
                return Err(invalid("merge link crosses a live history boundary"));
            }
        }
        for (id, boundary) in &self.boundary_marks {
            validate_id(id, "boundary")?;
            if boundary.boundary_id != *id
                || boundary.history_tree_id != *history
                || !self.commits.contains_key(&boundary.local_commit_id)
                || boundary
                    .local_branch_id
                    .as_ref()
                    .is_some_and(|id| !self.branches.contains_key(id))
            {
                return Err(invalid("boundary local reference missing"));
            }
            validate_id(&boundary.external_history_tree_id, "history")?;
            validate_id(&boundary.external_commit_id, "commit")?;
        }
        for (id, rule) in &self.placement_rules {
            validate_id(id, "rule")?;
            if id != &rule.rule_id || rule.repository_id != self.repository.repository_id {
                return Err(invalid("placement rule binding differs"));
            }
            if !rule.path_prefix.is_empty() {
                name_policy::validate_path(&rule.path_prefix)?;
            }
        }
        for (file, paths) in &self.file_backup_paths {
            if !file_ids.contains(file.as_str()) {
                return Err(invalid("backup location escapes the history"));
            }
            for (slot, path) in paths {
                name_policy::validate_slot(slot)?;
                name_policy::validate_path(path)?;
            }
        }
        for (file, policy) in &self.file_backup_policies {
            if !file_ids.contains(file.as_str()) || !self.protection_policies.contains_key(policy) {
                return Err(invalid("backup binding escapes its history or policy"));
            }
        }
        let bound_policies = self
            .file_backup_policies
            .values()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let mut inheritable_scopes = BTreeSet::new();
        for (id, policy) in &self.protection_policies {
            validate_id(id, "policy")?;
            if id != &policy.policy_id || policy.repository_id != self.repository.repository_id {
                return Err(invalid("protection policy binding differs"));
            }
            if !policy.path_prefix.is_empty() {
                name_policy::validate_path(&policy.path_prefix)?;
            }
            if !policy.inheritable && !bound_policies.contains(id.as_str()) {
                return Err(invalid("retained backup settings have no file binding"));
            }
            let scope = policy
                .scope_directory_id
                .as_deref()
                .map(|id| ("directory", id))
                .or_else(|| policy.scope_file_id.as_deref().map(|id| ("file", id)));
            if policy.inheritable && scope.is_some_and(|scope| !inheritable_scopes.insert(scope)) {
                return Err(invalid("backup scope has multiple inheritable policies"));
            }
            if policy.target_copies == 0
                || (policy.scope_file_id.is_some() && policy.scope_directory_id.is_some())
                || policy
                    .scope_file_id
                    .as_ref()
                    .is_some_and(|file| !file_ids.contains(file.as_str()))
                || policy.scope_directory_id.as_ref().is_some_and(|directory| {
                    directory != &self.repository.repository_id
                        && !self
                            .commits
                            .values()
                            .flat_map(|commit| &commit.entries)
                            .any(|entry| {
                                entry.kind == NamespaceEntryKind::Directory
                                    && entry.entry_id == *directory
                            })
                })
            {
                return Err(invalid("backup policy scope or count differs"));
            }
            for (slot, prefix) in &policy.backup_prefixes {
                name_policy::validate_slot(slot)?;
                if !prefix.is_empty() {
                    name_policy::validate_path(prefix)?;
                }
            }
        }
        Ok(())
    }
}
