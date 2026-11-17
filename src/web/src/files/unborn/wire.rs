//! Strict current response and immutable original HTTP request descriptions.
use crate::api;
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use storage_contract::file_draft::manifest::{ManifestCommitment, SaveGeneration};
use storage_contract::file_draft::{CommitMode, DraftLocation, WorkingRevision};
use storage_contract::{OperationId, Principal};

pub(super) const KIND: &str = "file.working.first_commit";
const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Content {
    Empty {},
    Saved { generation: SaveGeneration, commitment: ManifestCommitment },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Expectation {
    pub(super) revision: WorkingRevision,
    pub(super) content: Content,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) target: DraftLocation,
    pub(super) expected: Expectation,
    pub(super) mode: CommitMode,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) logical_size: u64,
}
impl Snapshot {
    pub(super) fn parse(value: Value, target: &DraftLocation) -> Result<Self, api::Error> {
        if !value.is_object()
            || !value.get("target").is_some_and(Value::is_object)
            || !value.get("expected").is_some_and(Value::is_object)
            || !value.get("expected").and_then(|item| item.get("content"))
                .is_some_and(Value::is_object)
        {
            return Err(invalid("The working response must be an object."));
        }
        let snapshot: Self = json_codec::from_value(value).map_err(decode)?;
        if &snapshot.target != target
            || matches!(snapshot.expected.content, Content::Empty {}) && snapshot.logical_size != 0
        {
            return Err(invalid("The working response differs from this exact file occurrence."));
        }
        Ok(snapshot)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Command {
    pub(super) operation_id: OperationId,
    pub(super) target: DraftLocation,
    pub(super) expected: Expectation,
    pub(super) message: String,
}
impl Command {
    pub(super) fn endpoint(&self) -> String {
        format!("/api/file-manager/files/{}/first-commit", self.target.file)
    }
    pub(super) fn body(&self) -> Result<Value, api::Error> {
        json_codec::to_value(self).map_err(decode)
    }
    fn native_request(&self) -> Value {
        let body = match self.expected.content {
            Content::Empty {} => json!({"kind":"empty"}),
            Content::Saved { generation, commitment } => json!({
                "kind":"saved", "generation":generation, "commitment":commitment,
            }),
        };
        json!({
            "target":self.target, "expected":self.expected.revision,
            "body":body, "message":self.message,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "unborn-first-commit-original-request")]
    Original,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: Format,
    pub(super) principal: Principal,
    pub(super) command: Command,
}
impl Record {
    pub(super) fn new(principal: Principal, command: Command) -> Result<Self, api::Error> {
        storage_contract::validate_note(&command.message).map_err(record_error)?;
        Ok(Self { format: Format::Original, principal, command })
    }
    pub(super) fn encode(&self) -> Result<String, api::Error> {
        let bytes = json_codec::to_string(self).map_err(decode)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("The first-commit original request exceeds its storage budget."));
        }
        Ok(bytes)
    }
    pub(super) fn parse(bytes: &str, principal: &Principal) -> Result<Self, api::Error> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("The original request is too large; it was not removed."));
        }
        let value = storage_contract::strict_json(bytes.as_bytes()).map_err(record_error)?;
        if !value.is_object()
            || !value.get("command").is_some_and(Value::is_object)
            || !value["command"].get("target").is_some_and(Value::is_object)
            || !value["command"].get("expected").is_some_and(Value::is_object)
            || !value["command"]["expected"].get("content").is_some_and(Value::is_object)
        {
            return Err(invalid("The original request must retain its exact object fields."));
        }
        let record: Self = json_codec::from_value(value).map_err(decode)?;
        if &record.principal != principal {
            return Err(invalid("The original first-commit request belongs to another account."));
        }
        storage_contract::validate_note(&record.command.message).map_err(record_error)?;
        Ok(record)
    }
    pub(super) fn receipt_status(&self, value: &Value)
        -> Result<Option<storage_contract::client_protocol::OperationStatus>, api::Error>
    {
        let native = self.command.native_request();
        let digest = storage_contract::object_hash("request", &json!({
            "kind":KIND, "principal":self.principal, "body":native,
        })).map_err(record_error)?;
        if value["operation_id"] != json!(self.command.operation_id)
            || value["operation_kind"] != KIND || value["request_digest"] != digest
            || value["result"]["request"] != native
        {
            return Ok(None);
        }
        Ok(storage_contract::client_protocol::operation_status(value))
    }
}
pub(super) fn invalid(message: &str) -> api::Error {
    api::error(storage_contract::ErrorCode::InvalidRequest, message)
}
fn decode(source: json_codec::Error) -> api::Error {
    api::Error::invalid_response(
        "The first-commit response or original request is invalid.",
        api::ErrorSource::FirstCommitWire(source),
    )
}
fn record_error(source: storage_contract::Error) -> api::Error {
    api::Error::invalid_response(
        "The original first-commit request could not be verified.",
        api::ErrorSource::FirstCommitRecord(source),
    )
}
