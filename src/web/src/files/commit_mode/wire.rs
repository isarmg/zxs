use crate::api;
use crate::files::commit_mode::{Pending, Target};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::file_draft::{DraftBase, FileWorkingState};

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid file commit mode JSON: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid file commit mode request digest: {0}")]
    Contract(#[source] storage_contract::Error),
    #[error("invalid file commit mode response: {0}")]
    Relation(&'static str),
}

pub(super) fn failure(source: DecodeError) -> api::Error {
    api::Error::invalid_response(
        "The file commit mode response could not be verified.",
        api::ErrorSource::FileCommitMode(source),
    )
}

#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) base: DraftBase,
    pub(super) working: FileWorkingState,
}

impl Snapshot {
    pub(super) fn parse(value: Value, target: &Target) -> Result<Self, api::Error> {
        if !value.is_object() || !value["base"].is_object() || !value["working"].is_object() {
            return Err(failure(DecodeError::Relation(
                "named working and base objects are required",
            )));
        }
        let snapshot: Self =
            json_codec::from_value(value).map_err(|source| failure(DecodeError::Wire(source)))?;
        if snapshot.working.unborn().is_some()
            || snapshot.base.branch_id() != target.branch
            || snapshot.base.node_id() != target.node
            || snapshot.base.version_id() != target.version
            || snapshot
                .working
                .draft()
                .is_some_and(|(base, _)| *base != snapshot.base)
        {
            return Err(failure(DecodeError::Relation(
                "working selection differs from this file",
            )));
        }
        Ok(snapshot)
    }
}

impl Pending {
    pub(super) fn endpoint(&self) -> String {
        format!(
            "/api/file-manager/files/{}/working",
            self.command.target().file
        )
    }

    pub(super) fn body(&self) -> Result<Value, api::Error> {
        json_codec::to_value(&self.command)
            .map(|command| json!({"operation_id":self.operation, "command":command}))
            .map_err(|source| failure(DecodeError::Wire(source)))
    }

    pub(super) fn receipt_matches(&self, value: &Value) -> Result<bool, api::Error> {
        let command = json_codec::to_value(&self.command)
            .map_err(|source| failure(DecodeError::Wire(source)))?;
        let digest = storage_contract::object_hash(
            "request",
            &json!({
                "kind":self.command.kind(), "principal":self.principal, "body":command,
            }),
        )
        .map_err(|source| failure(DecodeError::Contract(source)))?;
        Ok(value["operation_id"] == json!(self.operation)
            && value["operation_kind"] == self.command.kind()
            && value["request_digest"] == digest
            && value["result"]["request"] == command)
    }
}
