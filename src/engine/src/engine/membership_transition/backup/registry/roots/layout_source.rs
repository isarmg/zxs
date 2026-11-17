//! Actual original-member or original common-decision source. The preparation
//! wrapper exists only for the live fixed-member fence; detached observation
//! cannot construct it or install target entities.
use super::*;
use crate::engine::membership_transition::AdmittedBackupDecision;
use crate::history_package::AdmittedState;
use crate::{Config, State, ValidatedConfig, object_hash};

enum Origin<'a> {
    Preparing(&'a BackupChoiceContext),
    Decided(&'a AdmittedBackupDecision),
    Retained(&'a super::layouts::PreparedBackupLayouts),
}

enum Ownership<'a> {
    Prepared(PreparedLayoutSource<'a>),
    Published(&'a PublishedBackupPayload<'a>),
}

struct PreparedLayoutSource<'a> {
    origin: Origin<'a>,
    roots: &'a PreparedBackupRoots,
    evidence: &'a BackupSourceFenceEvidence,
    fence: &'a str,
    targets: &'a [RetainedBackupTarget],
    index: usize,
    config: Config,
}

pub(crate) struct BackupLayoutSource<'a> {
    ownership: Ownership<'a>,
    state: &'a State,
    disk: DiskId,
    operation: OperationId,
}

pub(crate) struct BackupLayoutPreparation<'a>(BackupLayoutSource<'a>);
impl BackupLayoutPreparation<'_> {
    pub(crate) fn source(&self) -> &BackupLayoutSource<'_> {
        &self.0
    }
}

