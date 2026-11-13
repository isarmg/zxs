//! Strict canonical boundary for container-mode request and receipt bytes.
use crate::engine::container_drafts::{DraftError, EDIT_KIND, plan_edit};
use crate::{Engine, Error, OperationId, OperationResult};
use data_serialization::{Deserialize, Serialize};
use storage_contract::container_edit::{ContainerEdit, EditCommand};
use storage_contract::{CommitState, CurrentHealth, Principal, ProtectionAtPublish};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Envelope {
    pub(crate) request: EditCommand,
    pub(crate) replacement: ContainerEdit,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<()>,
}

#[derive(Serialize)]
struct Body<'a> {
    request: &'a EditCommand,
    replacement: ContainerEdit,
}

fn codec(error: json_codec::Error) -> Error {
    Error::ContainerDrafts(DraftError::Encoding(error))
}

pub(crate) fn publish(
    engine: &mut Engine,
    request: &EditCommand,
    principal: &Principal,
    operation: OperationId,
) -> crate::Result<OperationResult> {
    let body = json_codec::to_value(request).map_err(codec)?;
    engine.mutate(EDIT_KIND, principal.as_str(), operation, body, |engine, next, _| {
        let replacement = plan_edit(engine.state(), request)?;
        next.container_edits.insert(request.file(), replacement.clone());
        json_codec::to_value(Body { request, replacement }).map_err(codec)
    })
}

pub(crate) fn decode(receipt: &OperationResult) -> crate::Result<Envelope> {
    let decoded: Envelope = json_codec::from_value(receipt.result.clone()).map_err(codec)?;
    if !decoded.version_protection.is_empty() {
        return Err(crate::engine::container_drafts::invalid());
    }
    Ok(decoded)
}

pub(crate) fn expected_receipt(
    operation: OperationId,
    principal: &str,
    sequence: u64,
    decoded: &Envelope,
) -> crate::Result<OperationResult> {
    let body = json_codec::to_value(&decoded.request).map_err(codec)?;
    Ok(OperationResult {
        operation_id: operation,
        kind: EDIT_KIND.to_owned(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: Engine::native_request_digest(EDIT_KIND, principal, &body)?,
        sequence,
        result: json_codec::to_value(decoded).map_err(codec)?,
    })
}
