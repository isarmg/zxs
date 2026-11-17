//! Read-side inputs for the existing budget-before-physical publication phase.
//! No FILE result, node writer, Candidate, or completion is issued here.
use crate::engine::PlanningContext;
use crate::snapshot_worktree::drafts::scope::DirectoryOccurrence;
use crate::snapshot_worktree::drafts::{
    DirectoryDraftError as Failure, DirectoryDraftFile, DirectoryDraftReader,
    PendingDirectoryDrafts, Result,
};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, MAX_LOGICAL_BYTES, PinnedSnapshotRepository,
    SnapshotEntryContent, SnapshotFile, SnapshotFileSource,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_contract::VersionId;

/// The same published repository parent and complete pinned BODY view. The
/// byte count is a logical input to publication budgeting, not a reservation.
/// In particular this product has no mkdir, persist, clear, or binding getter.
pub(crate) struct DirectoryNodeInput {
    source: Arc<PendingDirectoryDrafts>,
    repository: PinnedSnapshotRepository,
    logical_bytes: u64,
}

impl PendingDirectoryDrafts {
    pub(crate) fn node_input(
        self: &Arc<Self>,
        context: &PlanningContext<'_>,
    ) -> Result<DirectoryNodeInput> {
        self.require_context(context)?;
        if self.directory().projection().len() > context.config.max_entries {
            return Err(Failure::NodeBudget);
        }
        let logical_bytes = self.files().try_fold(0u64, |sum, file| {
            sum.checked_add(file.logical_size())
                .filter(|bytes| *bytes <= MAX_LOGICAL_BYTES)
                .ok_or(Failure::NodeBudget)
        })?;
        let host = self.directory().host();
        let root = context.namespace_host_root(
            host.namespace.repository_id, host.namespace.branch_id,
        ).map_err(Failure::Engine)?;
        let common = root.common().map_err(Failure::Engine)?;
        let repository = self.original().pin_repository(common).map_err(Failure::OriginalRead)?;
        self.require_context(context)?;
        Ok(DirectoryNodeInput { source: Arc::clone(self), repository, logical_bytes })
    }
}

impl DirectoryNodeInput {
    pub(crate) fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.source.require_context(context)?;
        let host = self.source.directory().host();
        let root = context.namespace_host_root(
            host.namespace.repository_id, host.namespace.branch_id,
        ).map_err(Failure::Engine)?;
        self.repository.require_source(
            self.source.original(), root.common().map_err(Failure::Engine)?,
        ).map_err(Failure::OriginalRead)
    }

    /// This binds only the proposed physical inventory to its read sources.
    /// The native batch must independently prove each Retained/Published FILE
    /// branch, node and version before using it in any Candidate. Syntax and
    /// readable bytes never stand in for that whole-State proof.
    pub(crate) fn bind_proposed_manifest(
        &self,
        manifest: &AdmittedSnapshotManifest,
    ) -> Result<ProposedDirectoryBodies<'_>> {
        let directory = self.source.directory();
        if manifest.repository_id() != directory.source().repository_id
            || manifest.node_id() == directory.source().node_id
            || manifest.operation_id() != directory.operation()
            || manifest.entries().len() != directory.projection().len()
        {
            return Err(Failure::NodeProjection);
        }
        for (entry, origin) in manifest.entries().iter().zip(directory.projection()) {
            match *origin {
                DirectoryOccurrence::Original(index) => {
                    if entry != &self.source.original().manifest().entries()[index] {
                        return Err(Failure::NodeProjection);
                    }
                }
                DirectoryOccurrence::Selected(index) => {
                    let selected = &self.source.selected().sources()[index];
                    let SnapshotEntryContent::File {
                        material, source: SnapshotFileSource::Container { .. },
                    } = entry.content else {
                        return Err(Failure::NodeProjection);
                    };
                    if entry.path != selected.target().path
                        || material.file_id != selected.target().file
                        || material.size != selected.logical_size()
                    {
                        return Err(Failure::NodeProjection);
                    }
                }
            }
        }
        let bodies: Vec<_> = self.source.files().collect();
        let mut by_version = BTreeMap::new();
        for (version, payload) in manifest.payloads() {
            let index = bodies.binary_search_by(|body| {
                body.path().key().cmp(payload.source_path.key())
            }).map_err(|_| Failure::NodeProjection)?;
            by_version.insert(*version, index);
        }
        Ok(ProposedDirectoryBodies {
            input: self, files: manifest.files().clone(), bodies, by_version,
        })
    }
}

/// Exact BODY routing for one proposed manifest. It deliberately exposes no
/// SnapshotNodeBinding or publication API. The existing destination copy must
/// still verify the full manifest size/hash; the separate FILE batch determines
/// whether those proposed historical identities may ever be published.
pub(crate) struct ProposedDirectoryBodies<'a> {
    input: &'a DirectoryNodeInput,
    files: BTreeMap<VersionId, SnapshotFile>,
    bodies: Vec<DirectoryDraftFile<'a>>,
    by_version: BTreeMap<VersionId, usize>,
}

impl ProposedDirectoryBodies<'_> {
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.input.require_context(context)
    }

    pub(crate) fn open_payload(&self, file: &SnapshotFile) -> Result<DirectoryDraftReader<'_>> {
        if self.files.get(&file.version_id) != Some(file) {
            return Err(Failure::NodeProjection);
        }
        let index = self.by_version.get(&file.version_id).ok_or(Failure::NodeProjection)?;
        self.bodies[*index].open()
    }
}

impl DirectoryNodeInput {
    pub(in crate::snapshot_worktree::drafts) fn source(&self) -> &Arc<PendingDirectoryDrafts> {
        &self.source
    }

    pub(in crate::snapshot_worktree::drafts) fn require_material(&self) -> Result<()> {
        self.repository.require_held_source(self.source.original())
            .map_err(Failure::OriginalRead)
    }

    pub(in crate::snapshot_worktree::drafts) fn prepare_node(
        &self,
        manifest: AdmittedSnapshotManifest,
        permit: &crate::engine::BudgetedDirectoryPreparation,
    ) -> std::result::Result<crate::working_tree::snapshot::HeldSnapshotNode,
        crate::working_tree::snapshot::SnapshotPreparationError>
    {
        let bodies = self.bind_proposed_manifest(&manifest).map_err(|source| {
            crate::working_tree::snapshot::SnapshotPreparationError::Rejected(
                crate::working_tree::snapshot::SnapshotStoreError::WorkingSource(Box::new(source)),
            )
        })?;
        self.repository.prepare_directory(manifest, permit, |file| {
            bodies.open_payload(file).map_err(|source| {
                crate::working_tree::snapshot::SnapshotStoreError::WorkingSource(Box::new(source))
            })
        })
    }

    pub(in crate::snapshot_worktree::drafts) fn require_prepared(
        &self,
        node: &crate::working_tree::snapshot::HeldSnapshotNode,
    ) -> Result<()> {
        self.require_material()?;
        self.repository.require_prepared(node).map_err(Failure::OriginalRead)
    }
}
