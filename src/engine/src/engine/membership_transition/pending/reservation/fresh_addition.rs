//! A fresh Add's request is not its final Config. Only exclusive creation and
//! synchronized source feedback can fill the new RootBinding. No target joins
//! the membership, becomes ready, or supplies business I/O during preparation.
use crate::{Config, DiskConfig, Engine, Result, ValidatedConfig, decode_strict, object_hash};
use crate::disk::{DiskRoot, MAX_METADATA, encode_metadata, fs_error};
use crate::engine::membership_transition::{MembershipFenceStatus, Origin};
use crate::engine::membership_transition::pending::{
    MemberIdentity, PendingDiskSeal, PendingMemberDirectory, admission,
};
use crate::engine::membership_transition::pending::reservation::{ReservedRoot, SourcePreparation};
use crate::journal::JournalWriter;
use crate::persist::layout;
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use crate::engine::fresh_layout::{self, FreshLayoutError as Failure};
use crate::engine::fresh_layout::root_areas::HeldRootAreas;
use crate::working_tree::records::file_identity;
use storage_contract::{DiskId, Generation, OperationId, Principal, Sequence};
use storage_contract::working_tree::{
    LocalDirectoryBinding, RootBinding,
};

mod physical;

const KIND: &str = "membership-fresh-root-preparation";

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Preparation {
    format: PreparationFormat,
    origin: Origin,
    request: Config,
    principal: Principal,
    operation_id: OperationId,
    disk_id: DiskId,
    stage: LocalDirectoryBinding,
    root: RootBinding,
}
#[derive(Serialize, Deserialize)]
enum PreparationFormat {
    #[serialization(rename = "membership-fresh-root-preparation")]
    FreshRoot,
}

/// Only `physical::HeldAddition::install`, after the original members have
/// synchronized the complete feedback, constructs this fixed-area operation.
/// It borrows the original writer/directory locks and grants no disk admission.
pub(crate) struct RecordedAdditionArea<'a> {
    root: &'a DiskRoot,
    stage: &'a DiskRoot,
    writer: &'a crate::disk::FenceOwner,
    preparation: &'a Preparation,
    name: &'a platform_io::fs_safety::EntryName,
    expected: &'a LocalDirectoryBinding,
}
impl RecordedAdditionArea<'_> {
    pub(crate) fn require(
        &self,
    ) -> Result<(&DiskRoot, &DiskRoot, &platform_io::fs_safety::EntryName)> {
        if admission::verify_named_root(self.root)?
            != file_identity(self.preparation.root.root_entity)
        {
            return Err(Failure::AdditionEvidence.into());
        }
        super::verify_writer(self.root, self.writer)?;
        let stage_name = layout::membership_layout_stage(self.preparation.stage.directory_id);
        let named = self.root.private_entry_matching(
            &stage_name, file_identity(self.preparation.stage.entity),
        ).map_err(fresh_layout::directory_error)?;
        if named.directory_identity(named.path())?
            != self.stage.directory_identity(self.stage.path())?
        {
            return Err(Failure::AdditionEvidence.into());
        }
        self.stage.private_entry_matching(self.name, file_identity(self.expected.entity))
            .map_err(fresh_layout::directory_error)?;
        Ok((self.stage, self.root, self.name))
    }
}

/// This product owns the original directory/writer fences. Its target is the
/// result of the actual synchronized creation, never caller-provided inodes.
pub(in crate::engine::membership_transition) struct PreparedAddition {
    source: ValidatedConfig,
    target: ValidatedConfig,
    roots: BTreeMap<DiskId, ReservedRoot>,
}
impl PreparedAddition {
    pub(in crate::engine::membership_transition) fn target(&self) -> &ValidatedConfig {
        &self.target
    }

