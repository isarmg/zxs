//! Derive the eligible occurrence from the previous registered owner, the
//! actual version/placement, and the new immutable commit. No supplied
//! attribution or current filename can select this subject.
use super::*;
use crate::{BranchKind, BranchState, ContentState, NamespaceEntryKind, file_container::NamespaceDirectoryParent};
use storage_contract::{
    BranchId, CommitId, ContentHash, Generation, HistoryTreeId, VersionId,
    working_tree::{BindingLifecycle, RootId},
};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DirectoryFence {
    pub(super) entry: Option<EntryId>,
    pub(super) selected: Option<String>,
    pub(super) generation: Generation,
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Fences {
    pub(super) file_branch: Option<BranchId>,
    pub(super) file_generation: Generation,
    pub(super) context: Option<crate::FileContext>,
    pub(super) directories: Vec<DirectoryFence>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Binding {
    pub(super) target: DraftLocation,
    pub(super) root: RootId,
    pub(super) owner: DiskId,
    pub(super) slot: String,
    pub(super) binding_generation: Generation,
    pub(super) ancestors: Vec<EntryId>,
    pub(super) history: HistoryTreeId,
    pub(super) branch: BranchId,
    pub(super) head: CommitId,
    pub(super) branch_generation: Generation,
    pub(super) base_node: FileNodeId,
    pub(super) base_version: VersionId,
    pub(super) fences: Fences,
    pub(super) node: FileNodeId,
    pub(super) hash: ContentHash,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) size: u64,
    pub(super) topology: Generation,
    pub(super) operation: OperationId,
    pub(super) sequence: Sequence,
    pub(super) published_at: Timestamp,
}

pub(super) struct Subject { pub(super) binding: Binding }

fn time(result: &OperationResult) -> Result<Timestamp> {
    Ok(Timestamp::from_unix_seconds(storage_contract::parse_u64_decimal(
        result.result.get("timestamp").and_then(json_codec::Value::as_str).ok_or_else(invalid)?,
    )?))
}

impl Binding {
    pub(super) fn node(&self) -> VerifiedWorkspaceNode {
        VerifiedWorkspaceNode {
            target: self.target.clone(), owner: self.owner, ancestors: self.ancestors.clone(),
            node: self.node, operation: self.operation, sequence: self.sequence,
            published_at: self.published_at,
        }
    }

    pub(super) fn require_retained(
        &self, state: &State, result: &OperationResult, actor: &str,
    ) -> Result<()> {
        let FileNodeId::Version(version) = self.node else { return Err(invalid()) };
        let generation = self.branch_generation.get().checked_add(1).ok_or_else(invalid)?;
        let protection = crate::persist::file_draft::Protection::single_verified(
            version, self.slot.parse()?, self.owner, self.topology.get(), self.binding_generation.get(),
        );
        let version = state.versions.get(&version.to_string()).ok_or_else(invalid)?;
        let commit = state.commits.get(result.commit_id.as_deref().ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        if result.commit_state != storage_contract::CommitState::Published
            || result.protection_at_publish != storage_contract::ProtectionAtPublish::TargetMet
            || result.current_health != storage_contract::CurrentHealth::Healthy
            || result.result.get("version_protection") != Some(&json_codec::json!([protection]))
            || result.result.get("branch_generation").and_then(json_codec::Value::as_str)
                .map(storage_contract::parse_u64_decimal).transpose()?
                != Some(generation)
            || self.operation != result.operation_id || self.sequence.get() != result.sequence
            || time(result)? != self.published_at || commit.operation_id != self.operation
            || commit.kind != "content" || commit.history_tree_id != self.history.to_string()
            || commit.entries.iter().filter(|e| e.kind == NamespaceEntryKind::File).count() != 1
            || crate::business::file_nodes::base_parents(state, &version.version_id)
                != [self.base_node.to_string()]
            || commit.author != actor || commit.sequence != result.sequence
            || commit.parent_commit_ids.as_slice() != [self.head.to_string()]
            || version.file_id != self.target.file.to_string() || version.history_tree_id != self.history.to_string()
            || self.node.to_string() != version.version_id || version.version_sequence != result.sequence
            || version.content_hash != self.hash || version.logical_size != self.size
            || version.creation_name != self.target.path.file_name()
            || version.created_as_full_checkpoint || version.creation_primary_slot_id != self.slot
            || version.source_version_ids.as_slice() != [self.base_version.to_string()]
            || result.branch_id.as_deref() != Some(self.branch.to_string().as_str())
            || result.history_tree_id.as_deref() != Some(version.history_tree_id.as_str())
            || !commit.entries.iter().any(|entry| entry.kind == NamespaceEntryKind::File
                && entry.file_id == version.file_id && entry.entry_id == self.target.entry.to_string()
                && entry.version_id == version.version_id && entry.path == self.target.path
                && entry.size == self.size)
        { return Err(invalid()); }
        Ok(())
    }
}

impl Subject {
    pub(super) fn observe(
        initial: bool, previous: &State, next: &State, receipt: &OperationResult,
        sole: Option<(&FileVersion, &Placement)>,
    ) -> Result<Option<Self>> {
        let Some((version, placement)) = sole.filter(|_| initial) else { return Ok(None) };
        let branch_id = receipt.branch_id.as_deref().ok_or_else(invalid)?;
        let before = previous.branches.get(branch_id).ok_or_else(invalid)?;
        let Some(file) = before.file_id.as_deref() else { return Ok(None) };
        if before.branch_kind != BranchKind::File || before.state != BranchState::Active
            || previous.file_branch_aliases.contains_key(branch_id)
            || before.pending_file_versions.contains_key(file) || version.file_id != file
            // The real Workspace constructor sets this flag only for a borrowed
            // pending source, never just because a stored byte body is FULL.
            || version.created_as_full_checkpoint || version.source_version_ids.len() != 1
        { return Ok(None); }
        let id = file.parse()?;
        let Some(record) = previous.file_containers.get(&id) else { return Ok(None) };
        let Some(new_record) = next.file_containers.get(&id) else { return Ok(None) };
        if record.working.unborn().is_some() || record.history_tree_id.to_string() != before.history_tree_id
            || record.created_by != new_record.created_by || record.history_tree_id != new_record.history_tree_id
            || record.namespace != new_record.namespace
            || record.replicas != new_record.replicas || record.owner_disk != new_record.owner_disk
            || record.replicas.len() != 1 || !record.replicas.contains_key(&record.owner_disk)
            || placement.target_copies != 1 || placement.route_slot_ids.as_slice() != [placement.primary_slot_id.as_str()]
            || placement.binding_generations.len() != 1
        { return Ok(None); }
        let owner_replica = &record.replicas[&record.owner_disk];
        if !owner_replica.prefixes.is_empty() { return Ok(None); }
        let repo = previous.repositories.get(&record.namespace.repository.to_string()).ok_or_else(invalid)?;
        let Some(mut parent) = record.namespace.parent.ordinary() else { return Ok(None) };
        if !repo.file_manager_root || repo.history_tree_id != before.history_tree_id
            || record.namespace.branch.to_string() != repo.main_branch_id
        { return Ok(None); }
        let before_commit = previous.commits.get(&before.head_commit_id).ok_or_else(invalid)?;
        let after_commit = next.commits.get(receipt.commit_id.as_deref().ok_or_else(invalid)?).ok_or_else(invalid)?;
        fn only_file(commit: &crate::Commit) -> Option<&crate::Entry> {
            let mut rows = commit.entries.iter().filter(|e| e.kind == NamespaceEntryKind::File);
            let row = rows.next()?;
            rows.next().is_none().then_some(row)
        }
        let (Some(old), Some(new)) = (only_file(before_commit), only_file(after_commit))
            else { return Ok(None) };
        if old.file_id != file || new.file_id != file || old.entry_id != new.entry_id
            || new.entry_id != record.namespace.entry.to_string()
            || new.version_id != version.version_id || old.version_id != version.source_version_ids[0]
        { return Ok(None); }
        let Some(base) = previous.versions.get(&old.version_id) else { return Err(invalid()) };
        if base.file_id != file || base.history_tree_id != before.history_tree_id
            || before_commit.pending_file_versions.contains_key(file)
            || previous.content_states.get(&old.version_id) != Some(&ContentState::Available)
        { return Ok(None); }
        let mut reverse = Vec::new();
        while let NamespaceDirectoryParent::Directory(entry) = parent {
            if reverse.len() == crate::working_tree::admission::MAX_DIRECTORY_DEPTH { return Err(invalid()); }
            let key = entry.to_string();
            let row = previous.namespace_directories.get(&key).ok_or_else(invalid)?;
            if previous.directory_git_repositories.contains_key(&key) || row.owner_disk != record.owner_disk
                || row.replicas.len() != 1 || !row.replicas.get(&record.owner_disk)
                    .is_some_and(|r| r.lifecycle == BindingLifecycle::Current
                        && r.root_id == owner_replica.root_id && r.source_chain.is_empty() && r.source_prefixes.is_empty())
                || next.namespace_directories.get(&key) != Some(row)
            { return Ok(None); }
            if row.entry != entry || row.repository != record.namespace.repository
                || row.branch != record.namespace.branch { return Err(invalid()); }
            reverse.push((entry, row.name.as_str()));
            parent = row.parent;
        }
        reverse.reverse();
        let mut path = String::new();
        for (_, name) in &reverse { path.push_str(name); path.push('/'); }
        path.push_str(record.namespace.name.as_str());
        let path = name_policy::PortablePath::parse(&path)?;
        if !crate::container_storage::namespace_host::ordinary_path(&path) || new.path != path {
            return Ok(None);
        }
        let origin = crate::engine::fresh_layout::namespace::root_origin(
            previous, record.namespace.repository, record.namespace.branch,
        )?;
        let owner = &origin.genesis.registration;
        if !origin.genesis.source_chain.is_empty() || owner.disk_id != record.owner_disk.to_string()
            || origin.genesis.root_id != owner_replica.root_id
            || owner.slot_id != placement.primary_slot_id
            || placement.binding_generations.get(&owner.slot_id) != Some(&owner.binding_generation)
        { return Ok(None); }
        let base_placement = previous.placements.get(&old.version_id).ok_or_else(invalid)?;
        if base_placement.primary_slot_id != owner.slot_id || base_placement.target_copies != 1
            || base_placement.route_slot_ids.as_slice() != [owner.slot_id.as_str()]
            || base_placement.binding_generations != placement.binding_generations
        { return Ok(None); }
        let base_node = crate::business::file_nodes::branch_head(previous, branch_id)?;
        if crate::business::file_nodes::node_version(previous, file, &base_node)? != old.version_id
            || previous.versions.contains_key(&version.version_id)
            || next.versions.get(&version.version_id) != Some(version)
            || next.placements.get(&version.version_id) != Some(placement)
            || new.size != version.logical_size || version.version_sequence != next.sequence
            || version.history_tree_id != before.history_tree_id || placement.version_id != version.version_id
            || (base.logical_size == version.logical_size && base.content_hash == version.content_hash)
            || next.branches.get(branch_id).map(|b| b.head_commit_id.as_str()) != Some(after_commit.commit_id.as_str())
            || crate::business::file_nodes::branch_head(next, branch_id)? != version.version_id
        { return Err(invalid()); }
        let target = DraftLocation {
            repository: record.namespace.repository, path, entry: record.namespace.entry, file: id,
        };
        let ancestors: Vec<_> = reverse.into_iter().map(|(entry, _)| entry).collect();
        if !crate::business::has_origin_occurrence(previous, &target, &ancestors)?
            || !crate::business::has_origin_occurrence(next, &target, &ancestors)?
        { return Ok(None); }
        let fences = Fences::observed(previous, &target, branch_id, &ancestors)?;
        if fences.context.as_ref().is_some_and(|c| c.inherited_from.is_some()) {
            return Ok(None);
        }
        Ok(Some(Self { binding: Binding {
            target, root: origin.genesis.root_id, owner: record.owner_disk, slot: owner.slot_id.clone(),
            binding_generation: Generation::new(owner.binding_generation), ancestors, history: before.history_tree_id.parse()?,
            branch: branch_id.parse()?, head: before.head_commit_id.parse()?, branch_generation: Generation::new(before.generation),
            base_node: base_node.parse()?, base_version: old.version_id.parse()?, fences,
            node: FileNodeId::Version(version.version_id.parse()?), hash: version.content_hash.clone(), size: version.logical_size,
            topology: Generation::new(placement.topology_generation), operation: receipt.operation_id,
            sequence: Sequence::new(receipt.sequence), published_at: time(receipt)?,
        } }))
    }
}

impl Fences {
    fn observed(
        state: &State, target: &DraftLocation, branch: &str, ancestors: &[EntryId],
    ) -> Result<Self> {
        let file = target.file.to_string();
        let repository = target.repository.to_string();
        let directories = std::iter::once(None).chain(ancestors.iter().copied().map(Some))
            .map(|entry| {
                let key = entry.map_or_else(|| repository.clone(), |entry| entry.to_string());
                DirectoryFence {
                    entry, selected: state.directory_selections.get(&key).cloned(),
                    generation: Generation::new(*state.directory_selection_generations.get(&key).unwrap_or(&0)),
                }
            }).collect();
        Ok(Self {
            file_branch: crate::business::selected_file_branch(state, branch, &file)
                .map(|b| b.branch_id.parse()).transpose()?,
            file_generation: Generation::new(*state.file_selection_generations.get(&file).unwrap_or(&0)),
            context: crate::business::context_at_key(state, &repository, target.path.key())
                .filter(|context| context.branch_id == branch),
            directories,
        })
    }

    pub(super) fn captured(workspace: &Workspace, binding: &Binding) -> Result<Self> {
        let repository = binding.target.repository.to_string();
        let file = binding.target.file.to_string();
        let key = crate::business::context_key_from_key(&repository, binding.target.path.key());
        let directories = std::iter::once(None).chain(binding.ancestors.iter().copied().map(Some))
            .map(|entry| {
                let key = entry.map_or_else(|| repository.clone(), |id| id.to_string());
                DirectoryFence {
                    entry, selected: workspace.selected_directory_branches.get(&key).cloned(),
                    generation: Generation::new(*workspace.directory_selection_generations.get(&key).unwrap_or(&0)),
                }
            }).collect();
        Ok(Self {
            file_branch: workspace.selected_file_branches.get(&file)
                .map(|branch| branch.parse()).transpose()?,
            file_generation: Generation::new(*workspace.file_selection_generations.get(&file).unwrap_or(&0)),
            context: workspace.selected_file_contexts.get(&key).cloned(), directories,
        })
    }
}
