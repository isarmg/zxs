//! Strict combined decision encoding; only complete original replay admits it.
//! The upper encoding is not an OperationResult or an admitted node binding.
use super::{
    ConstructionDescription, DecisionDescription, DecisionFormat, DirectoryResult,
    FileOutcome, SelectedOutcome, UnchangedDescription, UnchangedFormat,
};
use crate::business::file_draft::{PublishedContent, batch, origin::{NodeOrigin, Reason}};
use crate::engine::file_draft::WorkingBatchHandoff;
use crate::persist::file_draft::{Protection, content_protection};
use crate::snapshot_worktree::drafts::DirectoryPhysicalInput;
use crate::working_tree::snapshot::{SnapshotEntry, SnapshotNodeBinding, SnapshotReference};
use crate::{Engine, OperationResult, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::FileWorkingState;
use storage_contract::{CommitState, CurrentHealth, ProtectionAtPublish, Sequence};

pub(crate) use storage_contract::snapshot_worktree::draft_commit::KIND;

#[derive(Debug, error_definition::Error)]
pub(crate) enum NativeError {
    #[error("checking the complete directory request and results")]
    Description(#[source] super::DescriptionError),
    #[error("encoding the combined directory decision")]
    Encoding(#[source] storage_contract::Error),
    #[error("reading strict native directory bytes")]
    Boundary(#[source] storage_contract::Error),
    #[error("encoding or decoding the native directory value")]
    Decode(#[source] json_codec::Error),
    #[error("deriving the original FILE protection and request digest")]
    File(#[source] crate::Error),
    #[error("the combined decision exceeds its admitted record budget")]
    Budget,
    #[error("the combined decision does not exactly cover its selected FILE set")]
    Shape,
}

type Result<T> = std::result::Result<T, NativeError>;

impl NativeError {
    pub(crate) fn code(&self) -> storage_contract::ErrorCode {
        match self {
            Self::File(source) => source.code(),
            Self::Budget => storage_contract::ErrorCode::ResourceLimit,
            Self::Description(_) | Self::Encoding(_) | Self::Boundary(_)
            | Self::Decode(_) | Self::Shape => storage_contract::ErrorCode::PublicationInvalid,
        }
    }
}

/// Original constructor facts. Only the actual live handoff issues a plan;
/// decoded descriptions use the independent full reconstruction below it.
pub(crate) struct NativePlan {
    construction: ConstructionDescription,
    working_after: Vec<FileWorkingState>,
    origins: Vec<Option<NodeOrigin>>,
    contents: Vec<Option<PublishedContent>>,
    timestamp: u64,
    epoch: u64,
    protection: Vec<Protection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Envelope {
    pub(crate) directory: DirectoryResult,
    pub(crate) working_after: Vec<FileWorkingState>,
    pub(crate) contents: Vec<Option<PublishedContent>>,
    origins: Vec<Option<NodeOrigin>>,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) membership_epoch: u64,
    version_protection: Vec<Protection>,
}

/// Charges the maximum native envelope width before node creation. No decode,
/// receipt, State projection or physical-address accessor is available.
pub(crate) struct DirectoryEnvelopeBudget {
    bytes: Vec<u8>,
}

impl DirectoryEnvelopeBudget {
    pub(crate) fn encoded_bytes(&self) -> &[u8] { &self.bytes }
    pub(crate) fn maximum_bytes(&self) -> usize { self.bytes.len() }
}

impl NativePlan {
    pub(crate) fn from_batch(
        previous: &State,
        construction: ConstructionDescription,
        batch: &WorkingBatchHandoff,
        timestamp: u64,
        epoch: u64,
        maximum_entries: usize,
    ) -> Result<Self> {
        construction.validate(maximum_entries).map_err(NativeError::Description)?;
        if construction.request.operation != batch.request().operation()
            || construction.request.principal != *batch.request().principal()
            || construction.request.message != batch.request().message()
            || construction.request.selected.len() != batch.items().len()
            || construction.request.selected.iter().zip(batch.request().selected())
                .any(|(description, actual)| description.target != actual.target
                    || description.expectation() != actual.expected)
        {
            return Err(NativeError::Shape);
        }
        let protection = protection(previous, &construction.files, &batch.trace().contents)?;
        let origins = origins(previous, &construction.files, &batch.trace().contents,
            construction.request.operation, Sequence::new(batch.state().sequence), timestamp)?;
        Ok(Self {
            construction, origins,
            working_after: batch.items().iter().map(|item| item.working_after().clone()).collect(),
            contents: batch.trace().contents.clone(),
            timestamp,
            epoch,
            protection,
        })
    }

    pub(crate) fn budget(
        &self, input: &DirectoryPhysicalInput, maximum_bytes: usize,
    ) -> Result<DirectoryEnvelopeBudget> {
        if input.is_unchanged() {
            let envelope = self.feedback(input, input.original_binding())?;
            let bytes = storage_contract::canonical_json(&envelope)
                .map_err(NativeError::Encoding)?;
            if bytes.len() > maximum_bytes { return Err(NativeError::Budget); }
            return Ok(DirectoryEnvelopeBudget { bytes });
        }
        #[derive(Serialize)]
        struct Directory<'a, B> {
            format: DecisionFormat,
            construction: &'a ConstructionDescription,
            entries: &'a [SnapshotEntry],
            material: SnapshotReference,
            binding: B,
        }
        #[derive(Serialize)]
        struct Upper<'a, B> {
            directory: Directory<'a, B>,
            working_after: &'a [FileWorkingState],
            contents: &'a [Option<PublishedContent>],
            origins: &'a [Option<NodeOrigin>],
            #[serialization(with = "storage_contract::u64_string")]
            timestamp: u64,
            #[serialization(with = "storage_contract::u64_string")]
            membership_epoch: u64,
            version_protection: &'a [Protection],
        }
        let bytes = storage_contract::canonical_json(&Upper {
            directory: Directory {
                format: DecisionFormat::DirectoryDraftDecision,
                construction: &self.construction,
                entries: input.manifest().entries(),
                material: input.reference(),
                binding: input.original_binding().budget_encoding(input.reference()),
            },
            working_after: &self.working_after,
            contents: &self.contents, origins: &self.origins,
            timestamp: self.timestamp,
            membership_epoch: self.epoch,
            version_protection: &self.protection,
        }).map_err(NativeError::Encoding)?;
        if bytes.len() > maximum_bytes { return Err(NativeError::Budget); }
        Ok(DirectoryEnvelopeBudget { bytes })
    }

    pub(crate) fn feedback(
        &self, input: &DirectoryPhysicalInput, binding: SnapshotNodeBinding,
    ) -> Result<Envelope> {
        if binding.reference() != input.reference()
            || !binding.same_repository(input.original_binding())
        {
            return Err(NativeError::Shape);
        }
        let directory = if input.is_unchanged() {
            if binding != input.original_binding()
                || self.contents.iter().any(Option::is_some)
                || self.construction.files.iter()
                    .any(|item| !matches!(item.outcome, FileOutcome::Retained { .. }))
            {
                return Err(NativeError::Shape);
            }
            DirectoryResult::Unchanged(UnchangedDescription {
                format: UnchangedFormat::DirectoryDraftUnchanged,
                request: self.construction.request.clone(),
                files: self.construction.files.clone(),
                entries: input.manifest().entries().to_vec(),
                material: input.reference(), binding,
            })
        } else {
            DirectoryResult::Changed(DecisionDescription {
                format: DecisionFormat::DirectoryDraftDecision,
                construction: self.construction.clone(),
                entries: input.manifest().entries().to_vec(),
                material: input.reference(),
                binding,
            })
        };
        Ok(Envelope {
            directory,
            working_after: self.working_after.clone(),
            contents: self.contents.clone(), origins: self.origins.clone(),
            timestamp: self.timestamp,
            membership_epoch: self.epoch,
            version_protection: self.protection.clone(),
        })
    }
}

fn origins(previous: &State, files: &[SelectedOutcome], contents: &[Option<PublishedContent>],
    operation: storage_contract::OperationId, sequence: Sequence, timestamp: u64,
) -> Result<Vec<Option<NodeOrigin>>> {
    if files.len() != contents.len() { return Err(NativeError::Shape); }
    files.iter().zip(contents).map(|(item, content)| content.as_ref().map(|body|
        NodeOrigin::from_content(previous, &item.target, body, Reason::ManualDirectory {},
            operation, sequence, storage_contract::Timestamp::from_unix_seconds(timestamp))
            .map_err(NativeError::File)
    ).transpose()).collect()
}

fn protection(
    previous: &State, files: &[SelectedOutcome],
    contents: &[Option<PublishedContent>],
) -> Result<Vec<Protection>> {
    if contents.len() != files.len() { return Err(NativeError::Shape); }
    files.iter().zip(contents).filter_map(|(item, content)| {
        content.as_ref().map(|content| {
            content_protection(previous, &item.target, content).map_err(NativeError::File)
        })
    }).collect()
}

impl Envelope {
    pub(crate) fn batch_request(&self) -> batch::WorkingBatchRequest {
        let request = self.directory.request();
        batch::WorkingBatchRequest::new(
            request.operation, request.principal.clone(), request.message.clone(),
            request.selected.iter().map(|selected| storage_contract::file_draft::WorkingSelection {
                target: selected.target.clone(), expected: selected.expectation(),
            }).collect(),
        )
    }

    pub(crate) fn trace(&self) -> batch::WorkingBatchTrace {
        batch::WorkingBatchTrace { contents: self.contents.clone() }
    }

    pub(crate) fn receipt(
        &self, previous: &State, sequence: Sequence,
        maximum_bytes: usize, maximum_entries: usize,
    ) -> Result<OperationResult> {
        self.directory.validate(maximum_entries).map_err(NativeError::Description)?;
        if self.working_after.len() != self.directory.files().len()
            || self.origins != origins(previous, self.directory.files(), &self.contents,
                self.directory.request().operation, sequence, self.timestamp)?
            || self.version_protection
                != protection(previous, self.directory.files(), &self.contents)?
        {
            return Err(NativeError::Shape);
        }
        self.record(sequence, maximum_bytes, maximum_entries)
    }

    fn record(
        &self, sequence: Sequence, maximum_bytes: usize, maximum_entries: usize,
    ) -> Result<OperationResult> {
        self.directory.validate(maximum_entries).map_err(NativeError::Description)?;
        if self.working_after.len() != self.directory.files().len()
            || self.contents.len() != self.directory.files().len()
            || self.origins.len() != self.contents.len()
            || self.origins.iter().zip(&self.contents).any(|(cause, body)| match (cause, body) {
                (None, None) => false,
                (Some(cause), Some(body)) => cause.reason != (Reason::ManualDirectory {})
                    || cause.target.file.to_string() != body.version.file_id
                    || cause.node.to_string() != body.version.version_id
                    || cause.operation != self.directory.request().operation || cause.sequence != sequence
                    || cause.published_at.unix_seconds() != self.timestamp || cause.ancestors.len() > 31,
                _ => true,
            })
            || self.version_protection.len() != self.contents.iter().filter(|v| v.is_some()).count()
        {
            return Err(NativeError::Shape);
        }
        for (file, content) in self.directory.files().iter().zip(&self.contents) {
            match (file.outcome, content) {
                (FileOutcome::Retained { .. }, None) => {}
                (FileOutcome::Published { material, .. }, Some(content))
                | (FileOutcome::FirstPublished { material, .. }, Some(content)) => {
                    if content.version.file_id != material.file_id.to_string()
                        || content.version.version_id != material.version_id.to_string()
                        || content.version.logical_size != material.size
                        || content.version.content_hash != material.content_hash
                    {
                        return Err(NativeError::Shape);
                    }
                }
                _ => return Err(NativeError::Shape),
            }
        }
        let request = self.directory.request();
        let body = json_codec::to_value(request).map_err(NativeError::Decode)?;
        let digest = Engine::native_request_digest(KIND, request.principal.as_str(), &body)
            .map_err(NativeError::File)?;
        let bytes = storage_contract::canonical_json(self).map_err(NativeError::Encoding)?;
        if bytes.len() > maximum_bytes { return Err(NativeError::Budget); }
        Ok(OperationResult {
            operation_id: request.operation,
            kind: KIND.into(),
            commit_id: None, history_tree_id: None, branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: if self.contents.iter().any(Option::is_some) {
                ProtectionAtPublish::TargetMet
            } else {
                ProtectionAtPublish::NotApplicable
            },
            current_health: CurrentHealth::Healthy,
            request_digest: digest,
            sequence: sequence.get(),
            result: json_codec::to_value(self).map_err(NativeError::Decode)?,
        })
    }
}

/// Receipt indexing does not have an historical previous State. This returns
/// only a strict description, not a full-transition or physical capability.
/// The native reader below separately rederives protection from its actual pair.
pub(crate) fn decode_record(
    receipt: &OperationResult, maximum_bytes: usize, maximum_entries: usize,
) -> Result<Envelope> {
    if receipt.kind != KIND { return Err(NativeError::Shape); }
    let bytes = storage_contract::canonical_json(&receipt.result).map_err(NativeError::Encoding)?;
    if bytes.len() > maximum_bytes { return Err(NativeError::Budget); }
    let envelope: Envelope = storage_contract::decode_strict(&bytes)
        .map_err(NativeError::Boundary)?;
    let expected = envelope.record(
        Sequence::new(receipt.sequence), maximum_bytes, maximum_entries,
    )?;
    if expected != *receipt {
        return Err(NativeError::Shape);
    }
    Ok(envelope)
}

pub(crate) fn decode(
    previous: &State, receipt: &OperationResult, maximum_bytes: usize, maximum_entries: usize,
) -> Result<Envelope> {
    let envelope = decode_record(receipt, maximum_bytes, maximum_entries)?;
    if envelope.receipt(previous, Sequence::new(receipt.sequence), maximum_bytes, maximum_entries)?
        != *receipt
    {
        return Err(NativeError::Shape);
    }
    Ok(envelope)
}
