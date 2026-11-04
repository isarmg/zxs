//! Explicit historical derivations retain their current owning occurrence.
//! Recovery reconstructs the same bounded source proof from the permanent
//! receipt and immutable merge graph, not a temporary allowlist or global ID.
use super::*;
use crate::{
    Branch, FileBranchMerge, FileHistoryMergeRequest, FileMetadataKind, FileMetadataNode,
    OperationResult,
};
use data_serialization::Deserialize;
use storage_contract::{BranchName, FileHistoryMergeInput, FileLocation};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ReceiptLocation {
    repository_id: String,
    path: String,
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "historical merge publication differs from its authenticated source request",
    )
}

struct ReceiptInput {
    inputs: Vec<FileHistoryMergeInput>,
    name: BranchName,
    location: Option<FileLocation>,
}

impl ReceiptInput {
    fn parse(receipt: &OperationResult) -> Result<Self> {
        let inputs = receipt.result.get("source_inputs").ok_or_else(invalid)?;
        let count = inputs.as_array().ok_or_else(invalid)?.len();
        if !(2..=crate::business::file_ancestry::MAX_ANCESTRY_NODES).contains(&count) {
            return Err(invalid());
        }
        let location: Option<ReceiptLocation> =
            json_codec::from_value(receipt.result.get("location").ok_or_else(invalid)?.clone())?;
        let location = location
            .map(|location| -> Result<FileLocation> {
                name_policy::validate_path(&location.path)?;
                Ok(FileLocation {
                    repository_id: location.repository_id,
                    path: location.path,
                })
            })
            .transpose()?;
        Ok(Self {
            inputs: json_codec::from_value(inputs.clone())?,
            name: BranchName::parse(receipt.result["name"].as_str().ok_or_else(invalid)?)?,
            location,
        })
    }

    fn request<'a>(
        &'a self,
        event: &'a FileBranchMerge,
        principal: &'a str,
    ) -> FileHistoryMergeRequest<'a> {
        FileHistoryMergeRequest {
            file_id: &event.file_id,
            inputs: &self.inputs,
            chosen_node_id: &event.chosen_node_id,
            name: &self.name,
            location: self.location.as_ref(),
            principal,
            operation_id: event.operation_id,
        }
    }
}

fn result_matches(receipt: &OperationResult, event: &FileBranchMerge, version: &str) -> bool {
    let fields = [
        ("file_id", event.file_id.as_str()),
        ("merge_id", event.merge_id.as_str()),
        ("history_tree_id", event.history_tree_id.as_str()),
        ("branch_id", event.surviving_branch_id.as_str()),
        ("surviving_branch_id", event.surviving_branch_id.as_str()),
        ("node_id", event.result_node_id.as_str()),
        ("version_id", version),
        ("chosen_node_id", event.chosen_node_id.as_str()),
    ];
    fields
        .iter()
        .all(|(key, value)| receipt.result[*key].as_str() == Some(*value))
        && receipt.result["reused_node"].as_bool() == Some(false)
        && receipt.result["directory_branch_ids"]
            .as_array()
            .is_some_and(Vec::is_empty)
}

impl CurrentNamespaces {
    /// One ancestry proof per new history merge, shared by all derived scopes.
    /// Ordinary merges never acquire same-mutation source authority here.
    pub(super) fn history_merge_sources<'a>(
        &'a self,
        previous: &State,
        next: &State,
    ) -> Result<BTreeMap<String, &'a CurrentScope>> {
        let changes = next
            .file_branch_merges
            .changes_since(&previous.file_branch_merges);
        let events: Vec<_> = match &changes {
            storage_contract::MapChanges::Keys(keys) => keys
                .iter()
                .filter_map(|id| next.file_branch_merges.get(id))
                .collect(),
            storage_contract::MapChanges::All => next.file_branch_merges.values().collect(),
        };
        let mut sources = BTreeMap::new();
        for event in events {
            if previous.file_branch_merges.contains_key(&event.merge_id) {
                continue;
            }
            let receipt = next
                .operations
                .get(&event.operation_id)
                .ok_or_else(invalid)?;
            if receipt.kind != "file.history.merge" {
                continue;
            }
            VerifiedHistoryMerge::parse(previous, next, event, receipt)?
                .admit(self, &mut sources)?;
        }
        Ok(sources)
    }
}