impl PreparedBackupRoots {
    pub(in crate::engine) fn with_layout_preparation<T>(
        &self, choices: &BackupChoiceContext, targets: &[RetainedBackupTarget],
        index: usize, original: &AdmittedState, stop: &AtomicBool,
        action: impl FnOnce(&BackupLayoutPreparation<'_>) -> Result<T>,
    ) -> Result<T> {
        let source = self.layout_source(Origin::Preparing(choices), choices.root_evidence()?,
            &choices.transition_hash, choices.intent.operation_id, targets, index, original, stop)?;
        let source = BackupLayoutPreparation(source);
        let result = action(&source)?;
        source.0.require(stop)?;
        Ok(result)
    }

    pub(in crate::engine) fn with_layout_source<T>(
        &self, choices: &BackupChoiceContext, targets: &[RetainedBackupTarget],
        index: usize, original: &AdmittedState, stop: &AtomicBool,
        action: impl FnOnce(&BackupLayoutSource<'_>) -> Result<T>,
    ) -> Result<T> {
        let source = self.layout_source(Origin::Preparing(choices), choices.root_evidence()?,
            &choices.transition_hash, choices.intent.operation_id, targets, index, original, stop)?;
        let result = action(&source)?;
        source.require(stop)?;
        Ok(result)
    }

    pub(in crate::engine) fn with_decided_layout_source<T>(
        &self, decision: &AdmittedBackupDecision, targets: &[RetainedBackupTarget],
        index: usize, original: &AdmittedState, stop: &AtomicBool,
        action: impl FnOnce(&BackupLayoutSource<'_>) -> Result<T>,
    ) -> Result<T> {
        let transition = decision.transition();
        let crate::engine::membership_transition::Origin::BackupCutover { evidence, .. }
            = &transition.origin else {
                return Err(Failure::BackupLayoutEvidence.into());
            };
        let fence = object_hash(crate::engine::membership_transition::INTENT_KIND, transition)?;
        let source = self.layout_source(Origin::Decided(decision), evidence, &fence,
            transition.operation_id, targets, index, original, stop)?;
        let result = action(&source)?;
        source.require(stop)?;
        Ok(result)
    }

    pub(in crate::engine) fn with_retained_layout_source<T>(
        &self, product: &super::layouts::PreparedBackupLayouts,
        evidence: &BackupSourceFenceEvidence, fence: &str, targets: &[RetainedBackupTarget],
        index: usize, original: &AdmittedState, stop: &AtomicBool,
        action: impl FnOnce(&BackupLayoutSource<'_>) -> Result<T>,
    ) -> Result<T> {
        product.require_source(fence, self.preparation.operation_id)?;
        let source = self.layout_source(Origin::Retained(product), evidence, fence,
            product.operation(), targets, index, original, stop)?;
        let result = action(&source)?;
        source.require(stop)?;
        Ok(result)
    }

    fn layout_source<'a>(
        &'a self, origin: Origin<'a>, evidence: &'a BackupSourceFenceEvidence,
        fence: &'a str, operation: OperationId, targets: &'a [RetainedBackupTarget],
        index: usize, original: &'a AdmittedState, stop: &AtomicBool,
    ) -> Result<BackupLayoutSource<'a>> {
        if object_hash("backup-prepared-state", original.state())? != evidence.state_hash() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        self.verify(evidence, fence, targets, stop)?;
        let target = targets.get(index).ok_or(Failure::BackupRootEvidence)?;
        let disk = evidence.require_root_target(index, target.layout_root()?)?;
        let mut config = evidence.target_layout_configuration();
        for (id, binding) in &self.preparation.roots {
            if config.storage_roots.insert(*id, binding.clone()).is_some() {
                return Err(Failure::BackupRootEvidence.into());
            }
        }
        fresh_layout::validate_roots(&config)?;
        let source = BackupLayoutSource {
            ownership: Ownership::Prepared(PreparedLayoutSource {
                origin, roots: self, evidence, fence, targets, index, config,
            }),
            state: original.state(), disk, operation,
        };
        source.require(stop)?;
        Ok(source)
    }
}

impl BackupLayoutSource<'_> {
    pub(crate) fn state(&self) -> &State {
        self.state
    }
    pub(crate) fn config(&self) -> &Config {
        match &self.ownership {
            Ownership::Prepared(source) => &source.config,
            Ownership::Published(source) => source.config(),
        }
    }
    pub(crate) fn disk(&self) -> DiskId {
        self.disk
    }
    pub(crate) fn operation(&self) -> OperationId {
        self.operation
    }
    pub(crate) fn binding(&self) -> Result<&RootBinding> {
        match &self.ownership {
            Ownership::Prepared(source) => source.roots.binding(self.disk),
            Ownership::Published(source) => source.config().storage_roots.get(&self.disk)
                .ok_or_else(|| Failure::BackupRootEvidence.into()),
        }
    }

    pub(crate) fn root(&self) -> Result<&DiskRoot> {
        let root = match &self.ownership {
            Ownership::Prepared(source) => {
                let root = source.targets[source.index].layout_root()?;
                if source.evidence.require_root_target(source.index, root)? != self.disk {
                    return Err(Failure::BackupRootEvidence.into());
                }
                root
            }
            Ownership::Published(source) => source.root(),
        };
        HeldRootAreas::observe(root, &self.binding()?.roots)?.verify(root)?;
        Ok(root)
    }

    pub(crate) fn require(&self, stop: &AtomicBool) -> Result<()> {
        preparation_checkpoint(stop)?;
        match &self.ownership {
            Ownership::Prepared(source) => {
                match &source.origin {
                    Origin::Preparing(choices) => choices.recheck_root_sources(stop)?,
                    Origin::Retained(product) => {
                        product.require_source(source.fence, self.operation)?;
                    }
                    Origin::Decided(decision) => {
                        if decision.operation() != self.operation {
                            return Err(Failure::BackupLayoutEvidence.into());
                        }
                    }
                }
                source.roots.verify(source.evidence, source.fence, source.targets, stop)?;
            }
            Ownership::Published(source) => source.require(stop)?,
        }
        self.root()?;
        Ok(())
    }
}

/// Only a real original-member Publication and its full selected first State
/// can expose a serving preparation. The target still needs its separate held
/// writer admission; this product never supplies a DiskHandle or Engine Current.
pub(crate) struct PublishedBackupLayout<'a> {
    source: BackupLayoutSource<'a>,
    layout: &'a crate::container_storage::BackupTargetLayout,
    published: &'a AdmittedState,
    candidate: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
    publication: &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
}

