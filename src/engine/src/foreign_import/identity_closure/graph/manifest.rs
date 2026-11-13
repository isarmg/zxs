//! Whole history projection, including directory working/index/committed
//! graphs. No field is implicitly omitted or recursively string-replaced.
use super::super::catalog::Domain;
use super::remap::Remap;
use super::*;

impl Remap<'_> {
    pub(in crate::foreign_import::identity_closure) fn manifest(
        &self,
        source: &HistoryManifest,
    ) -> Result<HistoryManifest> {
        // Original receipt digests bind their IDs and principals. The existing
        // graph mapping is not a proof that permits rewriting those envelopes.
        let mut result = source.clone();
        let HistoryManifest {
            path_audit,
            repository,
            history,
            branches,
            directory_branches,
            directory_git_repositories,
            file_path_nodes,
            file_metadata_nodes,
            file_deletions,
            file_branch_merges,
            versions,
            placements,
            merge_links,
            boundary_marks,
            placement_rules,
            protection_policies,
            directory_trees,
            commits,
            file_selections,
            file_write_origins,
            file_branch_aliases,
            file_path_notes,
            file_locations,
            file_backup_paths,
            file_backup_policies,
            file_selection_generations,
            container_tags,
            file_notes,
            version_notes,
            file_names,
            content_states,
            directory_tree_selections,
            directory_selections,
            directory_selection_generations,
            file_contexts,
            file_node_parents,
        } = &mut result;
        if !path_audit.is_empty() {
            return Err(Error::PathChange(crate::PathChangeError::ForeignSource));
        }
        // The empty map is preserved verbatim, not cleared or remapped.
        self.repository(repository)?;
        self.history(history)?;
        for value in branches.values_mut() {
            self.branch(value)?;
        }
        *branches = self.keys(std::mem::take(branches), Domain::Branch)?;
        for value in directory_branches.values_mut() {
            self.directory_branch(value)?;
        }
        *directory_branches =
            self.keys(std::mem::take(directory_branches), Domain::DirectoryBranch)?;
        for value in directory_git_repositories.values_mut() {
            self.directory_git_repository(value)?;
        }
        *directory_git_repositories =
            self.directories(std::mem::take(directory_git_repositories))?;
        for value in file_path_nodes.values_mut() {
            self.path_node(value)?;
        }
        *file_path_nodes = self.keys(std::mem::take(file_path_nodes), Domain::PathNode)?;
        for value in file_metadata_nodes.values_mut() {
            self.metadata_node(value)?;
        }
        *file_metadata_nodes =
            self.keys(std::mem::take(file_metadata_nodes), Domain::MetadataNode)?;
        for value in file_deletions.values_mut() {
            self.deletion_node(value)?;
        }
        *file_deletions = self.keys(std::mem::take(file_deletions), Domain::File)?;
        for value in file_branch_merges.values_mut() {
            self.file_merge(value)?;
        }
        *file_branch_merges = self.keys(std::mem::take(file_branch_merges), Domain::FileMerge)?;
        for value in versions.values_mut() {
            self.version(value)?;
        }
        *versions = self.keys(std::mem::take(versions), Domain::Version)?;
        for value in placements.values_mut() {
            self.placement(value)?;
        }
        *placements = self.keys(std::mem::take(placements), Domain::Version)?;
        for value in merge_links.values_mut() {
            self.merge_link(value)?;
        }
        *merge_links = self.keys(std::mem::take(merge_links), Domain::MergeLink)?;
        for value in boundary_marks.values_mut() {
            self.boundary(value)?;
        }
        *boundary_marks = self.keys(std::mem::take(boundary_marks), Domain::Boundary)?;
        for value in placement_rules.values_mut() {
            self.placement_rule(value)?;
        }
        *placement_rules = self.keys(std::mem::take(placement_rules), Domain::PlacementRule)?;
        for value in protection_policies.values_mut() {
            self.protection_policy(value)?;
        }
        *protection_policies = self.keys(
            std::mem::take(protection_policies),
            Domain::ProtectionPolicy,
        )?;
        *directory_trees = directory_trees
            .iter()
            .map(|(key, value)| {
                let mut value = value.clone();
                self.directory_tree(&mut value)?;
                Ok((self.id(Domain::DirectoryTree, key)?, value))
            })
            .collect::<Result<_>>()?;
        *commits = commits
            .iter()
            .map(|(key, value)| {
                let mut value = value.clone();
                self.commit(&mut value)?;
                Ok((self.id(Domain::Commit, key)?, value))
            })
            .collect::<Result<_>>()?;
        self.pairs(file_selections, Domain::File, Domain::Branch)?;
        self.pairs(file_write_origins, Domain::File, Domain::DirectoryBranch)?;
        self.pairs(file_branch_aliases, Domain::Branch, Domain::Branch)?;
        *file_path_notes = std::mem::take(file_path_notes)
            .into_iter()
            .map(|(key, value)| Ok((self.node(&key)?, value)))
            .collect::<Result<_>>()?;
        *file_locations = self.keys(std::mem::take(file_locations), Domain::File)?;
        *file_backup_paths = self.keys(std::mem::take(file_backup_paths), Domain::File)?;
        self.pairs(file_backup_policies, Domain::File, Domain::ProtectionPolicy)?;
        *file_selection_generations =
            self.keys(std::mem::take(file_selection_generations), Domain::File)?;
        *container_tags = std::mem::take(container_tags).into_iter()
            .map(|(file, tags)| {
                let mapped = self.id(Domain::File, &file.to_string())?.parse()?;
                Ok((mapped, storage_contract::file_container_tags::ContainerTags::new(
                    tags.tags().clone(), storage_contract::Generation::new(0),
                )))
            }).collect::<Result<_>>()?;
        *file_notes = self.keys(std::mem::take(file_notes), Domain::File)?;
        *version_notes = self.keys(std::mem::take(version_notes), Domain::Version)?;
        *file_names = self.keys(std::mem::take(file_names), Domain::File)?;
        *content_states = self.keys(std::mem::take(content_states), Domain::Version)?;
        *directory_tree_selections = directory_tree_selections
            .iter()
            .map(|(key, value)| Ok((self.directory(key)?, self.id(Domain::DirectoryTree, value)?)))
            .collect::<Result<_>>()?;
        *directory_selections = directory_selections
            .iter()
            .map(|(key, value)| {
                Ok((
                    self.directory(key)?,
                    self.id(Domain::DirectoryBranch, value)?,
                ))
            })
            .collect::<Result<_>>()?;
        *directory_selection_generations =
            self.directories(std::mem::take(directory_selection_generations))?;
        self.contexts(file_contexts)?;
        let old_prefix = format!("{}/", source.repository.repository_id);
        let new_prefix = format!("{}/", repository.repository_id);
        *file_contexts = std::mem::take(file_contexts)
            .into_iter()
            .map(|(key, value)| {
                let path = key.strip_prefix(&old_prefix).ok_or_else(|| {
                    invalid("foreign file context escaped its original repository")
                })?;
                Ok((format!("{new_prefix}{path}"), value))
            })
            .collect::<Result<_>>()?;
        for parents in file_node_parents.values_mut() {
            self.nodes(parents)?;
        }
        *file_node_parents = self.keys(std::mem::take(file_node_parents), Domain::Version)?;
        Ok(result)
    }
}
