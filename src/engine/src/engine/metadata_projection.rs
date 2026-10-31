//! Business-only field capture. No authoritative State or disk format changes.
use super::{Arc, BusinessMetadataProjection, State};

enum ProjectionFields {
    Business,
    HistoryWindow,
}

impl ProjectionFields {
    fn business_only<T: Clone + Default>(&self, table: &T) -> T {
        match self {
            Self::Business => table.clone(),
            Self::HistoryWindow => T::default(),
        }
    }
}

impl BusinessMetadataProjection {
    pub(super) fn capture(state: &State) -> Self {
        Self::capture_fields(state, ProjectionFields::Business)
    }

    pub(super) fn capture_history_window(state: &State) -> Self {
        Self::capture_fields(state, ProjectionFields::HistoryWindow)
    }

    fn capture_fields(state: &State, fields: ProjectionFields) -> Self {
        // Exhaustive classification makes new authority-bearing fields a compile
        // error here instead of accidentally retaining them in every read.
        let State {
            sequence,
            business_generation,
            repositories,
            histories,
            branches,
            file_selections,
            directory_branches,
            directory_trees,
            file_write_origins,
            file_contexts,
            file_path_nodes,
            file_metadata_nodes,
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
            file_notes,
            version_notes,
            file_names,
            commits,
            versions,
            placements,
            merge_links,
            history_boundaries,
            content_states,
            operations: _,
            plans: _,
            placement_rules,
            protection_policies,
            operation_principals: _,
            control_auth: _,
            events: _,
            topology_generation,
        } = state;
        // Window consumers never inspect namespace trees, physical placement,
        // backup policy or selection bookkeeping. Do not pin those tables while
        // a bounded page is prepared outside the Engine lock.
        Self(Arc::new(State {
            sequence: *sequence,
            business_generation: *business_generation,
            repositories: repositories.clone(),
            histories: histories.clone(),
            branches: branches.clone(),
            file_selections: fields.business_only(file_selections),
            directory_branches: fields.business_only(directory_branches),
            directory_trees: fields.business_only(directory_trees),
            file_write_origins: file_write_origins.clone(),
            file_contexts: file_contexts.clone(),
            file_path_nodes: file_path_nodes.clone(),
            file_metadata_nodes: file_metadata_nodes.clone(),
            file_branch_merges: file_branch_merges.clone(),
            file_branch_aliases: file_branch_aliases.clone(),
            file_node_parents: file_node_parents.clone(),
            file_path_notes: file_path_notes.clone(),
            directory_tree_selections: fields.business_only(directory_tree_selections),
            file_locations: fields.business_only(file_locations),
            file_backup_paths: fields.business_only(file_backup_paths),
            file_backup_policies: fields.business_only(file_backup_policies),
            directory_selections: fields.business_only(directory_selections),
            directory_selection_generations: fields.business_only(directory_selection_generations),
            file_selection_generations: fields.business_only(file_selection_generations),
            file_notes: file_notes.clone(),
            version_notes: version_notes.clone(),
            file_names: file_names.clone(),
            commits: commits.clone(),
            versions: versions.clone(),
            placements: fields.business_only(placements),
            merge_links: fields.business_only(merge_links),
            history_boundaries: fields.business_only(history_boundaries),
            content_states: content_states.clone(),
            operations: Default::default(),
            plans: Default::default(),
            placement_rules: fields.business_only(placement_rules),
            protection_policies: fields.business_only(protection_policies),
            operation_principals: Default::default(),
            control_auth: None,
            events: Default::default(),
            topology_generation: *topology_generation,
        }))
    }
}
