//! On-demand, exact-root proof of a historical node's merge source branch.
use crate::{Error, ErrorCode, Result, history_window::*};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHistorySourceRequest {
    pub file_id: HistoryFileId,
    pub node_id: HistoryNodeId,
    pub history_root: HistoryRoot,
}
impl FileHistorySourceRequest {
    pub fn parse(file: &str, node: &str, root: &str) -> Result<Self> {
        Ok(Self {
            file_id: HistoryFileId::parse(file)?,
            node_id: HistoryNodeId::parse(node)?,
            history_root: HistoryRoot::parse(root)?,
        })
    }
    pub fn route(&self) -> String {
        format!(
            "/api/file-manager/files/{}/merge-history/source",
            self.file_id.as_str()
        )
    }
    pub fn query_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("node_id".into(), self.node_id.as_str().into()),
            ("history_root".into(), self.history_root.to_string()),
        ])
    }
    pub fn history_request(&self) -> HistoryWindowRequest {
        HistoryWindowRequest {
            file_id: self.file_id.clone(),
            collection: HistoryCollection::Nodes,
            location: None,
            history_root: Some(self.history_root),
            cursor: None,
            anchor_node_id: None,
            limit: HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes)
                .expect("invariant: one node is within history policy"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileHistorySource {
    pub file_id: HistoryFileId,
    pub source_node_id: HistoryNodeId,
    pub history_root: HistoryRoot,
    pub branch: HistoryBranchSummary,
}
impl FileHistorySource {
    pub fn validate(&self, request: &FileHistorySourceRequest) -> Result<()> {
        if self.file_id != request.file_id
            || self.source_node_id != request.node_id
            || self.history_root != request.history_root
            || self.branch.branch_kind != crate::BranchKind::File
            || self.branch.node_id.is_none()
            || self.branch.read_only
                != (self.branch.state != crate::BranchState::Active
                    || self.branch.alias_of.is_some())
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "historical merge source response differs from its request or has no independent head",
            ));
        }
        crate::validate_branch_name(&self.branch.name)?;
        Ok(())
    }
}
#[cfg(test)]
#[path = "../../../test/contract/unit/file_history_source.rs"]
mod tests;
