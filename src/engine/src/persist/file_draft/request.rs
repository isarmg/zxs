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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RangeSaveRequest {
    action: RangeSaveAction,
    pub(crate) target: DraftLocation,
    pub(crate) expected: WorkingRevision,
    pub(crate) base: DraftBase,
    pub(crate) manifest_commitment: ManifestCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) logical_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum RangeSaveAction { SaveRanges }

impl RangeSaveRequest {
    pub(crate) fn new(
        target: DraftLocation, expected: WorkingRevision, base: DraftBase,
        manifest_commitment: ManifestCommitment, logical_size: u64,
    ) -> Self {
        Self { action: RangeSaveAction::SaveRanges, target, expected, base,
            manifest_commitment, logical_size }
    }
}

impl Request {
    pub(crate) fn target(&self) -> &DraftLocation {
        match self {
            Self::Command(command) => command.target(),
            Self::Ranges(request) => &request.target,
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Command(command) => command.kind(),
            Self::Ranges(_) => "file.draft.write",
        }
    }
}
