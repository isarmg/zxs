//! Original explicit first-commit request and exact constructor facts.
//! Decoding descriptions never grants source, Version or working-clear authority.
use super::{Protection, codec, content_protection};
use crate::business::file_draft::{PublishedContent, invalid, trace_limit, origin::{NodeOrigin, Reason}};
use crate::{Engine, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::manifest::{ManifestCommitment, SaveGeneration};
use storage_contract::file_draft::{
    DraftLocation, WorkingContentExpectation, WorkingExpectation, WorkingRevision, WorkingSelection,
};
use storage_contract::{
    CommitState, CurrentHealth, OperationId, Principal, ProtectionAtPublish, Sequence,
};

pub(crate) const KIND: &str = "file.working.first_commit";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedBody {
    Empty {},
    Saved { generation: SaveGeneration, commitment: ManifestCommitment },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    target: DraftLocation,
    expected: WorkingRevision,
    body: ExpectedBody,
    message: String,
}

impl Request {
    fn from_selection(selection: &WorkingSelection, message: &str) -> Result<Self> {
        let body = match selection.expected.content {
            WorkingContentExpectation::UnbornEmpty => ExpectedBody::Empty {},
            WorkingContentExpectation::UnbornSaved { generation, commitment } => {
                ExpectedBody::Saved { generation, commitment }
            }
            WorkingContentExpectation::Born { .. } => return Err(invalid()),
        };
        Ok(Self {
            target: selection.target.clone(), expected: selection.expected.revision,
            body, message: message.into(),
        })
    }

    fn selection(&self) -> WorkingSelection {
        WorkingSelection {
            target: self.target.clone(),
            expected: WorkingExpectation {
                revision: self.expected,
                content: match self.body {
                    ExpectedBody::Empty {} => WorkingContentExpectation::UnbornEmpty,
                    ExpectedBody::Saved { generation, commitment } => {
                        WorkingContentExpectation::UnbornSaved { generation, commitment }
                    }
                },
            },
        }
    }
}

#[derive(Serialize)]
struct Outcome<'a> {
    request: Request,
    content: &'a PublishedContent,
    origin: &'a NodeOrigin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    request: Request,
    content: PublishedContent,
    origin: NodeOrigin,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: [Protection; 1],
}

pub(crate) fn request(selection: &WorkingSelection, message: &str) -> Result<json_codec::Value> {
    json_codec::to_value(Request::from_selection(selection, message)?).map_err(codec)
}

pub(crate) fn encode(
    selection: &WorkingSelection, message: &str, content: &PublishedContent, origin: &NodeOrigin,
) -> Result<json_codec::Value> {
    json_codec::to_value(Outcome {
        request: Request::from_selection(selection, message)?, content, origin,
    }).map_err(codec)
}

pub(crate) struct Decoded(Envelope);

impl Decoded {
    pub(crate) fn selection(&self) -> WorkingSelection { self.0.request.selection() }
    pub(crate) fn content(&self) -> &PublishedContent { &self.0.content }
    pub(crate) fn message(&self) -> &str { &self.0.request.message }
    pub(crate) fn epoch(&self) -> u64 { self.0.membership_epoch }

    pub(crate) fn receipt(
        &self, previous: &State, operation: OperationId, sequence: Sequence, principal: &Principal,
    ) -> Result<OperationResult> {
        let protection = content_protection(previous, &self.0.request.target, &self.0.content)?;
        let origin = NodeOrigin::from_content(previous, &self.0.request.target, &self.0.content,
            Reason::ManualFile {}, operation, sequence,
            storage_contract::Timestamp::from_unix_seconds(self.0.timestamp))?;
        if self.0.version_protection != [protection] || self.0.origin != origin { return Err(invalid()); }
        Ok(OperationResult {
            operation_id: operation, kind: KIND.into(),
            commit_id: None, history_tree_id: None, branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::TargetMet,
            current_health: CurrentHealth::Healthy,
            request_digest: Engine::native_request_digest(
                KIND, principal.as_str(),
                &request(&self.selection(), &self.0.request.message)?,
            )?,
            sequence: sequence.get(), result: json_codec::to_value(&self.0).map_err(codec)?,
        })
    }
}

pub(crate) fn decode(previous: &State, receipt: &OperationResult) -> Result<Decoded> {
    if receipt.kind != KIND { return Err(invalid()); }
    let object = receipt.result.as_object().ok_or_else(invalid)?;
    let required = ["request", "content", "origin", "timestamp", "membership_epoch", "version_protection"];
    if object.len() != required.len() || required.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid());
    }
    let request: Request = json_codec::from_value(object["request"].clone()).map_err(codec)?;
    let content = &object["content"];
    let version = content.get("version").ok_or_else(invalid)?;
    let placement = content.get("placement").ok_or_else(invalid)?;
    if !version.get("source_version_ids").and_then(json_codec::Value::as_array)
        .ok_or_else(invalid)?.is_empty()
    {
        return Err(invalid());
    }
    let protection = object["version_protection"].as_array().ok_or_else(invalid)?;
    if protection.len() != 1 { return Err(invalid()); }
    for list in [
        placement.get("route_slot_ids"), protection[0].get("required_slots"),
        protection[0].get("slots"),
    ] {
        if list.and_then(json_codec::Value::as_array).ok_or_else(invalid)?.len() != 1 {
            return Err(invalid());
        }
    }
    if placement.get("binding_generations").and_then(json_codec::Value::as_object)
        .ok_or_else(invalid)?.len() != 1
        || content.get("allocations").and_then(json_codec::Value::as_array)
            .ok_or_else(invalid)?.len() > trace_limit(previous, &request.target)?
    {
        return Err(invalid());
    }
    Ok(Decoded(json_codec::from_value(receipt.result.clone()).map_err(codec)?))
}
