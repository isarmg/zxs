//! Merge frozen historical nodes on new branches without consuming their sources.
use super::file_ancestry::{FileAncestry, MAX_ANCESTRY_HEADERS, MAX_ANCESTRY_NODES, SearchBudget};
use super::*;
use json_codec::json;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct PreparedSources {
    pub(crate) versions: Vec<String>,
}

/// Only this explicit history consumer can construct a same-mutation input.
/// Ordinary branch merges still require an admitted existing branch snapshot.
struct DerivedBranchSource {
    branch: Branch,
    entries: Vec<Entry>,
}

pub(super) struct CurrentHistoryMergeInputs {
    branches: BTreeMap<String, DerivedBranchSource>,
}

impl CurrentHistoryMergeInputs {
    fn prepare(engine: &Engine, state: &State, inputs: &[FileMergeInput]) -> Result<Option<Self>> {
        let Some(view) = engine.admitted_current_namespaces()? else {
            return Ok(None);
        };
        let mut branches = BTreeMap::new();
        for input in inputs {
            let branch = &state.branches[&input.branch_id];
            let entries = view.history_merge_input_entries(engine.state(), state, branch)?;
            branches.insert(
                input.branch_id.clone(),
                DerivedBranchSource {
                    branch: branch.clone(),
                    entries: entries.to_vec(),
                },
            );
        }
        Ok(Some(Self { branches }))
    }

    pub(super) fn entries(&self, branch: &Branch) -> Result<&[Entry]> {
        self.branches
            .get(&branch.branch_id)
            .filter(|admitted| &admitted.branch == branch)
            .map(|admitted| admitted.entries.as_slice())
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::WorkspaceChanged,
                    "historical merge input changed after occurrence admission",
                )
            })
    }
}

impl FileHistoryMergeRequest<'_> {
    pub(crate) fn native_digest(&self) -> Result<String> {
        Engine::native_request_digest("file.history.merge", self.principal, &self.body())
    }

    /// Shared by live planning and authenticated publication replay. The proof
    /// follows the original bounded history graph, never the current projection.
    pub(crate) fn prepared_sources(&self, state: &State) -> Result<PreparedSources> {
        if self.inputs.len() < 2 {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "historical merge requires at least two source nodes",
            ));
        }
        if self.inputs.len() > MAX_ANCESTRY_NODES {
            return Err(file_ancestry::exhausted());
        }
        sources(state, self)
    }
}
fn sources(state: &State, request: &FileHistoryMergeRequest<'_>) -> Result<PreparedSources> {
    let mut seen = BTreeSet::new();
    let mut versions = Vec::with_capacity(request.inputs.len());
    let mut ancestry_targets = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut history = None;
    let mut head_entries = 0;
    for input in request.inputs {
        if !seen.insert(&input.source_node_id) {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "historical merge source nodes must be distinct",
            ));
        }
        let branch = state
            .branches
            .get(&input.branch_id)
            .filter(|branch| {
                branch.branch_kind == BranchKind::File
                    && branch.file_id.as_deref() == Some(request.file_id)
            })
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::FileBranchNotFound,
                    "historical merge source branch does not belong to this file",
                )
            })?;
        // Sources are read-only references: archived branches and merged aliases
        // remain valid, but their exact reviewed head is still a precondition.
        let head = if let Some(head) = branch.file_node_id.as_deref() {
            head
        } else {
            let mut head = None;
            for entry in &state.commits[&branch.head_commit_id].entries {
                if head_entries == MAX_ANCESTRY_HEADERS {
                    return Err(file_ancestry::exhausted());
                }
                head_entries += 1;
                if entry.kind == NamespaceEntryKind::File && entry.file_id == request.file_id {
                    head = Some(entry.version_id.as_str());
                    break;
                }
            }
            head.ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "file branch head is missing",
                )
            })?
        };
        file_nodes::node_version(state, request.file_id, head)?;
        if head != input.expected_node_id {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchConflict,
                "historical merge source head changed; review the inputs again",
            ));
        }
        let version = file_nodes::node_version(state, request.file_id, &input.source_node_id)?;
        if state.versions[&version].history_tree_id != branch.history_tree_id {
            return Err(Error::new(
                storage_contract::ErrorCode::FileNodeNotFound,
                "historical merge source node is outside the selected branch",
            ));
        }
        if history.is_some_and(|history| history != branch.history_tree_id) {
            return Err(Error::new(
                storage_contract::ErrorCode::FileNodeNotFound,
                "historical merge sources belong to different histories",
            ));
        }
        history.get_or_insert(branch.history_tree_id.as_str());
        let explicitly_owned = state
            .file_metadata_nodes
            .get(&input.source_node_id)
            .is_some_and(|node| node.branch_id == input.branch_id)
            || state
                .file_path_nodes
                .get(&input.source_node_id)
                .is_some_and(|node| node.branch_id == input.branch_id);
        if !explicitly_owned && input.source_node_id != head {
            ancestry_targets
                .entry(input.expected_node_id.as_str())
                .or_default()
                .insert(input.source_node_id.as_str());
        }
        versions.push(version);
    }
    let mut budget = SearchBudget::write();
    if !ancestry_targets.is_empty() {
        // Still the same pre-mutation snapshot. Build once, group identical
        // reviewed heads, and reject every incomplete proof before deriving.
        let index = FileAncestry::new(
            state,
            request.file_id,
            history.expect("invariant: nonempty merge inputs have a file history"),
            || Ok(()),
        )?;

        for (head, targets) in ancestry_targets {
            if !index.contains_all(head, &targets, &mut budget, || Ok(()))? {
                return Err(Error::new(
                    storage_contract::ErrorCode::FileNodeNotFound,
                    "historical merge source node is outside the selected branch",
                ));
            }
        }
    }
    let chosen_version = file_nodes::node_version(state, request.file_id, request.chosen_node_id)?;
    if history != Some(state.versions[&chosen_version].history_tree_id.as_str()) {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNodeNotFound,
            "historical merge chosen node belongs to another history",
        ));
    }
    Ok(PreparedSources { versions })
}

