//! Exhaustive record-field classification. Preserved fields are source-domain
//! audit, content, path or placement data, not new-pool authority.
use super::super::catalog::Domain;
use super::remap::Remap;
use super::*;

impl Remap<'_> {
    pub(super) fn repository(&self, value: &mut storage_contract::Repository) -> Result<()> {
        let storage_contract::Repository {
            repository_id,
            history_tree_id,
            main_branch_id,
            name: _,
            primary_slot_id: _,
            target_copies: _,
            name_policy: _,
            unicode_version: _,
            smb_enabled: _,
            smb_binding_generation: _,
            file_manager_root: _,
        } = value;
        self.replace(Domain::Repository, repository_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Branch, main_branch_id)?;
        Ok(())
    }
    pub(super) fn history(&self, value: &mut storage_contract::HistoryTree) -> Result<()> {
        let storage_contract::HistoryTree {
            history_tree_id,
            repository_id,
            root_commit_id,
            main_branch_id,
            generation: _,
            state: _,
            provenance: _,
        } = value;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Repository, repository_id)?;
        self.replace(Domain::Commit, root_commit_id)?;
        self.optional(Domain::Branch, main_branch_id)?;
        Ok(())
    }
    pub(super) fn branch(&self, value: &mut storage_contract::Branch) -> Result<()> {
        let storage_contract::Branch {
            branch_id,
            history_tree_id,
            head_commit_id,
            fork_commit_id,
            file_id,
            file_node_id,
            pending_file_versions,
            name: _,
            branch_kind: _,
            generation: _,
            state: _,
        } = value;
        self.replace(Domain::Branch, branch_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Commit, head_commit_id)?;
        self.replace(Domain::Commit, fork_commit_id)?;
        self.optional(Domain::File, file_id)?;
        if let Some(id) = file_node_id {
            *id = self.node(id)?;
        }
        self.pairs(pending_file_versions, Domain::File, Domain::Version)?;
        Ok(())
    }
    pub(super) fn directory_branch(
        &self,
        value: &mut storage_contract::DirectoryBranch,
    ) -> Result<()> {
        let storage_contract::DirectoryBranch {
            directory_branch_id,
            repository_id,
            directory_id,
            parent_branch_id,
            head_tree_id,
            commit_id,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
            path: _,
            name: _,
            materialized: _,
            sequence: _,
        } = value;
        self.replace(Domain::DirectoryBranch, directory_branch_id)?;
        self.replace(Domain::Repository, repository_id)?;
        *directory_id = self.directory(directory_id)?;
        self.optional(Domain::DirectoryBranch, parent_branch_id)?;
        self.replace(Domain::DirectoryTree, head_tree_id)?;
        self.replace(Domain::Commit, commit_id)?;
        self.entries(entries)?;
        self.pairs(file_branches, Domain::File, Domain::Branch)?;
        self.contexts(file_contexts)?;
        self.pairs(inherited_files, Domain::File, Domain::DirectoryBranch)?;
        self.references(directory_refs)?;
        Ok(())
    }
    pub(super) fn directory_tree(&self, value: &mut storage_contract::DirectoryTree) -> Result<()> {
        let storage_contract::DirectoryTree {
            tree_id,
            directory_branch_id,
            parent_tree_id,
            commit_id,
            entries,
            file_branches,
            file_contexts,
            inherited_files,
            directory_refs,
            path: _,
            sequence: _,
        } = value;
        self.replace(Domain::DirectoryTree, tree_id)?;
        self.replace(Domain::DirectoryBranch, directory_branch_id)?;
        self.optional(Domain::DirectoryTree, parent_tree_id)?;
        self.replace(Domain::Commit, commit_id)?;
        self.entries(entries)?;
        self.pairs(file_branches, Domain::File, Domain::Branch)?;
        self.contexts(file_contexts)?;
        self.pairs(inherited_files, Domain::File, Domain::DirectoryBranch)?;
        self.references(directory_refs)?;
        Ok(())
    }
    pub(super) fn entry(&self, value: &mut storage_contract::Entry) -> Result<()> {
        let storage_contract::Entry {
            entry_id,
            kind,
            file_id,
            version_id,
            path: _,
            size: _,
        } = value;
        self.replace(Domain::Entry, entry_id)?;
        if *kind == storage_contract::NamespaceEntryKind::File {
            self.replace(Domain::File, file_id)?;
        }
        if *kind == storage_contract::NamespaceEntryKind::File {
            self.replace(Domain::Version, version_id)?;
        }
        Ok(())
    }
    pub(super) fn context(&self, value: &mut storage_contract::FileContext) -> Result<()> {
        let storage_contract::FileContext {
            node_id,
            branch_id,
            inherited_from,
            selection_generation: _,
        } = value;
        if let Some(id) = node_id {
            *id = self.node(id)?;
        }
        self.replace(Domain::Branch, branch_id)?;
        self.optional(Domain::DirectoryBranch, inherited_from)?;
        Ok(())
    }
    pub(super) fn reference(&self, value: &mut storage_contract::DirectoryReference) -> Result<()> {
        let storage_contract::DirectoryReference {
            directory_branch_id,
            tree_id,
        } = value;
        self.replace(Domain::DirectoryBranch, directory_branch_id)?;
        self.replace(Domain::DirectoryTree, tree_id)?;
        Ok(())
    }
    pub(super) fn commit(&self, value: &mut storage_contract::Commit) -> Result<()> {
        let storage_contract::Commit {
            commit_id,
            history_tree_id,
            parent_commit_ids,
            tree_id,
            entries,
            pending_file_versions,
            kind,
            sequence: _,
            operation_id: _,
            author: _,
            message: _,
            namespace_deletions: _,
            provenance,
        } = value;
        self.replace(Domain::Commit, commit_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.ids(Domain::Commit, parent_commit_ids)?;
        self.replace(Domain::CommitTree, tree_id)?;
        self.entries(entries)?;
        self.pairs(pending_file_versions, Domain::File, Domain::Version)?;
        if kind.as_str() == "directory_commit" {
            self.optional(Domain::DirectoryTree, provenance)?;
        }
        Ok(())
    }
    pub(super) fn version(&self, value: &mut storage_contract::FileVersion) -> Result<()> {
        let storage_contract::FileVersion {
            history_tree_id,
            version_id,
            file_id,
            source_version_ids,
            creation_primary_slot_id: _,
            logical_size: _,
            version_sequence: _,
            content_hash: _,
            checksum_chunk_bytes: _,
            checksum_manifest_hash: _,
            creation_name: _,
            storage_name: _,
            created_at: _,
            created_as_full_checkpoint: _,
        } = value;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Version, version_id)?;
        self.replace(Domain::File, file_id)?;
        self.ids(Domain::Version, source_version_ids)?;
        Ok(())
    }
    pub(super) fn placement(&self, value: &mut storage_contract::Placement) -> Result<()> {
        let storage_contract::Placement {
            version_id,
            primary_slot_id: _,
            target_copies: _,
            route_slot_ids: _,
            topology_generation: _,
            binding_generations: _,
        } = value;
        self.replace(Domain::Version, version_id)?;
        Ok(())
    }
    pub(super) fn path_node(&self, value: &mut storage_contract::FilePathNode) -> Result<()> {
        let storage_contract::FilePathNode {
            node_id,
            history_tree_id,
            file_id,
            version_id,
            parent_node_ids,
            branch_id,
            path: _,
            previous_path: _,
            operation_id: _,
            author: _,
            sequence: _,
            created_at: _,
        } = value;
        *node_id = self.node(node_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::File, file_id)?;
        self.replace(Domain::Version, version_id)?;
        self.nodes(parent_node_ids)?;
        self.replace(Domain::Branch, branch_id)?;
        Ok(())
    }
    pub(super) fn metadata_node(
        &self,
        value: &mut storage_contract::FileMetadataNode,
    ) -> Result<()> {
        let storage_contract::FileMetadataNode {
            node_id,
            history_tree_id,
            file_id,
            version_id,
            parent_node_ids,
            branch_id,
            kind: _,
            tags: _,
            operation_id: _,
            author: _,
            sequence: _,
            created_at: _,
        } = value;
        *node_id = self.node(node_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::File, file_id)?;
        self.replace(Domain::Version, version_id)?;
        self.nodes(parent_node_ids)?;
        self.replace(Domain::Branch, branch_id)?;
        Ok(())
    }
    pub(super) fn deletion_node(
        &self,
        value: &mut storage_contract::FileDeletionNode,
    ) -> Result<()> {
        let storage_contract::FileDeletionNode {
            node_id,
            repository_id,
            history_tree_id,
            file_id,
            branch_heads,
            parent_node_ids,
            path: _,
            operation_id: _,
            author: _,
            sequence: _,
            created_at: _,
        } = value;
        self.replace(Domain::DeletionNode, node_id)?;
        self.replace(Domain::Repository, repository_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::File, file_id)?;
        *branch_heads = branch_heads
            .iter()
            .map(|(branch, node)| Ok((self.id(Domain::Branch, branch)?, self.node(node)?)))
            .collect::<Result<_>>()?;
        self.nodes(parent_node_ids)?;
        Ok(())
    }
    pub(super) fn file_merge(&self, value: &mut storage_contract::FileBranchMerge) -> Result<()> {
        let storage_contract::FileBranchMerge {
            merge_id,
            history_tree_id,
            file_id,
            input_branch_ids,
            input_node_ids,
            chosen_node_id,
            result_node_id,
            surviving_branch_id,
            parent_node_ids,
            reused_node: _,
            operation_id: _,
            sequence: _,
        } = value;
        self.replace(Domain::FileMerge, merge_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::File, file_id)?;
        self.ids(Domain::Branch, input_branch_ids)?;
        self.nodes(input_node_ids)?;
        *chosen_node_id = self.node(chosen_node_id)?;
        *result_node_id = self.node(result_node_id)?;
        self.replace(Domain::Branch, surviving_branch_id)?;
        self.nodes(parent_node_ids)?;
        Ok(())
    }
    pub(super) fn merge_link(&self, value: &mut storage_contract::MergeLink) -> Result<()> {
        let storage_contract::MergeLink {
            merge_link_id,
            history_tree_id,
            source_branch_id,
            source_commit_id,
            target_branch_id,
            target_commit_id,
            author: _,
            operation_id: _,
        } = value;
        self.replace(Domain::MergeLink, merge_link_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Branch, source_branch_id)?;
        self.replace(Domain::Commit, source_commit_id)?;
        self.replace(Domain::Branch, target_branch_id)?;
        self.replace(Domain::Commit, target_commit_id)?;
        Ok(())
    }
    pub(super) fn boundary(&self, value: &mut storage_contract::HistoryBoundary) -> Result<()> {
        let storage_contract::HistoryBoundary {
            boundary_id,
            history_tree_id,
            local_commit_id,
            local_branch_id,
            relation: _,
            external_history_tree_id: _,
            external_commit_id: _,
            source_merge_link_id: _,
        } = value;
        self.replace(Domain::Boundary, boundary_id)?;
        self.replace(Domain::History, history_tree_id)?;
        self.replace(Domain::Commit, local_commit_id)?;
        self.optional(Domain::Branch, local_branch_id)?;
        Ok(())
    }
    pub(super) fn placement_rule(&self, value: &mut storage_contract::PlacementRule) -> Result<()> {
        let storage_contract::PlacementRule {
            rule_id,
            repository_id,
            path_prefix: _,
            primary_slot_id: _,
            generation: _,
        } = value;
        self.replace(Domain::PlacementRule, rule_id)?;
        self.replace(Domain::Repository, repository_id)?;
        Ok(())
    }
    pub(super) fn protection_policy(
        &self,
        value: &mut storage_contract::ProtectionPolicy,
    ) -> Result<()> {
        let storage_contract::ProtectionPolicy {
            policy_id,
            repository_id,
            scope_directory_id,
            scope_file_id,
            path_prefix: _,
            target_copies: _,
            inheritable: _,
            backup_prefixes: _,
            generation: _,
        } = value;
        self.replace(Domain::ProtectionPolicy, policy_id)?;
        self.replace(Domain::Repository, repository_id)?;
        if let Some(id) = scope_directory_id {
            *id = self.directory(id)?;
        }
        self.optional(Domain::File, scope_file_id)?;
        Ok(())
    }
    pub(super) fn directory_git_repository(
        &self,
        value: &mut storage_contract::directory_git::DirectoryGitRepository,
    ) -> Result<()> {
        let storage_contract::directory_git::DirectoryGitRepository {
            repository_id,
            directory_id,
            main_branch_id,
            current_branch_id,
            branches,
            initialized_sequence: _,
            deletions,
        } = value;
        if !deletions.is_empty() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "foreign directory lifecycle has not been remapped; retain the original graph",
            ));
        }
        self.replace(Domain::Repository, repository_id)?;
        *directory_id = self.directory(directory_id)?;
        self.replace(Domain::DirectoryBranch, main_branch_id)?;
        self.replace(Domain::DirectoryBranch, current_branch_id)?;
        for value in branches.values_mut() {
            self.directory_git_branch(value)?;
        }
        *branches = self.keys(std::mem::take(branches), Domain::DirectoryBranch)?;
        Ok(())
    }
    pub(super) fn directory_git_branch(
        &self,
        value: &mut storage_contract::directory_git::DirectoryGitBranch,
    ) -> Result<()> {
        let storage_contract::directory_git::DirectoryGitBranch {
            head_tree_id,
            index_tree_id,
            index_boundaries,
            index_children,
            index_generation: _,
        } = value;
        self.optional(Domain::DirectoryTree, head_tree_id)?;
        self.replace(Domain::DirectoryTree, index_tree_id)?;
        *index_boundaries = self.directories(std::mem::take(index_boundaries))?;
        for value in index_children.values_mut() {
            self.directory_git_child(value)?;
        }
        *index_children = self.directories(std::mem::take(index_children))?;
        Ok(())
    }
    pub(super) fn directory_git_child(
        &self,
        value: &mut storage_contract::directory_git::DirectoryGitChildReference,
    ) -> Result<()> {
        let storage_contract::directory_git::DirectoryGitChildReference {
            directory_branch_id,
            tree_id,
        } = value;
        self.replace(Domain::DirectoryBranch, directory_branch_id)?;
        self.optional(Domain::DirectoryTree, tree_id)?;
        Ok(())
    }
}
