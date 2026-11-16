//! Permanent file-manager root creation evidence. This codec grants no root I/O.
use crate::engine::FreshLayoutError as Failure;
use crate::{Engine, OperationResult, Repository, Result};
use data_serialization::{Deserialize, Serialize};
use storage_contract::{BranchId, HistoryTreeId, OperationId, RepositoryId, Sequence};
use storage_contract::working_tree::RootBinding;

const FIELD: &str = "file_manager_root_origin";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RootOrigin {
    pub(crate) repository: RepositoryId,
    pub(crate) history: HistoryTreeId,
    pub(crate) branch: BranchId,
    pub(crate) operation: OperationId,
    pub(crate) sequence: Sequence,
    pub(crate) genesis: RootBinding,
}

pub(crate) fn attach(result: &mut json_codec::Value, origin: &RootOrigin) -> Result<()> {
    result[FIELD] = json_codec::to_value(origin).map_err(Failure::NamespaceRecord)?;
    Ok(())
}

pub(crate) fn origin(result: &OperationResult) -> Result<Option<RootOrigin>> {
    result.result.get(FIELD).map(|value| {
        json_codec::from_value(value.clone()).map_err(Failure::NamespaceRecord).map_err(Into::into)
    }).transpose()
}

pub(crate) fn created_repository(result: &OperationResult) -> Result<Repository> {
    let value = result.result.get("created_repository").ok_or(Failure::NamespaceSource)?;
    Ok(json_codec::from_value(value.clone()).map_err(Failure::NamespaceRecord)?)
}

pub(crate) fn require_creation_request(
    receipt: &OperationResult,
    created: &Repository,
    principal: &str,
) -> Result<()> {
    let request = json_codec::json!({
        "name": &created.name,
        "primary_slot_id": &created.primary_slot_id,
        "target_copies": created.target_copies,
        "file_manager_root": true,
    });
    if receipt.request_digest != Engine::native_request_digest(
        "repository.create", principal, &request,
    )? || receipt.result.get("branch_generation") != Some(&json_codec::json!("1"))
        || receipt.result.get("version_protection") != Some(&json_codec::json!([]))
        || receipt.result.get("repository_id") != Some(&json_codec::json!(&created.repository_id))
        || receipt.result.get("history_tree_id")
            != Some(&json_codec::json!(&created.history_tree_id))
        || receipt.result.get("branch_id") != Some(&json_codec::json!(&created.main_branch_id))
        || receipt.result.get("commit_id") != Some(&json_codec::json!(&receipt.commit_id))
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok(())
}