impl Engine {
    pub fn merge_file_history(
        &mut self,
        request: FileHistoryMergeRequest<'_>,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.history.merge",
            request.principal,
            request.operation_id,
            request.body(),
            |engine, state, _| {
                file_requests::validate_location(engine, request.file_id, request.location)?;
                if request.inputs.len() < 2 || request.inputs.len() > engine.config.max_entries {
                    return Err(Error::new(
                        storage_contract::ErrorCode::InvalidRequest,
                        "historical merge requires at least two bounded source nodes",
                    ));
                }
                let prepared = request.prepared_sources(state)?;
                let mut inputs = Vec::with_capacity(request.inputs.len());
                for (ordinal, (input, version)) in
                    request.inputs.iter().zip(&prepared.versions).enumerate()
                {
                    let helper_name;
                    let name = if ordinal == 0 {
                        request.name
                    } else {
                        // Operation identity and input ordinal make these public
                        // provenance labels stable and independent of user names.
                        helper_name = storage_contract::BranchName::parse(&format!(
                            "history-merge-{}-{}",
                            request.operation_id,
                            ordinal + 1,
                        ))?;
                        &helper_name
                    };
                    let created = file::create_file_branch_change(
                        &engine.planning_context(),
                        state,
                        file::FileBranchCreation {
                            file_id: request.file_id,
                            source_version: version,
                            name: name.as_str(),
                            name_key: name.key(),
                        },
                        request.principal,
                        request.operation_id,
                    )?;
                    state
                        .branches
                        .get_mut(&created.branch_id)
                        .expect("invariant: branch was created in this mutation")
                        .file_node_id = Some(input.source_node_id.clone());
                    inputs.push(FileMergeInput {
                        branch_id: created.branch_id,
                        expected_node_id: input.source_node_id.clone(),
                    });
                }
                let current_inputs = CurrentHistoryMergeInputs::prepare(engine, state, &inputs)?;
                let mut result = file_merges::merge_change(
                    engine,
                    state,
                    FileBranchMergeRequest {
                        file_id: request.file_id,
                        inputs: &inputs,
                        chosen_node_id: request.chosen_node_id,
                        surviving_branch_id: &inputs[0].branch_id,
                        location: None,
                        principal: request.principal,
                        operation_id: request.operation_id,
                    },
                    file_merges::MergeResultPolicy::AlwaysNewNode {
                        current_inputs: current_inputs.as_ref(),
                    },
                )?;
                result["name"] = json!(request.name.as_str());
                result["source_inputs"] = json!(request.inputs);
                // The permanent receipt binds the exact original request during
                // recovery, including absence of a path. Older receipts stay intact.
                result["location"] = json!(request.location);
                Ok(result)
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_history_merge_limits.rs"]
mod tests;
