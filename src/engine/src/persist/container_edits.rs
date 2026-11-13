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
    crate::engine::container_drafts::format::require_manual_storage(&engine.config)?;
    let body = json_codec::to_value(request).map_err(codec)?;
    engine.mutate(EDIT_KIND, principal.as_str(), operation, body, |engine, next, _| {
        let replacement = plan_edit(engine.state(), request)?;
        next.container_edits.insert(request.file(), replacement.clone());
        json_codec::to_value(Body { request, replacement }).map_err(codec)
    })
}

pub(crate) fn save_receipt(
    engine: &Engine,
    file: storage_contract::FileId,
    revision: &storage_contract::container_edit::DraftRevision,
    principal: &Principal,
) -> crate::Result<Option<OperationResult>> {
    let conflict = || Error::ContainerDrafts(DraftError::ReplayConflict);
    if principal != &revision.principal {
        return Err(conflict());
    }
    let Some(receipt) = engine.state().operations.get(&revision.operation) else {
        return Ok(None);
    };
    let request = EditCommand::Save { file, revision: revision.clone() };
    let body = json_codec::to_value(&request).map_err(codec)?;
    let digest = Engine::native_request_digest(EDIT_KIND, principal.as_str(), &body)?;
    if receipt.kind != EDIT_KIND || receipt.request_digest != digest
        || engine.operation_owner(revision.operation)?.as_deref() != Some(principal.as_str())
        || decode(receipt)?.request != request
    {
        return Err(conflict());
    }
    Ok(Some(receipt.clone()))
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

/// Native workspace digests bind explicit consumption independently of content.
pub(crate) fn bind_workspace_input(
    mut body: json_codec::Value,
    binding: Option<&storage_contract::container_edit::DraftConsumption>,
) -> crate::Result<json_codec::Value> {
    if let Some(binding) = binding {
        body.as_object_mut().ok_or_else(crate::engine::container_drafts::invalid)?
            .insert("container_draft_commit".into(), json_codec::to_value(binding).map_err(codec)?);
    }
    Ok(body)
}

pub(crate) fn consumption(
    receipt: &OperationResult,
) -> crate::Result<Option<storage_contract::container_edit::DraftConsumption>> {
    let Some(value) = receipt.result.get("container_draft_commit") else {
        return Ok(None);
    };
    json_codec::from_value(value.clone()).map(Some).map_err(codec)
}
