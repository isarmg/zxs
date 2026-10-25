//! Admission of a complete decoded disk snapshot, before it becomes engine state.
use super::HistoryManifest;
use crate::{Error, ErrorCode, Result, State};
use data_serialization::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serialization(transparent)]
pub(crate) struct RawState(State);

impl RawState {
    pub(crate) fn sequence(&self) -> u64 {
        self.0.sequence
    }

    #[cfg(test)]
    pub(crate) fn into_codec_fixture(self) -> State {
        self.0
    }
}

impl TryFrom<RawState> for State {
    type Error = Error;

    fn try_from(raw: RawState) -> Result<Self> {
        let state = raw.0;
        validate(&state)?;
        Ok(state)
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::StateInvalid, message)
}

macro_rules! history_coverage {
    ($($field:ident => $projected:ident),* $(,)?) => {
        #[derive(Default)]
        struct HistoryCoverage {
            repositories: BTreeSet<String>,
            $($field: BTreeSet<String>,)*
        }

        impl HistoryCoverage {
            fn include(&mut self, manifest: &HistoryManifest) -> Result<()> {
                if !self.repositories.insert(manifest.repository.repository_id.clone()) {
                    return Err(invalid("snapshot repository belongs to multiple histories"));
                }
                $(for key in manifest.$projected.keys() {
                    if !self.$field.insert(key.clone()) {
                        return Err(invalid(concat!("snapshot ", stringify!($field), " contains multiply owned records")));
                    }
                })*
                Ok(())
            }

            fn finish(self, state: &State) -> Result<()> {
                let State {
                    sequence: _, business_generation: _, topology_generation: _,
                    control_auth: _, events: _, operations: _, operation_principals: _, plans: _,
                    content_states: _,
                    histories: _, repositories, $($field,)*
                } = state;
                if self.repositories.iter().ne(repositories.keys()) {
                    return Err(invalid("snapshot contains an unowned repository"));
                }
                $(if self.$field.iter().ne($field.keys()) {
                    return Err(invalid(concat!("snapshot ", stringify!($field), " contains unowned records")));
                })*
                Ok(())
            }
        }
    }
}

history_coverage! {
    branches => branches,
    file_selections => file_selections,
    directory_branches => directory_branches,
    directory_trees => directory_trees,
    file_write_origins => file_write_origins,
    file_contexts => file_contexts,
    file_path_nodes => file_path_nodes,
    file_metadata_nodes => file_metadata_nodes,
    file_branch_merges => file_branch_merges,
    file_branch_aliases => file_branch_aliases,
    file_node_parents => file_node_parents,
    file_path_notes => file_path_notes,
    directory_tree_selections => directory_tree_selections,
    file_locations => file_locations,
    file_backup_paths => file_backup_paths,
    file_backup_policies => file_backup_policies,
    directory_selections => directory_selections,
    directory_selection_generations => directory_selection_generations,
    file_selection_generations => file_selection_generations,
    file_notes => file_notes,
    version_notes => version_notes,
    file_names => file_names,
    commits => commits,
    versions => versions,
    placements => placements,
    merge_links => merge_links,
    placement_rules => placement_rules,
    protection_policies => protection_policies,
    history_boundaries => boundary_marks,
}

fn validate(state: &State) -> Result<()> {
    // History projection filters unowned rows; reject those at the disk boundary.
    if state
        .content_states
        .keys()
        .any(|id| !state.versions.contains_key(id))
    {
        return Err(invalid("snapshot content state has no version"));
    }
    let mut file_owners = BTreeMap::new();
    for version in state.versions.values() {
        if file_owners
            .insert(&version.file_id, &version.history_tree_id)
            .is_some_and(|owner| owner != &version.history_tree_id)
        {
            return Err(invalid("snapshot stable file identity crosses histories"));
        }
    }
    let mut covered = HistoryCoverage::default();
    for (id, history) in state.histories.iter() {
        if id != &history.history_tree_id {
            return Err(invalid("snapshot history identity differs from its key"));
        }
        let manifest = HistoryManifest::project_state(state, id)
            .map_err(|error| error.context(ErrorCode::StateInvalid, "invalid snapshot history"))?;
        manifest
            .validate()
            .map_err(|error| error.context(ErrorCode::StateInvalid, "invalid snapshot history"))?;
        covered.include(&manifest)?;
    }
    covered.finish(state)
}