impl PreparedBackupRoots {
    pub(in crate::engine) fn with_published_layout<T>(
        &self, decision: &AdmittedBackupDecision, layouts: &super::layouts::PreparedBackupLayouts,
        targets: &[RetainedBackupTarget], index: usize, original: &AdmittedState,
        published: &AdmittedState,
        candidate: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
        publication: &crate::engine::membership_transition::PreparedBackupFirstPublication,
        stop: &AtomicBool, action: impl FnOnce(&PublishedBackupLayout<'_>) -> Result<T>,
    ) -> Result<T> {
        let transition = decision.transition();
        let crate::engine::membership_transition::Origin::BackupCutover { evidence, .. }
            = &transition.origin else {
                return Err(Failure::BackupLayoutEvidence.into());
            };
        let fence = object_hash(crate::engine::membership_transition::INTENT_KIND, transition)?;
        layouts.require_source(&fence, transition.operation_id)?;
        let source = self.layout_source(Origin::Decided(decision), evidence, &fence,
            transition.operation_id, targets, index, original, stop)?;
        if candidate.decision_hash() != &decision.hash()
            || candidate.candidate().operation_id != source.operation()
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let layout = layouts.layout(source.disk())?;
        publication.require_namespace_candidate(candidate)?;
        candidate.require_state(published)?;
        layout.require_published(source.binding()?, source.state(), published.state(),
            source.operation())?;
        let value = PublishedBackupLayout { source, layout, published, candidate, publication };
        value.require(stop)?;
        let result = action(&value)?;
        value.require(stop)?;
        Ok(result)
    }
}

impl PublishedBackupLayout<'_> {
    pub(crate) fn source(&self) -> &BackupLayoutSource<'_> {
        &self.source
    }
    pub(crate) fn layout(&self) -> &crate::container_storage::BackupTargetLayout {
        self.layout
    }
    pub(crate) fn published(&self) -> &AdmittedState {
        self.published
    }
    pub(crate) fn candidate_hash(&self) -> &str {
        self.candidate.hash()
    }
    pub(crate) fn publication_hash(&self) -> &str {
        &self.publication.report().publication_hash
    }
    pub(crate) fn require(&self, stop: &AtomicBool) -> Result<()> {
        // All logical inputs are immutable borrowed products. Recheck only
        // the externally replaceable held roots and active source ownership.
        self.source.require(stop)
    }
}

/// Original occurrences remain bound to the frozen Config, even while the real
/// common switch exchanges old/new global seals. This source is independent of
/// target feedback and never observes a target in place of an original FILE.
pub(crate) struct PublishedBackupOriginal<'a> {
    root: &'a DiskRoot,
    disk: &'a crate::DiskConfig,
    first: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
    _publication: &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
    original: &'a AdmittedState,
    originals: &'a crate::engine::backup_preparation::OriginalFileEpochs,
    published: &'a AdmittedState,
}
impl<'a> PublishedBackupOriginal<'a> {
    pub(in crate::engine) fn observe(
        root: &'a DiskRoot, identity: &crate::DiskRegistrationIdentity,
        first: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
        publication: &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
        original: &'a AdmittedState, published: &'a AdmittedState,
        originals: &'a crate::engine::backup_preparation::OriginalFileEpochs, stop: &AtomicBool,
    ) -> Result<Self> {
        publication.require_namespace_candidate(first)?;
        first.require_original_state(original)?;
        originals.require_source(first, original)?;
        first.require_state(published)?;
        let config = first.original_config();
        let disk = config.disks.iter().find(|disk| disk.disk_id == identity.disk_id)
            .ok_or(Failure::BackupLayoutEvidence)?;
        if fresh_layout::registration(config, disk) != *identity || disk.root != root.path() {
            return Err(Failure::BackupRootEvidence.into());
        }
        let value = Self { root, disk, first, _publication: publication, original, published, originals };
        value.require(stop)?;
        Ok(value)
    }

