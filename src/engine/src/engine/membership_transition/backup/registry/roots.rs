//! Exact target lifecycle feedback, retained by all original fenced members.
//! A target's files alone never reissue a preparation. After the common choice,
//! its authenticated projection is the other, disjoint read-only source.
use crate::container_storage::BackupTargetLayout;
use crate::disk::DiskRoot;
use crate::disk_names::configuration::ConfiguredDiskNames;
use crate::history_package::AdmittedState;
use crate::engine::backup_preparation::{
    BackupRegistryProjection, BackupSourceFenceEvidence, RetainedBackupTarget,
};
use crate::engine::fresh_layout::{self, FreshLayoutError as Failure, root_areas::HeldRootAreas};
use crate::engine::membership_transition::backup::registry::{BackupChoiceContext, Source};
use crate::engine::membership_transition::Origin;
use crate::history_package::{install_preparation_bytes, preparation_checkpoint};
use crate::persist::layout;
use crate::working_tree::records::local_entity;
use crate::{Result, canonical_json};
use data_serialization::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use storage_contract::{DiskId, Generation, OperationId, decode_strict};
use storage_contract::working_tree::{DirectoryId, RootBinding, RootId, StorageLayout};

mod layout_source;
mod layouts;
pub(in crate::engine) use layouts::PreparedBackupLayouts;
pub(crate) use layout_source::{
    BackupLayoutPreparation,
    BackupLayoutSource,
    PublishedBackupLayout,
    PublishedBackupOriginal,
    PublishedBackupPayload,
};

const MAX_FEEDBACK: usize = 256 * 1024;
const RESERVE: u64 = 16 * 1024 * 1024;

