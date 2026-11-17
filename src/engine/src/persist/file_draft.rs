//! Mandatory native request/receipt bytes for one shared container draft.
pub(crate) mod birth;
pub(crate) mod unborn_save;
pub(crate) mod unborn_first;

use crate::business::file_draft::{Outcome, invalid, trace_limit, origin::{NodeOrigin, Reason}};
use crate::{Engine, Error, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{DraftCommand, FileWorkingState};
mod request;
pub(crate) use request::{Request, RangeSaveRequest, AutomaticRequest};
use storage_contract::{CommitState, CurrentHealth, DiskId, OperationId, ProtectionAtPublish};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    request: Request,
    working: FileWorkingState,
    content: Option<crate::business::file_draft::PublishedContent>,
    origin: Option<NodeOrigin>,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    version_protection: Vec<Protection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Protection {
    version_id: storage_contract::VersionId,
    primary_slot_id: name_policy::SlotId,
    target_copies: u32,
    required_slots: Vec<name_policy::SlotId>,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
    slots: Vec<Slot>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Slot {
    slot_id: name_policy::SlotId,
    copy_level: u32,
    disk_id: DiskId,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
    state: SlotState,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
enum SlotState {
    CompleteVerified,
}

fn codec(source: json_codec::Error) -> Error {
    Error::FileDraft(crate::DraftError::Encoding(source))
}

pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(kind, "file.working.mode" | "file.draft.write"
        | "file.draft.discard" | "file.draft.commit")
}

pub(crate) fn request(request: &impl Serialize) -> Result<json_codec::Value> {
    json_codec::to_value(request).map_err(codec)
}

pub(crate) fn digest(command: &Request, principal: &str) -> Result<String> {
    Engine::native_request_digest(command.kind(), principal, &request(command)?)
}

pub(crate) fn encode(outcome: Outcome) -> Result<json_codec::Value> {
    json_codec::to_value(outcome).map_err(codec)
}

pub(crate) fn content_protection(
    previous: &State,
    target: &storage_contract::file_draft::DraftLocation,
    content: &crate::business::file_draft::PublishedContent,
) -> Result<Protection> {
    content_protection_for_file(previous, target.file, content)
}

pub(crate) fn content_protection_for_file(
    previous: &State, file: storage_contract::FileId,
    content: &crate::business::file_draft::PublishedContent,
) -> Result<Protection> {
    let placement = &content.placement;
    let owner = previous.file_containers.get(&file).ok_or_else(invalid)?.owner_disk;
    let slot = placement.route_slot_ids.first().ok_or_else(invalid)?;
    if placement.route_slot_ids.len() != 1 || placement.target_copies != 1
        || placement.primary_slot_id != *slot
    {
        return Err(invalid());
    }
    Ok(Protection {
        version_id: content.version.version_id.parse()?,
        primary_slot_id: slot.parse()?, target_copies: 1,
        required_slots: vec![slot.parse()?],
        topology_generation: placement.topology_generation,
        slots: vec![Slot {
            slot_id: slot.parse()?, copy_level: 1, disk_id: owner,
            binding_generation: *placement.binding_generations.get(slot).ok_or_else(invalid)?,
            state: SlotState::CompleteVerified,
        }],
    })
}

pub(crate) struct Decoded {
    outcome: Outcome,
    timestamp: u64,
    epoch: u64,
    protection: Vec<Protection>,
}
impl Decoded {
    pub(crate) fn timestamp(&self) -> storage_contract::Timestamp { storage_contract::Timestamp::from_unix_seconds(self.timestamp) }

    pub(crate) fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub(crate) fn receipt(
        &self,
        previous: &State,
        operation: OperationId,
        sequence: u64,
        principal: &str,
    ) -> Result<OperationResult> {
        let expected = match &self.outcome.content {
            None => Vec::new(),
            Some(content) => vec![content_protection(
                previous, self.outcome.request.target(), content,
            )?],
        };
        let origin = self.outcome.content.as_ref().map(|body| NodeOrigin::from_content(
            previous, self.outcome.request.target(), body, self.outcome.request.reason()?,
            operation, storage_contract::Sequence::new(sequence), self.timestamp(),
        )).transpose()?;
        if self.protection != expected || self.outcome.origin != origin { return Err(invalid()); }
        let result = json_codec::to_value(Envelope {
            request: self.outcome.request.clone(), working: self.outcome.working.clone(),
            content: self.outcome.content.clone(), origin, timestamp: self.timestamp,
            membership_epoch: self.epoch, version_protection: expected,
        }).map_err(codec)?;
        Ok(OperationResult {
            operation_id: operation, kind: self.outcome.request.kind().into(),
            commit_id: None, history_tree_id: None, branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: if self.outcome.content.is_some() {
                ProtectionAtPublish::TargetMet
            } else {
                ProtectionAtPublish::NotApplicable
            },
            current_health: CurrentHealth::Healthy,
            request_digest: digest(&self.outcome.request, principal)?,
            sequence, result,
        })
    }
}

pub(crate) fn decode(previous: &State, receipt: &OperationResult) -> Result<Decoded> {
    if !is_kind(&receipt.kind) { return Err(invalid()); }
    let object = receipt.result.as_object().ok_or_else(invalid)?;
    let required = ["request", "working", "content", "origin", "timestamp", "membership_epoch",
        "version_protection"];
    if object.len() != required.len() || required.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid());
    }
    let request: Request = json_codec::from_value(object["request"].clone()).map_err(codec)?;
    if request.kind() != receipt.kind { return Err(invalid()); }
    let content = &object["content"];
    let has_content = !content.is_null();
    if has_content && !matches!(&request, Request::Command(DraftCommand::Commit { .. }) | Request::Automatic(_)) { return Err(invalid()); }
    let protection = object["version_protection"].as_array().ok_or_else(invalid)?;
    if protection.len() != usize::from(has_content) { return Err(invalid()); }
    if has_content {
        // This first scope has one source version and exactly one owner slot.
        // Bound every nested list before allocating the typed receipt mirror.
        let version = content.get("version").ok_or_else(invalid)?;
        let placement = content.get("placement").ok_or_else(invalid)?;
        for list in [
            version.get("source_version_ids"), placement.get("route_slot_ids"),
            protection[0].get("required_slots"), protection[0].get("slots"),
        ] {
            if list.and_then(json_codec::Value::as_array).ok_or_else(invalid)?.len() != 1 {
                return Err(invalid());
            }
        }
        if placement.get("binding_generations").and_then(json_codec::Value::as_object)
            .ok_or_else(invalid)?.len() != 1
        {
            return Err(invalid());
        }
        let allocations = content.get("allocations")
            .and_then(json_codec::Value::as_array).ok_or_else(invalid)?;
        if allocations.len() > trace_limit(previous, request.target())? {
            return Err(Error::FileDraft(crate::DraftError::Size));
        }
    }
    let envelope: Envelope = json_codec::from_value(receipt.result.clone()).map_err(codec)?;
    Ok(Decoded {
        outcome: Outcome {
            request: envelope.request, working: envelope.working, content: envelope.content, origin: envelope.origin,
        },
        timestamp: envelope.timestamp, epoch: envelope.membership_epoch,
        protection: envelope.version_protection,
    })
}

