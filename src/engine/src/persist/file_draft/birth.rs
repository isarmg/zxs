//! Strict request and original allocation bytes for the actual birth planner.
//! Decoding this receipt supplies no physical container or root authority.
use crate::business::file_draft::invalid;
use crate::{Engine, Error, OperationResult, Result};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{
    CommitState, CurrentHealth, DiskId, OperationId, Principal, ProtectionAtPublish, Sequence,
};

pub(crate) const KIND: &str = "file.working.birth";

#[derive(Serialize)]
struct Outcome<'a> {
    request: &'a BirthRequest,
    target: &'a DraftLocation,
    owner: DiskId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    request: BirthRequest,
    target: DraftLocation,
    owner: DiskId,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    // Birth cannot claim protection for a non-existent historical version.
    version_protection: [(); 0],
}

fn codec(source: json_codec::Error) -> Error {
    Error::FileDraft(crate::DraftError::Encoding(source))
}

pub(crate) fn request(request: &BirthRequest) -> Result<json_codec::Value> {
    json_codec::to_value(request).map_err(codec)
}

/// The normal planner adds timestamp/epoch, permanent principal and event data.
/// No physical replicas or caller-supplied inode is encoded as accepted feedback.
pub(crate) fn encode(
    request: &BirthRequest,
    target: &DraftLocation,
    owner: DiskId,
) -> Result<json_codec::Value> {
    json_codec::to_value(Outcome { request, target, owner }).map_err(codec)
}

pub(crate) struct Decoded(Envelope);

impl Decoded {
    pub(crate) fn request(&self) -> &BirthRequest { &self.0.request }
    pub(crate) fn target(&self) -> &DraftLocation { &self.0.target }
    pub(crate) fn owner(&self) -> DiskId { self.0.owner }
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
    if envelope.request.repository != envelope.target.repository
        || envelope.request.path != envelope.target.path
    {
        return Err(invalid());
    }
    Ok(Decoded(envelope))
}