    pub(in crate::engine::membership_transition) fn into_roots(
        self, engine: &Engine, target: &ValidatedConfig,
    ) -> Result<BTreeMap<String, ReservedRoot>> {
        if !engine.config.same_admission(&self.source) || !self.target.same_admission(target) {
            return Err(Failure::AdditionEvidence.into());
        }
        for root in self.roots.values() {
            if !root.layout.as_ref().is_some_and(|layout| layout.same_admission(target)) {
                return Err(Failure::AdditionEvidence.into());
            }
            root.verify_unsealed()?;
        }
        // The established handoff protocol still indexes its sealed roots by
        // the current wire spelling. Do not reparse those keys inside this producer.
        Ok(self.roots.into_iter().map(|(id, root)| (id.to_string(), root)).collect())
    }
}

/// Read-only pending evidence from the Engine's actual admitted Add fence.
/// Other control-only protocols cannot fill this field from a Config.
pub(in crate::engine::membership_transition) struct PendingRootLayout {
    binding: RootBinding,
    identity: MemberIdentity,
    target_hash: TargetConfigHash,
    areas: HeldRootAreas,
}
/// The existing PendingDiskSeal wire field remains text; only the actual
/// target encoder creates this private comparison value.
struct TargetConfigHash(String);

impl PendingRootLayout {
    pub(in crate::engine::membership_transition) fn verify(
        &self, root: &DiskRoot, seal: &PendingDiskSeal,
    ) -> Result<()> {
        if seal.identity != self.identity || seal.target_config_hash != self.target_hash.0
            || admission::verify_named_root(root)? != file_identity(self.binding.root_entity)
        {
            return Err(Failure::AdditionEvidence.into());
        }
        self.areas.verify(root)?;
        self.areas.require_empty()
    }
}

impl Engine {
    pub(in crate::engine::membership_transition) fn observe_pending_added_layout(
        &self, disk: &DiskConfig, root: &DiskRoot,
    ) -> Result<PendingRootLayout> {
        let fence = self.membership_fence.as_ref().ok_or(Failure::AdditionEvidence)?;
        if fence.intent.effect.is_some() || fence.intent.foreign.is_some()
            || !matches!(&fence.intent.origin, Origin::Genesis { .. } | Origin::Successor { .. })
        {
            return Err(Failure::AdditionEvidence.into());
        }
        let identity = fence.intent.added_roots.get(&disk.disk_id)
            .ok_or(Failure::AdditionEvidence)?;
        let target = &fence.intent.target;
        let physical = admission::verify_named_root(root)?;
        if admission::member_identity(target, disk, physical) != *identity {
            return Err(Failure::AdditionEvidence.into());
        }
        let binding = target.storage_roots.get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or(Failure::AdditionEvidence)?;
        if binding.operation_id != fence.intent.operation_id
            || file_identity(binding.root_entity) != physical
        {
            return Err(Failure::AdditionEvidence.into());
        }
        fresh_layout::verify_root(target, disk, root)?;
        let areas = HeldRootAreas::observe(root, &binding.roots)?;
        areas.require_empty()?;
        Ok(PendingRootLayout {
            binding: binding.clone(), identity: identity.clone(),
            target_hash: TargetConfigHash(object_hash("membership-target", target)?), areas,
        })
    }
}

fn origin(config: &ValidatedConfig) -> Origin {
    match config.generation_hash() {
        Some(hash) => Origin::Successor {
            config: config.clone(), predecessor_authority_hash: hash.to_owned(),
        },
        None => Origin::Genesis { config: config.clone() },
    }
}

fn selected(original: &ValidatedConfig, request: &Config, control: &Path) -> Result<DiskConfig> {
    crate::engine::config::validate_addition_input(request)?;
    if original.storage_roots.is_empty() || request.storage_roots != original.storage_roots {
        return Err(Failure::AdditionRequest.into());
    }
    let added = admission::validate_addition_members(original, request, control)?;
    // This first producer closes one actually empty device at a time. Batch
    // enrollment has no new implicit FD or partial-root cleanup budget here.
    let [disk] = added.as_slice() else {
        return Err(Failure::AdditionCount.into());
    };
    Ok(disk.clone())
}

impl Preparation {
    fn path(&self, disk: &DiskConfig) -> PathBuf {
        preparation_path(disk, self.operation_id)
    }

