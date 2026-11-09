//! One publication for explicitly selected current files. Directory containers,
//! immutable working compositions and user commits are never removed here.
use super::*;
use crate::{DirectoryNamespaceBatch, FileDeletionNode, OperationId, OperationResult};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;

mod receipt;
mod source;
pub(super) use receipt::{admit, node_for, verify};
pub(super) use source::capture;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct FrozenSource {
    tree_id: String,
    file_id: String,
}

pub(super) struct Captured {
    targets: std::collections::BTreeMap<String, name_policy::PortablePath>,
    sources: Vec<FrozenSource>,
}

pub(crate) const KIND: &str = "file.delete.batch";
pub(crate) const MAX_FILES: usize = 100;
pub(super) const MAX_RESULT_BYTES: usize =
    storage_contract::client_protocol::MAX_FILE_DELETION_RESPONSE_BYTES / 2;
pub(super) const MAX_FRONTIER: usize =
    storage_contract::client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES;

impl Engine {
    pub(crate) fn soft_delete_directory_files(
        &mut self,
        repository: &str,
        request: &DirectoryNamespaceBatch,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let input = json!({"repository_id": repository, "request": request});
        self.mutate(KIND, principal, operation, input.clone(), |engine, state, _| {
            let captured = capture(
                engine.state(),
                engine.admitted_current_namespaces()?,
                repository,
                request,
                engine.config().max_entries,
            )?;
            let repository_id = HistoryRepositoryId::parse(repository)?;
            let mut nodes = Vec::with_capacity(captured.targets.len());
            let mut frontier = (0, 0);
            for (file, path) in captured.targets {
                let file_id = HistoryFileId::parse(&file)?;
                let path = name_policy::LookupPath::from(&path);
                let capture = engine.file_deletion_preflight(
                    FileDeletionTarget {
                        repository_id: &repository_id,
                        file_id: &file_id,
                        path: &path,
                    },
                    &HistoryWindowCancellation::default(),
                )?;
                require_frontier((capture.branches().len(), capture.parent_node_ids().len()))?;
                frontier.0 += capture.branches().len();
                frontier.1 += capture.parent_node_ids().len();
                require_frontier(frontier)?;
                nodes.push(super::lifecycle::deletion_node(
                    &capture,
                    &path,
                    principal,
                    operation,
                    state.sequence,
                )?);
            }
            let result = json!({
                "input": input, "sources": captured.sources, "deletion_nodes": nodes
            });
            // Timestamp, membership epoch and empty version protection are the
            // only publication additions. Reserve their envelope before any D.
            if json_codec::to_vec(&result)?.len() > MAX_RESULT_BYTES - 1024 {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "complete batch deletion receipt exceeds its recovery response budget",
                ));
            }
            // Capture and admit the entire set before changing even the draft.
            for node in nodes {
                state.file_deletions.insert(node.file_id.clone(), node);
            }
            Ok(result)
        })
    }
}

pub(super) fn require_frontier((branches, parents): (usize, usize)) -> Result<()> {
    if branches > MAX_FRONTIER || parents > MAX_FRONTIER {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "complete batch deletion frontier exceeds the recovery response budget",
        ));
    }
    Ok(())
}