/// Commitment to the existing canonical transition object, not a BODY digest.
/// Text is the old transition codec boundary; equality with that verified source
/// is still required after strict feedback decoding.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(transparent)]
struct SourceFenceHash(String);
impl SourceFenceHash {
    fn from_transition(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Preparation {
    source_fence: SourceFenceHash,
    operation_id: OperationId,
    roots: BTreeMap<DiskId, RootBinding>,
}

/// No public constructor, raw Config input, or conversion to disk I/O authority.
/// Rechecking opens at most five child descriptors at once; the caller retains
/// every selected target's original directory/writer ownership throughout.
pub(in crate::engine) struct PreparedBackupRoots {
    preparation: Preparation,
}
impl PreparedBackupRoots {
    pub(in crate::engine) fn binding(&self, disk: DiskId) -> Result<&RootBinding> {
        self.preparation.roots.get(&disk).ok_or_else(|| Failure::BackupRootEvidence.into())
    }

    pub(in crate::engine) fn verify(
        &self,
        evidence: &BackupSourceFenceEvidence,
        fence: &str,
        targets: &[RetainedBackupTarget],
        stop: &AtomicBool,
    ) -> Result<()> {
        if self.preparation.source_fence != SourceFenceHash::from_transition(fence) {
            return Err(Failure::BackupRootEvidence.into());
        }
        require_preparation(&self.preparation, evidence, targets, stop)
    }

    pub(super) fn observe_decided(
        projection: &BackupRegistryProjection,
        evidence: &BackupSourceFenceEvidence,
        fence: &str,
        operation: OperationId,
        targets: &[RetainedBackupTarget],
        stop: &AtomicBool,
    ) -> Result<Self> {
        let value = Self {
            preparation: Preparation {
                source_fence: SourceFenceHash::from_transition(fence),
                operation_id: operation,
                roots: projection.root_bindings()?,
            },
        };
        value.verify(evidence, fence, targets, stop)?;
        Ok(value)
    }
}

impl BackupChoiceContext {
    /// Only the live all-original-member fence may create these empty areas.
    /// Partial target creation without a complete original-member record is
    /// preserved and rejected, never inferred from matching names or contents.
    pub(in crate::engine) fn prepare_target_roots(
        &self, targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<PreparedBackupRoots> {
        let evidence = self.root_evidence()?;
        self.recheck_root_sources(stop)?;
        let saved = self.read_root_preparation(stop)?;
        let value = match saved {
            Some(value) => value,
            None => {
                self.require_root_capacity()?;
                if crate::object_hash("backup-prepared-state", original.state())?
                    != evidence.state_hash()
                {
                    return Err(Failure::BackupLayoutEvidence.into());
                }
                create_preparation(self, evidence, targets, original, stop)?
            }
        };
        self.require_root_source(&value)?;
        require_preparation(&value, evidence, targets, stop)?;
        let bytes = bounded_bytes(&value)?;
        // Repeat missing immutable copies and read every original member back
        // before any consumer can append this binding to its selected Config.
        for source in self.sources.values() {
            source.recheck(stop)?;
            let path = feedback_path(&source.root, value.operation_id);
            install_preparation_bytes(&source.root, &path, &bytes)?;
        }
        let confirmed = self.observe_target_roots(targets, stop)?;
        if confirmed.preparation != value {
            return Err(Failure::BackupRootFork.into());
        }
        Ok(confirmed)
    }

    /// Inspection cannot fill a missing source copy or create a target area.
    pub(in crate::engine) fn observe_target_roots(
        &self, targets: &[RetainedBackupTarget], stop: &AtomicBool,
    ) -> Result<PreparedBackupRoots> {
        let evidence = self.root_evidence()?;
        self.recheck_root_sources(stop)?;
        let mut saved = None;
        for source in self.sources.values() {
            let value = read_source(source, self.intent.operation_id)?
                .ok_or(Failure::BackupRootMissing)?;
            self.require_root_source(&value)?;
            merge_record(&mut saved, value)?;
        }
        let preparation = saved.ok_or(Failure::BackupRootMissing)?;
        require_preparation(&preparation, evidence, targets, stop)?;
        self.recheck_root_sources(stop)?;
        Ok(PreparedBackupRoots { preparation })
    }

    fn root_evidence(&self) -> Result<&BackupSourceFenceEvidence> {
        match &self.intent.origin {
            Origin::BackupCutover { evidence, .. } => Ok(evidence),
            Origin::Genesis { .. } | Origin::Successor { .. } | Origin::Readmission { .. }
            | Origin::TerminalResume { .. } => Err(Failure::BackupRootEvidence.into()),
        }
    }

    fn require_root_source(&self, value: &Preparation) -> Result<()> {
        if value.source_fence != SourceFenceHash::from_transition(&self.transition_hash)
            || value.operation_id != self.intent.operation_id
        {
            return Err(Failure::BackupRootEvidence.into());
        }
        Ok(())
    }

    fn read_root_preparation(&self, stop: &AtomicBool) -> Result<Option<Preparation>> {
        let mut saved = None;
        for source in self.sources.values() {
            source.recheck(stop)?;
            if let Some(value) = read_source(source, self.intent.operation_id)? {
                self.require_root_source(&value)?;
                merge_record(&mut saved, value)?;
            }
        }
        Ok(saved)
    }

    fn recheck_root_sources(&self, stop: &AtomicBool) -> Result<()> {
        for source in self.sources.values() {
            source.recheck(stop)?;
        }
        Ok(())
    }

    fn require_root_capacity(&self) -> Result<()> {
        for source in self.sources.values() {
            if source.root.available()? < RESERVE + MAX_FEEDBACK as u64 {
                return Err(Failure::BackupRootSpace.into());
            }
        }
        Ok(())
    }
}

fn create_preparation(
    choices: &BackupChoiceContext,
    evidence: &BackupSourceFenceEvidence,
    targets: &[RetainedBackupTarget],
    original: &AdmittedState,
    stop: &AtomicBool,
) -> Result<Preparation> {
    if targets.len() != evidence.root_target_count() {
        return Err(Failure::BackupRootEvidence.into());
    }
    let config = evidence.target_layout_configuration();
    let names = ConfiguredDiskNames::new(&config).map_err(crate::Error::ConfiguredDiskName)?;
    let mut identities = Vec::with_capacity(targets.len());
    // All targets and original member capacity are checked before the first
    // mkdir. Existing lifecycle names cannot be imported as a preparation.
    for (index, target) in targets.iter().enumerate() {
        let root = target.layout_root()?;
        preparation_checkpoint(stop)?;
        let id = evidence.require_root_target(index, root)?;
        if config.storage_roots.contains_key(&id) {
            return Err(Failure::BackupRootEvidence.into());
        }
        let path = names.source_path(id).map_err(crate::Error::ConfiguredDiskName)?;
        BackupTargetLayout::require_new_names(root, &path, original.state(), stop)?;
        for name in [&*layout::COMMON_DATA_ROOT, &*layout::DELETED_DATA_ROOT,
            &*layout::ZXS_DATA_ROOT]
        {
            if root.optional_metadata(&root.path().join(name.as_path()))?.is_some() {
                return Err(Failure::BackupRootUnrecorded.into());
            }
        }
        if root.available()? < RESERVE + MAX_FEEDBACK as u64 {
            return Err(Failure::BackupRootSpace.into());
        }
        identities.push((id, RootId::generate()?, [
            DirectoryId::generate()?, DirectoryId::generate()?, DirectoryId::generate()?,
            DirectoryId::generate()?, DirectoryId::generate()?,
        ]));
    }
    let mut roots = BTreeMap::new();
    for (index, (id, root_id, directories)) in identities.into_iter().enumerate() {
        preparation_checkpoint(stop)?;
        for source in choices.sources.values() {
            source.lease.check_running()?;
        }
        let root = targets[index].layout_root()?;
        evidence.require_root_target(index, root)?;
        let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
            .ok_or(Failure::BackupRootEvidence)?;
        let areas = HeldRootAreas::create(root, directories)?;
        areas.require_empty()?;
        roots.insert(id, RootBinding {
            layout: StorageLayout::UnifiedWorkingTreeV1,
            root_id,
            registration: fresh_layout::registration(&config, disk),
            source_chain: fresh_layout::source_chain(&config, disk)?,
            root_entity: local_entity(root.directory_identity(root.path())?),
            roots: areas.binding().clone(),
            operation_id: choices.intent.operation_id,
            generation: Generation::new(1),
        });
        evidence.require_root_target(index, root)?;
    }
    let value = Preparation {
        source_fence: SourceFenceHash::from_transition(&choices.transition_hash),
        operation_id: choices.intent.operation_id,
        roots,
    };
    bounded_bytes(&value)?;
    Ok(value)
}

fn require_preparation(
    value: &Preparation,
    evidence: &BackupSourceFenceEvidence,
    targets: &[RetainedBackupTarget],
    stop: &AtomicBool,
) -> Result<()> {
    if value.roots.len() != evidence.root_target_count()
        || targets.len() != evidence.root_target_count()
    {
        return Err(Failure::BackupRootEvidence.into());
    }
    let mut config = evidence.target_layout_configuration();
    for (index, target) in targets.iter().enumerate() {
        let root = target.layout_root()?;
        preparation_checkpoint(stop)?;
        let id = evidence.require_root_target(index, root)?;
        let binding = value.roots.get(&id).ok_or(Failure::BackupRootEvidence)?;
        if binding.operation_id != value.operation_id
            || binding.generation != Generation::new(1)
            || binding.root_entity != local_entity(root.directory_identity(root.path())?)
            || config.storage_roots.insert(id, binding.clone()).is_some()
        {
            return Err(Failure::BackupRootEvidence.into());
        }
        HeldRootAreas::observe(root, &binding.roots)?.verify(root)?;
        evidence.require_root_target(index, root)?;
    }
    // This preserves every original binding and validates all complete source
    // registrations and root/directory identity uniqueness, not just final IDs.
    fresh_layout::validate_roots(&config)
}

fn feedback_path(root: &DiskRoot, operation: OperationId) -> std::path::PathBuf {
    root.path().join(layout::BACKUP_ROOT_PREPARATIONS.as_path())
        .join(layout::backup_root_preparation(operation).as_path())
}

fn read_source(source: &Source, operation: OperationId) -> Result<Option<Preparation>> {
    let path = feedback_path(&source.root, operation);
    // The neutral feedback parent is absent on the first preparation. Probe
    // every ancestor from the same held root; unsafe entries still fail closed.
    if !crate::history_package::preparation_path_exists(&source.root, &path)? {
        return Ok(None);
    }
    let bytes = source.root.read_bytes(&path, MAX_FEEDBACK)?;
    let value: Preparation = decode_strict(&bytes).map_err(Failure::BackupRootRecord)?;
    if bounded_bytes(&value)? != bytes {
        return Err(Failure::BackupRootEvidence.into());
    }
    Ok(Some(value))
}

fn merge_record(saved: &mut Option<Preparation>, value: Preparation) -> Result<()> {
    match saved {
        Some(previous) if previous != &value => Err(Failure::BackupRootFork.into()),
        Some(_) => Ok(()),
        None => {
            *saved = Some(value);
            Ok(())
        }
    }
}

fn bounded_bytes(value: &Preparation) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_FEEDBACK {
        return Err(Failure::BackupRootBudget.into());
    }
    Ok(bytes)
}