    fn target(&self, control: &Path) -> Result<ValidatedConfig> {
        let disk = selected(self.origin.config(), &self.request, control)?;
        if disk.disk_id.parse::<DiskId>()? != self.disk_id
            || self.root.registration != fresh_layout::registration(&self.request, &disk)
            || self.root.source_chain != fresh_layout::source_chain(&self.request, &disk)?
            || self.root.operation_id != self.operation_id
            || self.root.generation != Generation::new(1)
            || self.root.root_entity.inode() == 0
            || self.stage.entity.device() != self.root.root_entity.device()
            || self.stage.entity.inode() == 0
        {
            return Err(Failure::AdditionEvidence.into());
        }
        let mut target = self.request.clone();
        if target.storage_roots.insert(self.disk_id, self.root.clone()).is_some() {
            return Err(Failure::AdditionEvidence.into());
        }
        let target = ValidatedConfig::try_from(target)?;
        admission::validate_pure_add_target(self.origin.config(), &target, control)?;
        Ok(target)
    }

    fn require_request(
        &self, engine: &Engine, request: &Config, principal: &Principal, operation_id: OperationId,
    ) -> Result<()> {
        if self.operation_id != operation_id || &self.principal != principal
            || object_hash("membership-layout-origin", &self.origin)?
                != object_hash("membership-layout-origin", &origin(&engine.config))?
            || object_hash("membership-layout-request", &self.request)?
                != object_hash("membership-layout-request", request)?
        {
            return Err(Failure::AdditionConflict.into());
        }
        self.target(&engine.root)?;
        Ok(())
    }
}

fn preparation_path(disk: &DiskConfig, operation: OperationId) -> PathBuf {
    disk.root.join(layout::MEMBERSHIP_ROOT_PREPARATIONS.as_path())
        .join(layout::membership_root_preparation(operation).as_path())
}

impl Engine {
    pub(in crate::engine::membership_transition) fn begin_fresh_membership_add(
        &mut self, request: Config, principal: &str, operation: OperationId,
        directories: Option<Vec<PendingMemberDirectory>>,
    ) -> Result<MembershipFenceStatus> {
        let added = selected(&self.config, &request, &self.root)?;
        self.require_addition_chain(&request)?;
        let principal = principal.parse::<Principal>().map_err(Failure::AdditionPrincipal)?;
        self.require_handoff_source()?;
        if let Some(fence) = &self.membership_fence {
            let mut original_request = (*fence.intent.target).clone();
            original_request.storage_roots = fence.intent.origin.config().storage_roots.clone();
            if fence.intent.effect.is_some() || fence.intent.foreign.is_some()
                || object_hash("membership-layout-request", &original_request)?
                    != object_hash("membership-layout-request", &request)?
            {
                return Err(Failure::AdditionRequest.into());
            }
            let target = fence.intent.target.clone();
            return self.begin_membership_handoff_prepared(target, None, None,
                principal.as_str(), operation, directories, None);
        }
        self.require_write()?;
        if self.state.operations.contains_key(&operation) {
            return Err(Failure::AdditionConflict.into());
        }
        Sequence::new(self.next_slot).next().map_err(Failure::AdditionSequence)?;
        let members = self.members();
        self.validate_fence_members(&members)?;
        if self.journals.values().any(JournalWriter::needs_recovery) {
            return Err(Failure::AdditionJournals.into());
        }
        // This is bounded serialization before any new-root mutation, not an
        // authority minted from the request's Config or physical path.
        encode_metadata(&request, MAX_METADATA)?;
        let mut preparations = self.read_fresh_root_preparations()?;
        let saved = preparations.remove(&operation);
        if let Some(saved) = &saved {
            saved.require_request(self, &request, &principal, operation)?;
        }
        let root = match directories {
            Some(directories) => {
                let [directory]: [PendingMemberDirectory; 1] = directories.try_into()
                    .map_err(|_| Failure::AdditionRequest)?;
                if directory.disk_id != added.disk_id {
                    return Err(Failure::AdditionRequest.into());
                }
                DiskRoot::from_directory(added.root.clone(), directory.directory)?
            }
            None => DiskRoot::open(&added.root)?,
        };
        let held = physical::HeldAddition::lock(
            self, root, &added, operation, saved.as_ref(), &preparations,
        )?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_membership_layout_root_lock")?;
        let (saved, areas) = match saved {
            Some(saved) => {
                let areas = held.observe(&saved)?;
                (saved, areas)
            }
            None => held.create(self, request, &principal, operation, &added)?,
        };
        let target = saved.target(&self.root)?;
        // Repeat every original member's immutable write and both durability
        // barriers before any stage-to-final move, also on partial-write retry.
        self.persist_fresh_root_preparation(&saved)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_membership_layout_feedback")?;
        self.validate_fence_members(&self.members())?;
        // A lower data-only ancestor is not necessarily an original voter.
        // Feedback I/O does not retain the earlier named-source observation.
        self.require_addition_chain(&saved.request)?;
        let reserved = held.install(self, &saved, &target, areas)?;
        let source = SourcePreparation {
            origin: saved.origin.clone(), target: target.clone(),
            principal: principal.to_string(), operation_id: operation,
            added_roots: BTreeMap::from([(added.disk_id.clone(), reserved.identity.clone())]),
        };
        self.persist_source_preparation(&source)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_membership_layout_install")?;
        self.require_addition_chain(&saved.request)?;
        let prepared = PreparedAddition {
            source: self.config.clone(), target,
            roots: BTreeMap::from([(saved.disk_id, reserved)]),
        };
        self.begin_membership_handoff_prepared(prepared.target().clone(), None, None,
            principal.as_str(), operation, None, Some(prepared))
    }