/// Derive past immutable payload identities from the one permanent receipt
/// source, including superseded/discarded drafts. Never scan or adopt orphans.
/// This private first implementation is O(past draft writes); no cache is an
/// independent authority and no old receipt may silently fail decoding.
pub(crate) fn payload_was_published(
    previous: &State,
    payload: storage_contract::file_draft::DraftPayloadId,
) -> Result<bool> {
    for receipt in previous.operations.values().filter(|r| r.kind == "file.draft.write") {
        let decoded = decode(previous, receipt)?;
        if decoded.outcome.content.is_some() { return Err(invalid()); }
        let (_, old) = decoded.outcome.working.draft().ok_or_else(invalid)?;
        if old.payload_id() == payload { return Ok(true); }
    }
    Ok(false)
}

/// Project only a previously admitted original. Unlike transition replay this
/// must not resolve the saved path in today's (possibly moved) namespace.
pub(crate) fn observed_node(original: &OperationResult) -> Result<Option<NodeOrigin>> {
    let object = original.result.as_object().ok_or_else(invalid)?;
    let value = object.get("request").ok_or_else(invalid)?;
    let (target, reason) = match original.kind.as_str() {
        "file.draft.commit" => {
            let req: Request = json_codec::from_value(value.clone()).map_err(codec)?;
            (req.target().clone(), req.reason()?)
        }
        unborn_first::KIND => unborn_first::read_reason(value)?,
        _ => return Err(invalid()),
    };
    let content = object.get("content").ok_or_else(invalid)?;
    let raw = object.get("origin").ok_or_else(invalid)?;
    if raw.is_null() && content.is_null() && original.kind == "file.draft.commit" { return Ok(None); }
    if content.is_null() || raw.get("ancestors").and_then(json_codec::Value::as_array)
        .is_none_or(|ids| ids.len() > 31)
    { return Err(invalid()); }
    let origin: NodeOrigin = json_codec::from_value(raw.clone()).map_err(codec)?;
    let version = content.get("version").ok_or_else(invalid)?;
    let timestamp: storage_contract::Timestamp = json_codec::from_value(object.get("timestamp")
        .ok_or_else(invalid)?.clone()).map_err(codec)?;
    if origin.operation != original.operation_id || origin.sequence.get() != original.sequence
        || origin.published_at != timestamp || origin.target != target || origin.reason != reason
        || version.get("version_id").and_then(json_codec::Value::as_str) != Some(origin.node.to_string().as_str())
        || version.get("file_id").and_then(json_codec::Value::as_str) != Some(origin.target.file.to_string().as_str())
    { return Err(invalid()); }
    Ok(Some(origin))
}