    pub(crate) fn root(&self) -> &DiskRoot {
        self.root
    }
    pub(crate) fn config(&self) -> &ValidatedConfig {
        self.first.original_config()
    }
    pub(crate) fn disk(&self) -> &crate::DiskConfig {
        self.disk
    }
    pub(crate) fn original(&self) -> &AdmittedState {
        self.original
    }
    /// The old on-disk FILE must come from its exact scoped original writer,
    /// not just an equal current projection or an unrelated whole-pool epoch.
    pub(crate) fn require_file_predecessor(
        &self, file: storage_contract::FileId, stored_sequence: u64, records_hash: &str,
    ) -> Result<()> {
        self.originals.require_file(
            self.original, self.disk.disk_id.parse()?, file, stored_sequence, records_hash,
        )
    }

    pub(crate) fn published(&self) -> &AdmittedState {
        self.published
    }
    pub(crate) fn require(&self, stop: &AtomicBool) -> Result<()> {
        preparation_checkpoint(stop)?;
        fresh_layout::verify_root(self.config(), self.disk, self.root)
    }
}

/// Read-only first-slot material on either an original or selected target. Its
/// scope is the actual full first Publication and that exact effective Config,
/// never an Engine Current or a standalone prepared JSON record.
pub(crate) struct PublishedBackupPayload<'a> {
    root: &'a DiskRoot,
    disk: &'a crate::DiskConfig,
    config: &'a ValidatedConfig,
    first: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
    publication: &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
    published: &'a AdmittedState,
}
impl<'a> PublishedBackupPayload<'a> {
    pub(in crate::engine) fn observe(
        root: &'a DiskRoot, disk: &'a crate::DiskConfig, config: &'a ValidatedConfig,
        first: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
        publication: &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
        published: &'a AdmittedState, stop: &AtomicBool,
    ) -> Result<Self> {
        publication.require_namespace_candidate(first)?;
        first.require_state(published)?;
        first.require_published_configuration(config)?;
        let configured = config.disks.iter().find(|known| known.disk_id == disk.disk_id)
            .ok_or(Failure::BackupRootEvidence)?;
        if crate::canonical_json(configured)? != crate::canonical_json(disk)?
            || disk.root != root.path()
        {
            return Err(Failure::BackupRootEvidence.into());
        }
        let value = Self { root, disk, config, first, publication, published };
        value.require(stop)?;
        Ok(value)
    }
    pub(crate) fn root(&self) -> &DiskRoot {
        self.root
    }
    pub(crate) fn disk(&self) -> &crate::DiskConfig {
        self.disk
    }
    pub(crate) fn config(&self) -> &ValidatedConfig {
        self.config
    }
    pub(crate) fn state(&self) -> &State {
        self.published.state()
    }
    /// An unfinished common switch already has a full first Publication, but
    /// cannot reconstruct live preparation locks from disk paths. Consume that
    /// actual native source and its original State in the SAME serving verifier.
    /// Only verified completion bytes escape; no install-capable layout/image
    /// or ordinary Current source is issued by this read-only entry point.
    pub(in crate::engine) fn verify_initial_serving(
        &self, original: &AdmittedState, recipe: &str, stop: &AtomicBool,
    ) -> Result<Vec<u8>> {
        self.first.require_original_state(original)?;
        let disk = self.disk.disk_id.parse::<DiskId>()?;
        let layout = self.first.target_layout(disk)?;
        let source = BackupLayoutSource {
            ownership: Ownership::Published(self), state: original.state(), disk,
            operation: self.first.result().operation_id,
        };
        source.require(stop)?;
        layout.require_published(source.binding()?, source.state(), self.state(),
            source.operation())?;
        let value = PublishedBackupLayout {
            source, layout, published: self.published, candidate: self.first,
            publication: self.publication,
        };
        let image = crate::container_storage::BackupServingImage::capture(&value, stop)?;
        if image.hash() != recipe {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let bytes = image.verify(self.root, stop)?;
        value.require(stop)?;
        Ok(bytes)
    }

    pub(in crate::engine) fn require_publication(
        &self, first: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
        publication: &crate::engine::membership_transition::PreparedBackupFirstPublication,
    ) -> Result<()> {
        if !std::ptr::eq(first, self.first) || !std::ptr::eq(publication, self.publication) {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        Ok(())
    }
    pub(crate) fn require(&self, stop: &AtomicBool) -> Result<()> {
        preparation_checkpoint(stop)?;
        fresh_layout::verify_root(self.config, self.disk, self.root)
    }
}
