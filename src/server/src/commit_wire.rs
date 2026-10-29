//! Public commit response fields borrowed from one immutable shared record.
use data_serialization::{Serialize, Serializer};
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::{Commit, CommitNamespace, NamespaceDeletions, OperationId};

pub(crate) struct CommitResponse(Arc<Commit>);
impl From<Arc<Commit>> for CommitResponse {
    fn from(commit: Arc<Commit>) -> Self {
        Self(commit)
    }
}
impl Serialize for CommitResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        CommitRecord::from(self.0.as_ref()).serialize(serializer)
    }
}

#[derive(Serialize)]
pub(crate) struct CommitRecord<'a> {
    commit_id: &'a str,
    history_tree_id: &'a str,
    kind: &'a str,
    parent_commit_ids: &'a [String],
    tree_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    operation_id: OperationId,
    author: &'a str,
    message: &'a str,
    namespace: &'a CommitNamespace,
    #[serialization(skip_serializing_if = "Option::is_none")]
    namespace_deletions: Option<&'a NamespaceDeletions>,
    #[serialization(skip_serializing_if = "BTreeMap::is_empty")]
    pending_file_versions: &'a BTreeMap<String, String>,
    provenance: Option<&'a str>,
}
impl<'a> From<&'a Commit> for CommitRecord<'a> {
    fn from(commit: &'a Commit) -> Self {
        let Commit {
            commit_id,
            history_tree_id,
            kind,
            parent_commit_ids,
            tree_id,
            sequence,
            operation_id,
            author,
            message,
            namespace,
            namespace_deletions,
            pending_file_versions,
            provenance,
        } = commit;
        Self {
            commit_id,
            history_tree_id,
            kind,
            parent_commit_ids,
            tree_id,
            sequence: *sequence,
            operation_id: *operation_id,
            author,
            message,
            namespace,
            namespace_deletions: namespace_deletions.as_ref(),
            pending_file_versions,
            provenance: provenance.as_deref(),
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/commit_wire.rs"]
mod tests;
