//! Private description codec, not registered in native operation dispatch.
//! Decoding never supplies a FILE batch, a working clear, or a node publication.
pub(crate) mod native;

use crate::persist::snapshot_worktree::HostEvidence;
use crate::working_tree::snapshot::{
    MAX_ENTRIES, SnapshotEntry, SnapshotFile, SnapshotNodeBinding, SnapshotReference,
};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeSet;
use storage_contract::file_draft::{
    DraftBase, DraftLocation, DraftPayload, WorkingContentExpectation,
    WorkingExpectation, WorkingRevision, WorkingSelection,
};
use storage_contract::file_draft::manifest::{ManifestCommitment, SaveGeneration};
use storage_contract::snapshot_worktree::{
    NamespaceDirectoryId, SnapshotHandle, SnapshotHost,
};
use storage_contract::working_tree::SnapshotNodeId;
use storage_contract::{BranchId, FileNodeId, Generation, OperationId, Principal};

#[derive(Debug, error_definition::Error)]
pub(crate) enum DescriptionError {
    #[error("directory description exceeds its actual record or entry budget")]
    Budget,
    #[error("directory description is not strict canonical data")]
    Decode(#[source] storage_contract::Error),
    #[error("encoding a directory description")]
    Encode(#[source] storage_contract::Error),
    #[error("directory description has noncanonical bytes")]
    Canonical,
    #[error("directory description has repeated or conflicting selection keys")]
    Selection,
    #[error("directory description differs from its complete source reference")]
    Source,
    #[error("directory file descriptions do not exactly cover the selected set")]
    Cardinality,
    #[error("directory file description has an invalid outcome category or identity")]
    Outcome,
}

type Result<T> = std::result::Result<T, DescriptionError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum RequestFormat {
    DirectoryDraftRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SourceDescription {
    pub(crate) handle: SnapshotHandle,
    pub(crate) binding: SnapshotNodeBinding,
    pub(crate) host: HostEvidence,
    pub(crate) root: SnapshotHost,
    pub(crate) namespace_generation: Generation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ExpectedContent {
    Born { base: DraftBase, payload: DraftPayload },
    // An empty struct keeps surplus fields rejected for the tagged variant.
    UnbornEmpty {},
    UnbornSaved { generation: SaveGeneration, commitment: ManifestCommitment },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SelectionDescription {
    pub(crate) target: DraftLocation,
    pub(crate) revision: WorkingRevision,
    pub(crate) content: ExpectedContent,
}

impl SelectionDescription {
    pub(crate) fn from_selection(selection: &WorkingSelection) -> Self {
        let content = match selection.expected.content {
            WorkingContentExpectation::Born { base, payload } => {
                ExpectedContent::Born { base, payload }
            }
            WorkingContentExpectation::UnbornEmpty => ExpectedContent::UnbornEmpty {},
            WorkingContentExpectation::UnbornSaved { generation, commitment } => {
                ExpectedContent::UnbornSaved { generation, commitment }
            }
        };
        Self {
            target: selection.target.clone(), revision: selection.expected.revision, content,
        }
    }

    pub(crate) fn expectation(&self) -> WorkingExpectation {
        let content = match self.content {
            ExpectedContent::Born { base, payload } => {
                WorkingContentExpectation::Born { base, payload }
            }
            ExpectedContent::UnbornEmpty {} => WorkingContentExpectation::UnbornEmpty,
            ExpectedContent::UnbornSaved { generation, commitment } => {
                WorkingContentExpectation::UnbornSaved { generation, commitment }
            }
        };
        WorkingExpectation { revision: self.revision, content }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RequestDescription {
    pub(crate) format: RequestFormat,
    pub(crate) operation: OperationId,
    pub(crate) principal: Principal,
    pub(crate) source: SourceDescription,
    pub(crate) selected: Vec<SelectionDescription>,
    pub(crate) message: String,
}

impl RequestDescription {
    pub(crate) fn validate(&self, maximum_entries: usize) -> Result<()> {
        let source = &self.source;
        let reference = source.binding.reference();
        if self.operation == source.handle.publication
            || reference.repository != source.handle.repository_id
            || reference.node != source.handle.node_id
            || source.root.directory_id != source.host.common_data.directory_id
            || !source.host.directories.is_empty()
            || source.host.namespace.directory
                != NamespaceDirectoryId::Repository(source.host.namespace.repository_id)
        {
            return Err(DescriptionError::Source);
        }
        if self.selected.is_empty() || self.selected.len() > maximum_entries.min(MAX_ENTRIES) {
            return Err(DescriptionError::Budget);
        }
        let mut files = BTreeSet::new();
        let mut entries = BTreeSet::new();
        let mut paths = Vec::with_capacity(self.selected.len());
        for selection in &self.selected {
            let target = &selection.target;
            if target.repository != source.host.namespace.repository_id
                || target.path.parent().is_some()
                || !files.insert(target.file) || !entries.insert(target.entry)
            {
                return Err(DescriptionError::Selection);
            }
            paths.push(&target.path);
        }
        paths.sort_by(|left, right| left.key().cmp(right.key()));
        if paths.windows(2).any(|pair| pair[0].key() == pair[1].key()) {
            return Err(DescriptionError::Selection);
        }
        Ok(())
    }
}

/// These are descriptions of claimed results, not constructors of a batch.
/// This private directory portion deliberately has no working_after field:
/// only the future actual FILE batch can define and supply that native result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FileOutcome {
    Retained { base: DraftBase, material: SnapshotFile },
    Published {
        base: DraftBase, branch: BranchId, node: FileNodeId, material: SnapshotFile,
    },
    FirstPublished { branch: BranchId, node: FileNodeId, material: SnapshotFile },
}

impl FileOutcome {
    pub(crate) fn material(self) -> SnapshotFile {
        match self {
            Self::Retained { material, .. } | Self::Published { material, .. }
            | Self::FirstPublished { material, .. } => material,
        }
    }

    pub(crate) fn source(self) -> (BranchId, FileNodeId) {
        match self {
            Self::Retained { base, .. } => (base.branch_id(), base.node_id()),
            Self::Published { branch, node, .. } | Self::FirstPublished { branch, node, .. } => {
                (branch, node)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SelectedOutcome {
    pub(crate) target: DraftLocation,
    pub(crate) outcome: FileOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ConstructionDescription {
    pub(crate) request: RequestDescription,
    pub(crate) node: SnapshotNodeId,
    pub(crate) files: Vec<SelectedOutcome>,
}

impl ConstructionDescription {
    pub(crate) fn validate(&self, maximum_entries: usize) -> Result<()> {
        self.request.validate(maximum_entries)?;
        if self.node == self.request.source.handle.node_id {
            return Err(DescriptionError::Source);
        }
        validate_files(&self.request, &self.files)
    }
}

fn validate_files(request: &RequestDescription, files: &[SelectedOutcome]) -> Result<()> {
    if files.len() != request.selected.len() {
        return Err(DescriptionError::Cardinality);
    }
    let mut versions = BTreeSet::new();
    for (selected, result) in request.selected.iter().zip(files) {
        let material = result.outcome.material();
        if result.target != selected.target
            || material.file_id != selected.target.file
            || !versions.insert(material.version_id)
        {
            return Err(DescriptionError::Outcome);
        }
        match (selected.content, result.outcome) {
            (ExpectedContent::Born { base, .. }, FileOutcome::Retained {
                base: actual, material,
            }) if base == actual && material.version_id == base.version_id() => {}
            (ExpectedContent::Born { base, .. }, FileOutcome::Published {
                base: actual, branch, node: FileNodeId::Version(version), material,
            }) if base == actual && version == material.version_id
                && version != base.version_id() && branch == base.branch_id() => {}
            (ExpectedContent::UnbornEmpty {} | ExpectedContent::UnbornSaved { .. },
                FileOutcome::FirstPublished {
                    node: FileNodeId::Version(version), material, ..
                }) if version == material.version_id => {}
            _ => return Err(DescriptionError::Outcome),
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum DecisionFormat {
    DirectoryDraftDecision,
}

/// No OperationResult adapter exists here. In particular this record's binding
/// is an untrusted claim until independent original Candidate/source admission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DecisionDescription {
    pub(crate) format: DecisionFormat,
    pub(crate) construction: ConstructionDescription,
    pub(crate) entries: Vec<SnapshotEntry>,
    pub(crate) material: SnapshotReference,
    pub(crate) binding: SnapshotNodeBinding,
}

impl DecisionDescription {
    pub(crate) fn validate(&self, maximum_entries: usize) -> Result<()> {
        self.construction.validate(maximum_entries)?;
        if self.entries.len() > maximum_entries.min(MAX_ENTRIES) {
            return Err(DescriptionError::Budget);
        }
        if self.material.repository != self.construction.request.source.handle.repository_id
            || self.material.node != self.construction.node
            || self.binding.reference() != self.material
            || !self.binding.same_repository(self.construction.request.source.binding)
        {
            return Err(DescriptionError::Source);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum UnchangedFormat {
    DirectoryDraftUnchanged,
}

/// The original immutable node is retained. There is no new node allocation
/// in this mirror; full replay still proves that every selected row may clear.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct UnchangedDescription {
    pub(crate) format: UnchangedFormat,
    pub(crate) request: RequestDescription,
    pub(crate) files: Vec<SelectedOutcome>,
    pub(crate) entries: Vec<SnapshotEntry>,
    pub(crate) material: SnapshotReference,
    pub(crate) binding: SnapshotNodeBinding,
}

impl UnchangedDescription {
    pub(crate) fn validate(&self, maximum_entries: usize) -> Result<()> {
        self.request.validate(maximum_entries)?;
        validate_files(&self.request, &self.files)?;
        if self.entries.len() > maximum_entries.min(MAX_ENTRIES) {
            return Err(DescriptionError::Budget);
        }
        if self.binding != self.request.source.binding
            || self.material != self.binding.reference()
            || self.files.iter().any(|file| !matches!(file.outcome, FileOutcome::Retained { .. }))
        {
            return Err(DescriptionError::Outcome);
        }
        Ok(())
    }
}

/// Each strict inner record has a disjoint mandatory format value and rejects
/// surplus fields. The changed variant preserves its original canonical bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(untagged)]
pub(crate) enum DirectoryResult {
    Changed(DecisionDescription),
    Unchanged(UnchangedDescription),
}

impl DirectoryResult {
    pub(crate) fn validate(&self, maximum_entries: usize) -> Result<()> {
        match self {
            Self::Changed(value) => value.validate(maximum_entries),
            Self::Unchanged(value) => value.validate(maximum_entries),
        }
    }

    pub(crate) fn request(&self) -> &RequestDescription {
        match self {
            Self::Changed(value) => &value.construction.request,
            Self::Unchanged(value) => &value.request,
        }
    }

    pub(crate) fn files(&self) -> &[SelectedOutcome] {
        match self {
            Self::Changed(value) => &value.construction.files,
            Self::Unchanged(value) => &value.files,
        }
    }

    pub(crate) fn entries(&self) -> &[SnapshotEntry] {
        match self {
            Self::Changed(value) => &value.entries,
            Self::Unchanged(value) => &value.entries,
        }
    }

    pub(crate) fn material(&self) -> SnapshotReference {
        match self {
            Self::Changed(value) => value.material,
            Self::Unchanged(value) => value.material,
        }
    }

    pub(crate) fn binding(&self) -> SnapshotNodeBinding {
        match self {
            Self::Changed(value) => value.binding,
            Self::Unchanged(value) => value.binding,
        }
    }

    pub(crate) fn is_unchanged(&self) -> bool { matches!(self, Self::Unchanged(_)) }
}

pub(crate) fn encode_request(
    request: &RequestDescription, maximum_bytes: usize, maximum_entries: usize,
) -> Result<Vec<u8>> {
    request.validate(maximum_entries)?;
    if request.message.len() > maximum_bytes {
        return Err(DescriptionError::Budget);
    }
    let bytes = storage_contract::canonical_json(request).map_err(DescriptionError::Encode)?;
    if bytes.len() > maximum_bytes {
        return Err(DescriptionError::Budget);
    }
    Ok(bytes)
}

pub(crate) fn decode_request(
    bytes: &[u8], maximum_bytes: usize, maximum_entries: usize,
) -> Result<RequestDescription> {
    if bytes.len() > maximum_bytes {
        return Err(DescriptionError::Budget);
    }
    let request: RequestDescription = storage_contract::decode_strict(bytes)
        .map_err(DescriptionError::Decode)?;
    let canonical = encode_request(&request, maximum_bytes, maximum_entries)?;
    if canonical != bytes {
        return Err(DescriptionError::Canonical);
    }
    Ok(request)
}

pub(crate) fn decode_decision(
    bytes: &[u8], maximum_bytes: usize, maximum_entries: usize,
) -> Result<DecisionDescription> {
    if bytes.len() > maximum_bytes {
        return Err(DescriptionError::Budget);
    }
    let decision: DecisionDescription = storage_contract::decode_strict(bytes)
        .map_err(DescriptionError::Decode)?;
    decision.validate(maximum_entries)?;
    let canonical = storage_contract::canonical_json(&decision)
        .map_err(DescriptionError::Encode)?;
    if canonical != bytes {
        return Err(DescriptionError::Canonical);
    }
    Ok(decision)
}