    fn read_fresh_root_preparations(&self) -> Result<BTreeMap<OperationId, Preparation>> {
        let mut selected: BTreeMap<OperationId, Preparation> = BTreeMap::new();
        for disk in self.config.metadata_members() {
            let root = &self.disk_resources.get(disk)?.root;
            let directory = disk.root.join(layout::MEMBERSHIP_ROOT_PREPARATIONS.as_path());
            if root.optional_metadata(&directory)?.is_none() {
                continue;
            }
            for entry in root.entries(&directory)? {
                let entry = entry.map_err(fs_error)?;
                let path = directory.join(entry.as_path());
                if platform_io::fs_safety::AtomicFile::is_temporary_name(&entry) {
                    // An incomplete source write authorizes no target rename.
                    // Its stage is preserved; only a complete exact record is usable.
                    root.open_regular(&path)?;
                    continue;
                }
                let value: Preparation = decode_strict(&root.read_bytes(&path, MAX_METADATA)?)
                    .map_err(Failure::AdditionRecord)?;
                if value.path(disk) != path {
                    return Err(Failure::AdditionEvidence.into());
                }
                match selected.get(&value.operation_id) {
                    Some(previous)
                        if object_hash(KIND, previous)? != object_hash(KIND, &value)? =>
                    {
                        return Err(Failure::AdditionFork.into());
                    }
                    Some(_) => (),
                    None => {
                        selected.insert(value.operation_id, value);
                    }
                }
            }
        }
        Ok(selected)
    }

    fn persist_fresh_root_preparation(&mut self, value: &Preparation) -> Result<()> {
        for (index, disk) in self.members().iter().enumerate() {
            let root = &self.disk_resources.get(disk)?.root;
            root.write_json(&value.path(disk), value, false)?;
            let observed: Preparation = decode_strict(
                &root.read_bytes(&value.path(disk), MAX_METADATA)?)
                .map_err(Failure::AdditionRecord)?;
            if object_hash(KIND, &observed)? != object_hash(KIND, value)? {
                return Err(Failure::AdditionEvidence.into());
            }
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_layout_feedback_member_{}", index + 1))?;
            #[cfg(not(feature = "fault-injection"))]
            let _ = index;
        }
        Ok(())
    }
}
