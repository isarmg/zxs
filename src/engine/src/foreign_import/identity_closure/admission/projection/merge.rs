//! The naming proof made all objects disjoint from both pools before this merge.
//! Exhaustive destructuring makes a newly added history family a compile error.
use super::*;

pub(super) fn include(next: &mut State, mapped: HistoryManifest) -> Result<()> {
    let HistoryManifest {
        path_audit,
        repository,
        history,
        branches,
        file_selections,
        directory_branches,
        directory_git_repositories,
        directory_trees,
        file_write_origins,
        file_contexts,
        file_path_nodes,
        file_metadata_nodes,
        file_deletions,
        file_branch_merges,
        file_branch_aliases,
        file_node_parents,
        file_path_notes,
        directory_tree_selections,
        file_locations,
        file_backup_paths,
        file_backup_policies,
        directory_selections,
        directory_selection_generations,
        file_selection_generations,
        container_tags,
        container_edits,
        file_notes,
        version_notes,
        file_names,
        commits,
        versions,
        placements,
        merge_links,
        boundary_marks,
        content_states,
        placement_rules,
        protection_policies,
    } = mapped;
    // This merge has no source-envelope identity mapping proof. Reject before
    // changing even one target record rather than dropping permanent evidence.
    if !path_audit.is_empty() {
        return Err(Error::PathChange(crate::PathChangeError::ForeignSource));
    }
    next.container_tags.extend(container_tags);
    next.container_edits.extend(container_edits);
    next.repositories
        .insert(repository.repository_id.clone(), repository);
    next.histories
        .insert(history.history_tree_id.clone(), history);
    next.directory_trees.extend_shared(&directory_trees);
    next.commits.extend_shared(&commits);
    next.history_boundaries.extend(boundary_marks);
    macro_rules! include { ($($field:ident),* $(,)?) => {$(next.$field.extend($field);)*}; }
    include! {
        branches, file_selections, directory_branches, directory_git_repositories,
        file_write_origins, file_contexts, file_path_nodes, file_metadata_nodes,
        file_deletions, file_branch_merges, file_branch_aliases, file_node_parents,
        file_path_notes, directory_tree_selections, file_locations, file_backup_paths,
        file_backup_policies, directory_selections, directory_selection_generations,
        file_selection_generations, file_notes, version_notes, file_names, versions,
        placements, merge_links, content_states, placement_rules, protection_policies,
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/foreign_import/path_audit_merge_tests.rs"]
mod tests;