/// Parsed once per newly published operation, then consumed for each helper.
struct VerifiedHistoryMerge<'a> {
    previous: &'a State,
    next: &'a State,
    event: &'a FileBranchMerge,
    node: &'a FileMetadataNode,
    input: ReceiptInput,
    versions: Vec<String>,
}

impl<'a> VerifiedHistoryMerge<'a> {
    fn parse(
        previous: &'a State,
        next: &'a State,
        event: &'a FileBranchMerge,
        receipt: &OperationResult,
    ) -> Result<Self> {
        let principal = next
            .operation_principals
            .get(&event.operation_id)
            .ok_or_else(invalid)?;
        let input = ReceiptInput::parse(receipt)?;
        let request = input.request(event, principal);
        if previous.operations.contains_key(&event.operation_id)
            || receipt.operation_id != event.operation_id
            || receipt.sequence != next.sequence
            || event.sequence != next.sequence
            || !matches!(
                receipt.commit_state,
                crate::CommitState::Published | crate::CommitState::PublishedDegraded
            )
            || receipt.branch_id.as_deref() != Some(event.surviving_branch_id.as_str())
            || receipt.history_tree_id.as_deref() != Some(event.history_tree_id.as_str())
            || receipt.request_digest != request.native_digest()?
        {
            return Err(invalid());
        }
        let prepared = request.prepared_sources(previous)?;
        let version = crate::business::file_nodes::node_version(
            previous,
            &event.file_id,
            &event.chosen_node_id,
        )?;
        if !result_matches(receipt, event, &version)
            || event.input_branch_ids.len() != input.inputs.len()
            || event.input_branch_ids.iter().collect::<BTreeSet<_>>().len() != input.inputs.len()
            || event.input_node_ids
                != input
                    .inputs
                    .iter()
                    .map(|source| source.source_node_id.clone())
                    .collect::<Vec<_>>()
            || event.input_branch_ids.first() != Some(&event.surviving_branch_id)
            || event.reused_node
            || previous
                .file_metadata_nodes
                .contains_key(&event.result_node_id)
        {
            return Err(invalid());
        }
        let node = next
            .file_metadata_nodes
            .get(&event.result_node_id)
            .ok_or_else(invalid)?;
        let parents = event
            .input_node_ids
            .iter()
            .chain(std::iter::once(&event.chosen_node_id))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if node.node_id != event.result_node_id
            || node.file_id != event.file_id
            || node.history_tree_id != event.history_tree_id
            || node.branch_id != event.surviving_branch_id
            || node.version_id != version
            || node.kind != FileMetadataKind::Merge
            || node.operation_id != event.operation_id
            || node.author != *principal
            || node.sequence != next.sequence
            || node.tags != prepared.tags
            || node.parent_node_ids != parents
            || event.parent_node_ids != parents
        {
            return Err(invalid());
        }
        let merged = event
            .input_branch_ids
            .iter()
            .skip(1)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if receipt.result["merged_branch_ids"] != json_codec::json!(merged) {
            return Err(invalid());
        }
        Ok(Self {
            previous,
            next,
            event,
            node,
            input,
            versions: prepared.versions,
        })
    }

