use crate::api;
use crate::files::commit_mode::{Pending, Selection, Target};
use data_serialization::Deserialize;
use json_codec::{Value, json};
use storage_contract::file_draft::{
    CommitMode, DraftBase, FileWorkingState, WorkingRevision, automatic::AutomaticController,
};

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid file commit mode JSON: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid automatic working observation: {0}")]
    Controller(#[source] storage_contract::file_draft::automatic::ControllerError),
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
pub(super) struct Born {
    pub(super) base: DraftBase,
    pub(super) working: FileWorkingState,
}

#[derive(Clone)]
pub(super) enum Snapshot {
    Born(Born),
    Unborn(crate::files::unborn::Snapshot),
}
impl Snapshot {
    pub(super) fn mode(&self) -> CommitMode {
        match self {
            Self::Born(b) => b.working.mode(),
            Self::Unborn(b) => b.mode,
        }
    }
    pub(super) fn revision(&self) -> WorkingRevision {
        match self {
            Self::Born(b) => b.working.revision(),
            Self::Unborn(b) => b.revision(),
        }
    }
    pub(super) fn dirty(&self) -> bool {
        match self {
            Self::Born(b) => b.working.draft().is_some(),
            Self::Unborn(b) => b.has_draft(),
        }
    }
    pub(super) fn controller(&self) -> Option<&AutomaticController> {
        match self {
            Self::Born(b) => b.working.automatic_controller(),
            Self::Unborn(b) => b.automatic.as_ref(),
        }
    }
    pub(super) fn parse(value: Value, target: &Target) -> Result<Self, api::Error> {
        let Selection::Born {
            branch,
            node,
            version,
        } = target.selection
        else {
            return crate::files::unborn::Snapshot::parse(value, &target.location)
                .map(Self::Unborn);
        };
        if !value.is_object() || !value["base"].is_object() || !value["working"].is_object() {
            return Err(failure(DecodeError::Relation(
                "named working and base objects are required",
            )));
        }
        let snapshot: Born =
            json_codec::from_value(value).map_err(|source| failure(DecodeError::Wire(source)))?;
        if snapshot.working.unborn().is_some()
            || snapshot.base.branch_id() != branch
            || snapshot.base.node_id() != node
            || snapshot.base.version_id() != version
            || snapshot
                .working
                .draft()
                .is_some_and(|(base, _)| *base != snapshot.base)
        {
            return Err(failure(DecodeError::Relation(
                "working selection differs from this file",
            )));
        }
        if let Some(controller) = snapshot.working.automatic_controller() {
            controller
                .validate(snapshot.working.revision())
                .map_err(|e| failure(DecodeError::Controller(e)))?;
        }
        Ok(Self::Born(snapshot))
    }
}

impl Pending {
    pub(super) fn endpoint(&self) -> String {
        format!(
            "/api/file-manager/files/{}/{}",
            self.command.target().file,
            match self.selection {
                Selection::Born { .. } => "working",
                Selection::Unborn => "unborn-working/mode",
            }
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
