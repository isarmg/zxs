//! Canonical request and receipt encoding for the container-label mutation.
use crate::{Engine, Error, OperationId, OperationResult, Result};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_container_tags::{KIND, ReplaceCommand, Replacement, TagSet};
use storage_contract::{Generation, Principal};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    request: ReplaceCommand,
    tags: TagSet,
    generation: Generation,
    changed: bool,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<()>,
}

fn codec(source: json_codec::Error) -> Error {
    Error::ContainerTags(crate::ContainerTagsError::Encoding(source))
}

pub(crate) fn replace(
    engine: &mut Engine,
    request: &ReplaceCommand,
    principal: &Principal,
    operation: OperationId,
) -> Result<OperationResult> {
    let body = json_codec::to_value(request).map_err(codec)?;
    engine.mutate(KIND, principal.as_str(), operation, body, |engine, next, _| {
        let replacement = crate::business::file_tags::plan(
            engine.state(),
            engine.admitted_current_namespaces()?,
            request,
        )?;
        crate::business::file_tags::apply(next, &replacement);
        json_codec::to_value(replacement).map_err(codec)
    })
}

pub(crate) fn decode(receipt: &OperationResult) -> Result<(Replacement, u64, u64)> {
    let envelope: Envelope = json_codec::from_value(receipt.result.clone()).map_err(codec)?;
    if !envelope.version_protection.is_empty() {
        return Err(crate::business::file_tags::invalid());
    }
    Ok((
        Replacement {
            request: envelope.request,
            tags: envelope.tags,
            generation: envelope.generation,
            changed: envelope.changed,
        },
        envelope.timestamp,
        envelope.membership_epoch,
    ))
}

pub(crate) fn digest(request: &ReplaceCommand, principal: &str) -> Result<String> {
    let body = json_codec::to_value(request).map_err(codec)?;
    Engine::native_request_digest(KIND, principal, &body)
}

pub(crate) fn encoded(
    replacement: Replacement,
    timestamp: u64,
    membership_epoch: u64,
) -> Result<json_codec::Value> {
    json_codec::to_value(Envelope {
        request: replacement.request,
        tags: replacement.tags,
        generation: replacement.generation,
        changed: replacement.changed,
        timestamp,
        membership_epoch,
        version_protection: Vec::new(),
    })
    .map_err(codec)
}