    fn admit<'view>(
        &self,
        view: &'view CurrentNamespaces,
        sources: &mut BTreeMap<String, &'view CurrentScope>,
    ) -> Result<()> {
        let Self {
            previous,
            next,
            event,
            node: _,
            input,
            versions,
        } = self;
        for (ordinal, source_version) in versions.iter().enumerate() {
            let branch = next
                .branches
                .get(&event.input_branch_ids[ordinal])
                .ok_or_else(invalid)?;
            let name = if ordinal == 0 {
                input.name.as_str().to_owned()
            } else {
                format!("history-merge-{}-{}", event.operation_id, ordinal + 1)
            };
            if branch.name != name
                || next.branches.get(&input.inputs[ordinal].branch_id)
                    != previous.branches.get(&input.inputs[ordinal].branch_id)
            {
                return Err(invalid());
            }
            let owner = self.branch_source(
                view,
                branch,
                source_version,
                &input.inputs[ordinal].source_node_id,
            )?;
            if let Some(location) = &input.location {
                let repository = &previous.histories[&event.history_tree_id].repository_id;
                if location.repository_id != *repository
                    || !owner.entries.iter().any(|entry| {
                        entry.kind == NamespaceEntryKind::File
                            && entry.file_id == event.file_id
                            && entry.lookup_key() == name_policy::path_key(&location.path)
                    })
                {
                    return Err(invalid());
                }
            }
            if sources.insert(branch.branch_id.clone(), owner).is_some() {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn branch_source<'view>(
        &self,
        view: &'view CurrentNamespaces,
        branch: &Branch,
        source_version: &str,
        source_node: &str,
    ) -> Result<&'view CurrentScope> {
        let Self {
            previous,
            next,
            event,
            node,
            ..
        } = self;
        if previous.branches.contains_key(&branch.branch_id)
            || branch.generation != 2
            || branch.file_id.as_deref() != Some(event.file_id.as_str())
            || branch.history_tree_id != event.history_tree_id
        {
            return Err(invalid());
        }
        let head = next
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(invalid)?;
        let survivor = branch.branch_id == event.surviving_branch_id;
        let pending = if survivor {
            let [parent] = head.parent_commit_ids.as_slice() else {
                return Err(invalid());
            };
            if branch.state != BranchState::Active
                || branch.file_node_id.as_deref() != Some(node.node_id.as_str())
                || !branch.pending_file_versions.is_empty()
                || next.file_branch_aliases.contains_key(&branch.branch_id)
                || head.kind != "file_metadata"
                || head.provenance.as_deref() != Some(node.node_id.as_str())
                || !head.pending_file_versions.is_empty()
                || head.namespace_deletions.is_some()
            {
                return Err(invalid());
            }
            next.commits.get(parent).ok_or_else(invalid)?
        } else {
            if branch.state != BranchState::Merged
                || branch.file_node_id.as_deref() != Some(source_node)
                || next.file_branch_aliases.get(&branch.branch_id)
                    != Some(&event.surviving_branch_id)
            {
                return Err(invalid());
            }
            head
        };
        for commit in [head, pending] {
            if previous.commits.contains_key(&commit.commit_id)
                || commit.history_tree_id != event.history_tree_id
                || commit.operation_id != event.operation_id
                || commit.author != node.author
                || commit.sequence != next.sequence
            {
                return Err(invalid());
            }
        }
        if pending.kind != "file_branch_pending"
            || pending.provenance.as_deref() != Some(source_version)
        {
            return Err(invalid());
        }
        let mut derived = branch.clone();
        derived.head_commit_id = pending.commit_id.clone();
        derived.file_node_id = Some(source_node.into());
        derived.pending_file_versions = pending.pending_file_versions.clone();
        derived.state = BranchState::Active;
        derived.generation = 1;
        let owner = view
            .new_file_branch_source(previous, next, &derived, pending)?
            .ok_or_else(invalid)?;
        view.validate_entries(Some(owner), &pending.entries)?;
        if !survivor && branch.pending_file_versions != pending.pending_file_versions {
            return Err(invalid());
        }
        if survivor {
            let size = next.versions[&node.version_id].logical_size;
            let mut expected = pending.entries.clone();
            for entry in expected
                .iter_mut()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
            {
                entry.version_id = node.version_id.clone();
                entry.size = size;
            }
            if head.entries != expected {
                return Err(invalid());
            }
        }
        Ok(owner)
    }
}
