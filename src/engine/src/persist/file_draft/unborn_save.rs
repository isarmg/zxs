//! The original accepted edit batch, not a transport frame or a FULL version.
use crate::business::file_draft::invalid;
use crate::{Engine, Error, OperationResult, Result};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::UnbornSaveRequest;
use storage_contract::{
    CommitState, CurrentHealth, OperationId, Principal, ProtectionAtPublish, Sequence,
};

pub(crate) const KIND: &str = "file.working.save";

#[derive(Serialize)]
struct Outcome<'a> {
    request: &'a UnbornSaveRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    request: UnbornSaveRequest,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: [(); 0],
}

fn codec(source: json_codec::Error) -> Error {
    Error::FileDraft(crate::DraftError::Encoding(source))
}

pub(crate) fn request(request: &UnbornSaveRequest) -> Result<json_codec::Value> {
    json_codec::to_value(request).map_err(codec)
}

pub(crate) fn encode(request: &UnbornSaveRequest) -> Result<json_codec::Value> {
    json_codec::to_value(Outcome { request }).map_err(codec)
}

pub(crate) struct Decoded(Envelope);

impl Decoded {
    pub(crate) fn request(&self) -> &UnbornSaveRequest { &self.0.request }
    pub(crate) fn timestamp(&self) -> storage_contract::Timestamp { storage_contract::Timestamp::from_unix_seconds(self.0.timestamp) }
    pub(crate) fn epoch(&self) -> u64 { self.0.membership_epoch }

    pub(crate) fn receipt(
        &self,
        operation: OperationId,
        sequence: Sequence,
        principal: &Principal,
    ) -> Result<OperationResult> {
        Ok(OperationResult {
            operation_id: operation,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest(
                KIND, principal.as_str(), &request(&self.0.request)?,
            )?,
            sequence: sequence.get(),
            result: json_codec::to_value(&self.0).map_err(codec)?,
        })
    }
}

pub(crate) fn decode(receipt: &OperationResult) -> Result<Decoded> {
    if receipt.kind != KIND { return Err(invalid()); }
    let envelope: Envelope = json_codec::from_value(receipt.result.clone()).map_err(codec)?;
    if envelope.request.target.file != envelope.request.manifest.file()
        || receipt.operation_id != envelope.request.manifest.operation()
    {
        return Err(invalid());
    }
    Ok(Decoded(envelope))
}
