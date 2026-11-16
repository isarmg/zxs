//! Decode the existing permanent ordinary Mkdir request without changing its
//! wire or digest. A parsed request alone has no physical-directory authority.
use crate::engine::FreshLayoutError as Failure;
use crate::{OperationResult, Result, State};
use storage_contract::RepositoryId;

pub(crate) fn mkdir_request(
    state: &State, result: &OperationResult,
) -> Result<(RepositoryId, crate::NamespaceOperation)> {
    if result.kind != "file.namespace" || result.sequence > state.sequence {
        return Err(Failure::NamespaceSource.into());
    }
    let path: name_policy::PortablePath = json_codec::from_value(
        result.result.get("path").ok_or(Failure::NamespaceSource)?.clone(),
    ).map_err(Failure::NamespaceRecord)?;
    let action: crate::NamespaceOperationKind = json_codec::from_value(
        result.result.get("action").ok_or(Failure::NamespaceSource)?.clone(),
    ).map_err(Failure::NamespaceRecord)?;
    if action != crate::NamespaceOperationKind::Mkdir {
        return Err(Failure::NamespaceSource.into());
    }
    let history = state.histories.get(result.history_tree_id.as_deref()
        .ok_or(Failure::NamespaceSource)?).ok_or(Failure::NamespaceSource)?;
    let repository = history.repository_id.parse::<RepositoryId>()?;
    let change = crate::NamespaceOperation::Mkdir { path };
    let request = json_codec::json!({"repository_id":repository,"change":change});
    let principal = state.operation_principals.get(&result.operation_id)
        .ok_or(Failure::NamespaceSource)?;
    if crate::Engine::native_request_digest(&result.kind, principal, &request)?
        != result.request_digest
    {
        return Err(Failure::NamespaceSource.into());
    }
    Ok((repository, change))
}

