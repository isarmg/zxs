//! Native range saves have no HTTP body hash and cannot use the upload command.
//! This mirror grants no material authority; the live extent producer is mandatory.
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{DraftBase, DraftCommand, DraftLocation, WorkingRevision};
use storage_contract::file_draft::manifest::ManifestCommitment;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(untagged)]
pub(crate) enum Request {
    Command(DraftCommand),
    Ranges(RangeSaveRequest),
    Automatic(AutomaticRequest),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RangeSaveRequest {
    action: RangeSaveAction,
    pub(crate) target: DraftLocation,
    pub(crate) expected: WorkingRevision,
    pub(crate) controller: storage_contract::file_draft::SaveController,
    pub(crate) base: DraftBase,
    pub(crate) manifest_commitment: ManifestCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) logical_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum RangeSaveAction { SaveRanges }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AutomaticRequest {
    pub(crate) target: DraftLocation,
    pub(crate) base: DraftBase,
    pub(crate) payload: storage_contract::file_draft::DraftPayload,
    pub(crate) saved: crate::business::file_draft::automatic::Saved,
}
impl AutomaticRequest {
    pub(crate) fn command(&self) -> DraftCommand {
        DraftCommand::Commit { target: self.target.clone(), expected: self.saved.decision.revision,
            base: self.base, message: crate::business::file_draft::automatic::MESSAGE.into() }
    }
    pub(crate) fn require_body(&self, state: &crate::State) -> crate::Result<()> {
        let record = state.file_containers.get(&self.target.file)
            .ok_or_else(crate::business::file_draft::invalid)?;
        if record.working.draft() != Some((&self.base, &self.payload)) {
            return Err(crate::business::file_draft::automatic::stale());
        }
        Ok(())
    }
}

impl RangeSaveRequest {
    pub(crate) fn new(
        target: DraftLocation, expected: WorkingRevision, base: DraftBase,
        manifest_commitment: ManifestCommitment, logical_size: u64,
        controller: storage_contract::file_draft::SaveController,
    ) -> Self {
        Self { action: RangeSaveAction::SaveRanges, target, expected, controller, base,
            manifest_commitment, logical_size }
    }
}

impl Request {
    pub(crate) fn reason(&self) -> crate::Result<crate::business::file_draft::origin::Reason> {
        match self {
            Self::Command(DraftCommand::Commit { .. }) => Ok(super::Reason::ManualFile {}),
            Self::Automatic(request) => Ok(request.saved.reason()),
            _ => Err(crate::business::file_draft::invalid()),
        }
    }

    pub(crate) fn target(&self) -> &DraftLocation {
        match self {
            Self::Command(command) => command.target(),
            Self::Ranges(request) => &request.target,
            Self::Automatic(request) => &request.target,
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Command(command) => command.kind(),
            Self::Ranges(_) => "file.draft.write",
            Self::Automatic(_) => "file.draft.commit",
        }
    }
}
