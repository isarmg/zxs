//! Canonical directory reconstruction from real pinned source + descriptions.
//! No described FILE outcome becomes a batch, working clear or native receipt.
use super::scope::DirectoryOccurrence;
use super::{DirectoryDraftError, PendingDirectoryDrafts};
use crate::engine::PlanningContext;
use crate::persist::directory_draft_commit::{
    ConstructionDescription, DecisionDescription, DescriptionError, ExpectedContent,
    FileOutcome, RequestDescription, RequestFormat, SelectionDescription, SourceDescription,
};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotFileSource, SnapshotManifestError,
};
use storage_contract::snapshot_worktree::SnapshotHost;
use storage_contract::{FileId, Generation};

#[derive(Debug, error_definition::Error)]
pub(crate) enum ReconstructionError {
    #[error("retaining the actual directory/working source")]
    Gateway(#[source] DirectoryDraftError),
    #[error("reading the registered root of the actual planning context")]
    Root(#[source] crate::Error),
    #[error("the described request differs from this exact source occurrence")]
    Request,
    #[error("the selected base lacks its admitted immutable material: {0}")]
    Base(FileId),
    #[error("a described FILE result differs from its selected source: {0}")]
    File(FileId),
    #[error("the proposed directory material does not match its canonical reconstruction")]
    Material,
    #[error("checking the bounded strict directory description")]
    Description(#[source] DescriptionError),
    #[error("constructing the canonical complete directory manifest")]
    Manifest(#[source] SnapshotManifestError),
}

type Result<T> = std::result::Result<T, ReconstructionError>;

/// Bound only by actual PendingDirectoryDrafts; the serializable request below
/// is a descriptive mirror and has no constructor back to this borrowed source.
pub(crate) struct BoundDirectoryRequest<'a> {
    pending: &'a PendingDirectoryDrafts,
    request: RequestDescription,
    retained: Vec<Option<SnapshotFile>>,
}

/// Syntax and source routing only. In particular there is no into-node,
/// working-after application, FILE capability or OperationResult conversion.
pub(crate) struct ProposedDirectoryProjection {
    manifest: AdmittedSnapshotManifest,
}

impl ProposedDirectoryProjection {
    pub(super) fn into_manifest(self) -> AdmittedSnapshotManifest {
        self.manifest
    }

    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest {
        &self.manifest
    }
}

impl<'a> BoundDirectoryRequest<'a> {
    pub(crate) fn capture(
        pending: &'a PendingDirectoryDrafts, context: &PlanningContext<'_>,
    ) -> Result<Self> {
        pending.require_context(context).map_err(ReconstructionError::Gateway)?;
        let directory = pending.directory();
        let host = directory.host();
        let root = context.namespace_host_root(
            host.namespace.repository_id, host.namespace.branch_id,
        ).map_err(ReconstructionError::Root)?;
        let branch = context.state.branches.get(&host.namespace.branch_id.to_string())
            .ok_or(ReconstructionError::Request)?;
        let binding = root.binding();
        let request = RequestDescription {
            format: RequestFormat::DirectoryDraftRequest,
            operation: directory.operation(),
            principal: directory.principal().clone(),
            source: SourceDescription {
                handle: directory.source(),
                binding: pending.original().binding(),
                host: host.clone(),
                root: SnapshotHost {
                    root_id: binding.root_id,
                    directory_id: binding.roots.common_data.directory_id,
                    expected_root_generation: binding.generation,
                },
                namespace_generation: Generation::new(branch.generation),
            },
            selected: directory.selections().iter()
                .map(SelectionDescription::from_selection).collect(),
            message: directory.message().into(),
        };
        request.validate(context.config.max_entries).map_err(ReconstructionError::Description)?;
        let mut retained = Vec::with_capacity(request.selected.len());
        for selection in &request.selected {
            let material = match selection.content {
                ExpectedContent::Born { base, .. } => {
                    let version = context.state.versions.get(&base.version_id().to_string())
                        .filter(|version| version.file_id == selection.target.file.to_string())
                        .ok_or(ReconstructionError::Base(selection.target.file))?;
                    Some(SnapshotFile {
                        file_id: selection.target.file,
                        version_id: base.version_id(),
                        size: version.logical_size,
                        content_hash: version.content_hash,
                    })
                }
                ExpectedContent::UnbornEmpty {} | ExpectedContent::UnbornSaved { .. } => None,
            };
            retained.push(material);
        }
        pending.require_context(context).map_err(ReconstructionError::Gateway)?;
        Ok(Self { pending, request, retained })
    }

    pub(crate) fn request(&self) -> &RequestDescription {
        &self.request
    }

    pub(crate) fn require_request(
        &self, context: &PlanningContext<'_>, request: &RequestDescription,
    ) -> Result<()> {
        self.pending.require_context(context).map_err(ReconstructionError::Gateway)?;
        request.validate(context.config.max_entries).map_err(ReconstructionError::Description)?;
        if request != &self.request {
            return Err(ReconstructionError::Request);
        }
        Ok(())
    }

    /// Rebuild the directory, never the FILE graph. New-version legitimacy and
    /// every working_after still require the shared full-State batch product.
    pub(crate) fn reconstruct_proposed(
        &self, context: &PlanningContext<'_>, description: &ConstructionDescription,
    ) -> Result<ProposedDirectoryProjection> {
        self.require_request(context, &description.request)?;
        description.validate(context.config.max_entries)
            .map_err(ReconstructionError::Description)?;
        for (index, result) in description.files.iter().enumerate() {
            let selected = &self.pending.selected().sources()[index];
            let material = result.outcome.material();
            if selected.target() != &result.target || selected.logical_size() != material.size {
                return Err(ReconstructionError::File(result.target.file));
            }
            match (self.request.selected[index].content, result.outcome) {
                (ExpectedContent::Born { payload, .. }, FileOutcome::Retained { .. }) => {
                    if self.retained[index] != Some(material)
                        || payload.content_hash() != material.content_hash
                    {
                        return Err(ReconstructionError::File(result.target.file));
                    }
                }
                (ExpectedContent::Born { payload, .. }, FileOutcome::Published { .. }) => {
                    let base = self.retained[index]
                        .ok_or(ReconstructionError::Base(result.target.file))?;
                    if payload.content_hash() != material.content_hash
                        || context.state.versions.contains_key(&material.version_id.to_string())
                        || (base.size == material.size
                            && base.content_hash == material.content_hash)
                    {
                        return Err(ReconstructionError::File(result.target.file));
                    }
                }
                (ExpectedContent::UnbornEmpty {} | ExpectedContent::UnbornSaved { .. },
                    FileOutcome::FirstPublished { .. }) => {
                    if context.state.versions.contains_key(&material.version_id.to_string()) {
                        return Err(ReconstructionError::File(result.target.file));
                    }
                    // A sparse manifest is not a full ContentHash. This layer
                    // cannot authenticate the proposed first full-file hash.
                    // It remains descriptive until the actual batch supplies it.
                }
                _ => return Err(ReconstructionError::File(result.target.file)),
            }
        }
        let mut entries = Vec::with_capacity(self.pending.directory().projection().len());
        for origin in self.pending.directory().projection() {
            let entry = match *origin {
                DirectoryOccurrence::Original(index) => {
                    self.pending.original().manifest().entries()[index].clone()
                }
                DirectoryOccurrence::Selected(index) => {
                    let result = &description.files[index];
                    let (branch, node) = result.outcome.source();
                    SnapshotEntry {
                        path: result.target.path.clone(),
                        content: SnapshotEntryContent::File {
                            material: result.outcome.material(),
                            source: SnapshotFileSource::Container { branch, node },
                        },
                    }
                }
            };
            entries.push(entry);
        }
        if entries.len() > context.config.max_entries {
            return Err(ReconstructionError::Description(DescriptionError::Budget));
        }
        let manifest = AdmittedSnapshotManifest::prepare(
            self.request.source.handle.repository_id, description.node,
            self.request.operation, entries,
        ).map_err(ReconstructionError::Manifest)?;
        self.pending.require_context(context).map_err(ReconstructionError::Gateway)?;
        Ok(ProposedDirectoryProjection { manifest })
    }

    /// Exact reconstructed entries/reference only. The described new physical
    /// binding is NOT opened/adopted and cannot register a node via this method.
    pub(crate) fn compare_decision_description(
        &self, context: &PlanningContext<'_>, description: &DecisionDescription,
    ) -> Result<ProposedDirectoryProjection> {
        description.validate(context.config.max_entries)
            .map_err(ReconstructionError::Description)?;
        let projection = self.reconstruct_proposed(context, &description.construction)?;
        if description.entries.as_slice() != projection.manifest.entries()
            || description.material != projection.manifest.reference()
        {
            return Err(ReconstructionError::Material);
        }
        Ok(projection)
    }
}
