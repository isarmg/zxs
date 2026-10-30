//! Admission of a complete decoded disk snapshot, before it becomes engine state.
use super::HistoryManifest;
use crate::{Error, ErrorCode, Result, State};
use data_serialization::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Deserialize)]
#[serialization(transparent)]
pub(crate) struct RawState(State);

impl RawState {
    #[cfg(test)]
    pub(crate) fn into_codec_fixture(self) -> State {
        self.0
    }
}

pub(crate) struct AdmittedState(State);

impl AdmittedState {
    pub(crate) fn state(&self) -> &State {
        &self.0
    }
    pub(crate) fn into_state(self) -> State {
        self.0
    }

    /// Lend the original admitted bytes to read-only workspace admission while
    /// retaining the proof for the same candidate's later history checks.
    pub(crate) fn into_shared(self) -> SharedAdmittedState {
        SharedAdmittedState(Arc::new(self.0))
    }
}

pub(crate) struct SharedAdmittedState(Arc<State>);

impl SharedAdmittedState {
    pub(crate) fn share_state(&self) -> Arc<State> {
        Arc::clone(&self.0)
    }

    /// The workspace-admission loan must end before replay consumes this proof.
    /// Recover ownership without revalidation, decoding or cloning the State.
    pub(crate) fn into_admitted(self) -> AdmittedState {
        let Ok(state) = Arc::try_unwrap(self.0) else {
            unreachable!("invariant: workspace admission released its snapshot loan");
        };
        AdmittedState(state)
    }
}

impl TryFrom<RawState> for AdmittedState {
    type Error = Error;

    fn try_from(raw: RawState) -> Result<Self> {
        validate(&raw.0)?;
        Ok(Self(raw.0))
    }
}

impl TryFrom<RawState> for State {
    type Error = Error;

    fn try_from(raw: RawState) -> Result<Self> {
        Ok(AdmittedState::try_from(raw)?.into_state())
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
    // Event polling seeks by sequence while preserving ordinal array order.
    // Writers append increasing publication slots; recovery must establish the
    // same premise for decoded bytes, including equal-sequence event groups.
    if !state.events.iter().map(|event| event.sequence).is_sorted() {
        return Err(invalid("snapshot event sequences are not nondecreasing"));
    }
    // The sorted tail must already be published, so the next writer's newer
    // publication slot preserves this order instead of appending behind it.
    if state
        .events
        .iter()
        .next_back()
        .is_some_and(|event| event.sequence > state.sequence)
    {
        return Err(invalid(
            "snapshot event sequence exceeds published sequence",
        ));
    }
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
