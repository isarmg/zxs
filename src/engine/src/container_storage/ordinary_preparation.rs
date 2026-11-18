//! One observed container occurrence from exclusive preparation through decided
//! namespace installation. Published readers have no preparation-path fallback.
use super::*;
use crate::engine::fresh_layout::{FreshLayoutError as Failure, containers::ContainerPreparation};
use crate::persist::layout;
use platform_io::fs_safety::linux::FileIdentity;
use storage_contract::{DiskId, FileId};
use storage_contract::file_container::{ContainerEntity, FileContainerRecord};
use std::sync::OnceLock;

mod prefix;
mod backup;
pub(crate) use backup::{
    BackupLayoutPlan,
    BackupOriginalImage,
    BackupPayloadVersion,
    BackupServingImage,
    BackupServingVersion,
    BackupTargetLayout,
};
mod areas;
mod draft;
pub(crate) use draft::{HeldManualReader, ReadDraftDigest};
mod birth;
pub(in crate::container_storage) use birth::UnbornBirthFile;
mod working;
pub(crate) use working::{
    HeldUnbornReader, PreparedUnbornFull, PreparedWorkingSave,
    ReadWorkingDigest, WorkingWriteSession, CapturedPackedWorkingFiles, VerifiedWorkingPackFile,
};
mod directories;
mod rename;
use directories::HeldDirectories;
use storage_contract::file_container::ContainerParent;
use prefix::{HeldParent, HeldPrefixes, Observation};

pub(crate) struct ObservedContainerReplica {
    file: FileId,
    disk: Arc<DiskHandle>,
    record: FileContainerRecord,
    observed_name: name_policy::LookupName,
    parent: HeldParent,
    held: Arc<DiskRoot>,
    entity: ContainerEntity,
    draft: Option<draft::HeldDraft>,
    unborn: Option<Arc<working::HeldUnbornMaterial>>,
    first: Option<Arc<crate::engine::fresh_layout::containers::VerifiedUnbornFirst>>,
    rename: Option<rename::Rename>,
    // Initial publication and actual recovery may observe the same inode at
    // either end of the one decided rename. A published-only reader cannot.
    installed: OnceLock<()>,
    // Every shared descriptor keeps the one actual admission charged until its
    // last consumer closes. This Arc shares accounting, not a second reservation.
    _reservation: Arc<crate::engine::PayloadProofReservation>,
}
impl ObservedContainerReplica {
    pub(crate) fn open_working_draft(self: &Arc<Self>) -> Result<ReadFile> {
        self.verify()?;
        self.draft.as_ref().ok_or(Failure::ContainerScope)?.open(self)
    }

    pub(crate) fn read_incremental_draft(
        &self, output: &mut [u8], offset: u64,
    ) -> Result<usize> {
        self.verify()?;
        self.draft.as_ref().ok_or(Failure::ContainerScope)?.read_at(self, output, offset)
    }

    pub(crate) fn incremental_identity(&self)
        -> Result<storage_contract::file_draft::DraftPayload>
    {
        self.verify()?;
        match &self.record.working {
            storage_contract::file_draft::FileWorkingState::ManualIncremental { payload, .. } => {
                Ok(*payload)
            }
            storage_contract::file_draft::FileWorkingState::Unborn { .. }
            | storage_contract::file_draft::FileWorkingState::Automatic { .. }
            | storage_contract::file_draft::FileWorkingState::ManualClean { .. }
            | storage_contract::file_draft::FileWorkingState::ManualDirty { .. } => {
                Err(Failure::ContainerScope.into())
            }
        }
    }

    pub(crate) fn working_size(&self) -> Result<u64> {
        self.verify()?;
        self.record.working.draft().map(|(_, payload)| payload.logical_size())
            .ok_or_else(|| Failure::ContainerScope.into())
    }

    /// Opening authority remains the actual observed source. A manifest or a
    /// caller-supplied FileId cannot construct a reader on another container.
    pub(crate) fn open_unborn_working(
        self: &Arc<Self>, maximum: u64,
    ) -> Result<HeldUnbornReader> {
        HeldUnbornReader::capture(Arc::clone(self), maximum)
    }

    pub(crate) fn observe(
        source: crate::engine::fresh_layout::containers::PublishedReplicaSource<'_>,
    ) -> Result<Self> {
        use crate::engine::fresh_layout::containers::ReplicaNamespace;
        let location = source.location();
        let config = source.config();
        let resources = source.resources();
        let (file, record, disk, namespace, state) = source.into_parts();
        let id = disk.seal.disk_id.parse::<DiskId>()?;
        let replica = record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
        let container_descriptors = if record.working.draft().is_some() { 2 } else { 1 };
        let parent_count = match &record.namespace.parent {
            ContainerParent::Root | ContainerParent::Worktree { .. } => 0,
            ContainerParent::Directory(entry) => {
                crate::engine::fresh_layout::directories::chain(state, *entry)?.len()
            }
        };
        let count = replica.prefixes.len().checked_mul(6)
            .and_then(|count| count.checked_add(container_descriptors))
            .and_then(|count| count.checked_add(parent_count))
            .ok_or(Failure::ContainerBudget)?;
        let reservation = Arc::new(crate::engine::payload_proof_budget().reserve(count)
            .ok_or(Failure::ContainerBudget)?);
        let parent = match namespace {
            ReplicaNamespace::Owner(source) => {
                if !std::ptr::eq(source.common()?, disk.content_root()?) {
                    return Err(Failure::ContainerScope.into());
                }
                HeldParent::Common(Arc::clone(disk))
            }
            ReplicaNamespace::Worktree(source) => {
                if !std::ptr::eq(source.deleted()?, disk.deleted_root()?) {
                    return Err(Failure::ContainerScope.into());
                }
                HeldParent::Deleted(Arc::clone(disk))
            }
            ReplicaNamespace::Backup(source) => {
                prefix::published_parent(&source, disk, &replica.prefixes)?
            }
        };
        let parent = attach_directory_parent(
            config, resources, state, record, id, parent, Observation::Published,
            &reservation, &mut HeldDirectories::new(),
        )?;
        let installed = OnceLock::new();
        let (held, observed_name, rename) = match location {
            crate::engine::fresh_layout::containers::ReplicaReadLocation::Published(name) => {
                let held = if record.namespace.parent.is_worktree() {
                    open_record_container(parent.root()?, record, replica.entity)?
                } else {
                    open_published_container(parent.root()?, name, replica.entity)?
                };
                let _ = installed.set(());
                (held, name.clone(), None)
            }
            crate::engine::fresh_layout::containers::ReplicaReadLocation::Finalized {
                source, target, sequence,
            } => {
                let (held, binding, complete) = rename::recover_names(
                    parent.root()?, source, target, replica.entity, sequence,
                )?;
                if complete {
                    let _ = installed.set(());
                }
                (held, target.clone(), Some(binding))
            }
        };
        let mut result = Self {
            file, _reservation: reservation,
            disk: Arc::clone(disk), record: record.clone(), observed_name, parent,
            held: Arc::new(held), entity: replica.entity,
            draft: None, unborn: None, first: None, rename, installed,
        };
        if let Some((_, payload)) = record.working.draft() {
            result.draft = Some(draft::HeldDraft::capture(&result, state, *payload)?);
        }
        if record.working.unborn().is_some() {
            result.unborn = Some(Arc::new(working::HeldUnbornMaterial::capture(
                &result, config.max_file_bytes,
            )?));
        }
        result.verify()?;
        Ok(result)
    }

    pub(crate) fn container(&self) -> Result<&DiskRoot> {
        self.verify()?;
        Ok(&self.held)
    }

    pub(crate) fn current_zxs(&self) -> Result<&DiskRoot> {
        self.verify()?;
        self.parent.current_zxs()
    }

    pub(crate) fn disk(&self) -> &Arc<DiskHandle> {
        &self.disk
    }

    pub(crate) fn require_version(&self, version: &FileVersion) -> Result<()> {
        if version.file_id.parse::<FileId>()? != self.file
            || version.history_tree_id.parse::<storage_contract::HistoryTreeId>()?
                != self.record.history_tree_id
        {
            return Err(Failure::ContainerScope.into());
        }
        self.verify()
    }

    pub(crate) fn name(&self) -> &name_policy::LookupName {
        &self.observed_name
    }

    pub(super) fn require_rename_start(&self) -> Result<()> {
        self.verify()?;
        self.check_rename_start()
    }

    fn check_rename_start(&self) -> Result<()> {
        if self.rename.as_ref().is_some_and(|rename| !rename.finalized)
            && self.installed.get().is_none()
            && self.held.optional_metadata(
                &self.held.path().join(layout::VERSION_NAMES_INTENT.as_path()),
            )?.is_some()
        {
            return Err(Failure::ContainerIdentity.into());
        }
        Ok(())
    }

    pub(super) fn require_rename_binding(
        &self, source: &name_policy::Name, target: &name_policy::Name,
        sequence: storage_contract::Sequence,
    ) -> Result<()> {
        let rename = self.rename.as_ref().ok_or(Failure::ContainerScope)?;
        if source.as_str() != rename.source.as_str() || target.as_str() != self.name().as_str()
            || sequence != rename.sequence
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }

    pub(crate) fn body_path(&self, version: &FileVersion) -> Result<PathBuf> {
        self.require_version(version)?;
        self.body_path_for_material(version)
    }

    fn body_path_for_material(&self, version: &FileVersion) -> Result<PathBuf> {
        self.check_rename_start()?;
        if let Some(rename) = &self.rename {
            let expected = version_rename::CompletionBinding {
                file: self.file,
                history: self.record.history_tree_id,
                sequence: rename.sequence,
                target_name: name_policy::Name::parse(self.name().as_str())?,
            };
            if let Some(path) = version_rename::interrupted_path_bound(
                &self.held, self.held.path(), version, &rename.source, &expected,
            )? {
                return Ok(path);
            }
            if self.installed.get().is_none() {
                return ordinary_body_path(self.held.path(), version, &rename.source);
            }
        }
        ordinary_body_path(self.held.path(), version, self.name())
    }

    pub(crate) fn verify_version_body(&self, version: &FileVersion) -> Result<()> {
        let reservation = crate::engine::payload_proof_budget().reserve(1)
            .ok_or(Failure::ContainerBudget)?;
        let path = self.body_path(version)?;
        let body = verify_body(&self.held, &path, version)?;
        self.verify()?;
        drop(body);
        drop(reservation);
        Ok(())
    }

    fn identity(&self) -> FileIdentity {
        FileIdentity { device: self.entity.device(), inode: self.entity.inode() }
    }

    fn physical_name(&self) -> platform_io::fs_safety::EntryName {
        if self.record.namespace.parent.is_worktree() {
            layout::namespace::container(&self.record.namespace)
        } else {
            layout::namespace::directory(&self.observed_name)
        }
    }

    fn named_path(&self) -> Result<PathBuf> {
        Ok(self.parent.root()?.path()
            .join(self.physical_name().as_path()))
    }

    fn preparation_path(&self) -> Result<PathBuf> {
        if let Some(rename) = &self.rename {
            return Ok(self.parent.root()?.path()
                .join(layout::namespace::directory(&rename.source).as_path()));
        }
        let id = self.disk.seal.disk_id.parse::<DiskId>()?;
        let replica = self.record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
        Ok(self.disk.management_root()?.path()
            .join(layout::container::preparation(replica.preparation).as_path()))
    }

    fn origin_root(&self) -> Result<&DiskRoot> {
        match &self.rename {
            Some(_) => self.parent.root(),
            None => self.disk.management_root(),
        }
    }

    pub(super) fn current_path(&self) -> Result<PathBuf> {
        self.verify()?;
        if self.installed.get().is_some() {
            // The held parent still carries its original preparation spelling
            // after its inode is installed. Export the decided address only
            // after verify() has checked that same held entity at its real name.
            Ok(self.parent.published_path()?
                .join(self.physical_name().as_path()))
        } else {
            self.preparation_path()
        }
    }

    pub(super) fn has_rename(&self) -> bool {
        self.rename.is_some()
    }

    fn verify_entity(&self) -> Result<()> {
        let (root, path) = if self.installed.get().is_some() {
            (self.parent.root()?, self.named_path()?)
        } else {
            (self.origin_root()?, self.preparation_path()?)
        };
        let current = root.private_child_matching(&path, self.identity())?;
        if current.directory_identity(current.path())?
            != self.held.directory_identity(self.held.path())?
        {
            return Err(Failure::ContainerIdentity.into());
        }
        Ok(())
    }

    pub(crate) fn verify(&self) -> Result<()> {
        self.verify_entity()?;
        self.parent.current_zxs()?;
        match (&self.draft, self.record.working.draft()) {
            (Some(draft), Some(_)) => draft.validate(self),
            (None, None) => Ok(()),
            (Some(_), None) | (None, Some(_)) => Err(Failure::ContainerScope.into()),
        }?;
        match (&self.unborn, self.record.working.unborn()) {
            (Some(material), Some(_)) => material.validate(self),
            (None, None) => Ok(()),
            (Some(_), None) | (None, Some(_)) => Err(Failure::ContainerScope.into()),
        }?;
        if let Some(first) = &self.first {
            first.require_record(self.file, &self.record)?;
        }
        Ok(())
    }

    /// Only PreparedNames consumes this after the real Accept/Finalize gate.
    fn install(&self, observe: &mut impl FnMut(&'static str) -> Result<()>) -> Result<()> {
        self.verify()?;
        self.parent.install()?;
        if self.installed.get().is_some() {
            // A restart may observe the rename after a failed directory sync.
            // The actual decision installer must complete both durability tails.
            let target = self.parent.root()?;
            target.sync_dir(target.path())?;
            if self.rename.is_none() {
                let source = self.disk.management_root()?;
                source.sync_dir(source.path())?;
            }
            return Ok(());
        }
        let source = self.origin_root()?;
        let target = self.parent.root()?;
        let from = self.preparation_path()?;
        let to = self.named_path()?;
        // Never remove, exchange or infer ownership of a colliding user entry.
        if target.optional_metadata(&to)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        if self.rename.is_some() {
            // One parent FD: preserve the primitive's same-inode sync de-duplication.
            target.rename_noreplace_synced(&from, &to)?;
        } else {
            source.rename_to_held_synced(&from, target, &to)?;
        }
        target.private_child_matching(&to, self.identity())?;
        self.installed.set(()).map_err(|_| Failure::ContainerIdentity)?;
        self.verify()?;
        if self.rename.is_some() {
            observe("ordinary-container-renamed")?;
        }
        Ok(())
    }
}

pub(crate) struct PreparedContainerEntities {
    held: BTreeMap<(FileId, DiskId), Arc<ObservedContainerReplica>>,
    prefixes: HeldPrefixes,
    directories: HeldDirectories,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    draft_reads: Vec<Arc<ReadDraftDigest>>,
    birth: Option<Arc<crate::engine::fresh_layout::containers::VerifiedUnbornBirth>>,
    _reservation: Arc<crate::engine::PayloadProofReservation>,
}
/// Best-effort local-proof discovery is not a complete installation set. This
/// wrapper deliberately cannot be passed to PreparedNames or unwrapped by it.
pub(crate) struct AvailableCandidateContainers {
    observed: PreparedContainerEntities,
}
impl AvailableCandidateContainers {
    pub(crate) fn scope(&self, file: FileId, disk: DiskId) -> Result<&ObservedContainerReplica> {
        self.observed.scope(file, disk)
    }
}

#[derive(Clone, Copy)]
enum CandidateAvailability {
    Required,
    Available,
}

impl PreparedContainerEntities {
    /// Budgets have already admitted this exact plan. No user-name entry is
    /// created until its Candidate has crossed the existing decision gate.
    pub(crate) fn prepare(
        context: &crate::engine::PlanningContext<'_>,
        plan: &ContainerPreparation,
    ) -> Result<Self> {
        Self::prepare_with_draft(context, plan, None)
    }

    pub(crate) fn prepare_with_draft(
        context: &crate::engine::PlanningContext<'_>,
        plan: &ContainerPreparation,
        input: Option<&mut crate::engine::file_draft::DraftPublicationInput>,
    ) -> Result<Self> {
        let directory_targets = directories::targets(
            plan.budget_state(), Some(plan.targets()), Some(plan.directory_targets()),
        )?;
        let count = descriptor_count(
            plan.budget_state(), Some(plan.targets()), &directory_targets,
        )?;
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(count)
            .ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
        let mut directories = HeldDirectories::new();
        directories::preflight(context.config, context.disk_resources, context.state,
            plan.budget_state(), &directory_targets)?;
        // Check every component against the actual held target before any mkdir.
        // Existing source directories are not retroactively charged as new names.
        for (file, id) in plan.targets() {
            crate::engine::fresh_layout::containers::require_current_container(
                plan.budget_state(), *file,
            )?;
            let record = &plan.budget_state().file_containers[file];
            let disk = configured_disk(context.config, *id)?;
            let handle = context.disk_resources.get(disk)?;
            if *id != record.owner_disk {
                let source = context.prepare_backup_source_path(
                    record.namespace.repository, record.namespace.branch, *id,
                )?;
                source.require_context(context)?;
                if record.replicas[id].prefixes.iter().any(|prefix| prefix.entity.inode() == 0) {
                    source.verify_new_component_budget()?;
                    for name in [
                        &*layout::COMMON_DATA_ROOT, &*layout::DELETED_DATA_ROOT,
                        &*layout::ZXS_DATA_ROOT,
                    ] {
                        version_names::check_target_name(source.target_root()?, name)?;
                    }
                }
            } else {
                require_owner(context, record, *id)?;
            }
            let common = if record.namespace.parent.is_worktree() {
                handle.deleted_root()?
            } else { handle.content_root()? };
            if plan.rename(*file).is_some() || plan.additions().contains_key(&(*file, *id)) {
                require_introduced_user_name(common, &record.namespace.name)?;
            }
            if let Some((_, payload)) = record.working.draft() {
                version_names::check_target_name(common, &layout::container::DRAFTS)?;
                version_names::check_target_name(common,
                    &layout::container::draft_payload(payload.payload_id()))?;
            }
            version_names::check_target_name(
                common, &layout::namespace::container(&record.namespace),
            )?;
            let name = name_policy::Name::parse(record.namespace.name.as_str())?;
            for version in plan.budget_state().versions.values()
                .filter(|version| version.file_id == file.to_string())
            {
                version_names::check_target_name(
                    common, &installed_version_leaf(version.storage_id, &name)?,
                )?;
            }
            let replica = record.replicas.get(id).ok_or(Failure::ContainerScope)?;
            version_names::check_target_name(
                handle.management_root()?, &layout::container::preparation(replica.preparation),
            )?;
            for prefix in &replica.prefixes {
                version_names::check_target_name(handle.management_root()?,
                    &layout::container::prefix_preparation(prefix.preparation))?;
            }
        }
        for (entry, disk) in directories::ordered(plan.budget_state(), directory_targets)? {
            directories::bind(context.config, context.disk_resources, context.state,
                plan.budget_state(), entry, disk, Observation::Prepare, &reservation,
                &mut directories, &mut prefixes)?;
        }
        for (file, id) in plan.targets() {
            crate::engine::fresh_layout::containers::require_current_container(
                plan.budget_state(), *file,
            )?;
            let record = &plan.budget_state().file_containers[file];
            let disk = configured_disk(context.config, *id)?;
            let handle = context.disk_resources.get(disk)?;
            let parent = bind_parent(context, record, *id, Observation::Prepare, &mut prefixes)?;
            let parent = attach_directory_parent(context.config, context.disk_resources,
                plan.budget_state(), record, *id, parent, Observation::Prepare,
                &reservation, &mut directories)?;
            let target = parent.root()?;
            let replica = record.replicas.get(id).ok_or(Failure::ContainerScope)?;
            let installed = OnceLock::new();
            let mut rename = None;
            let (directory, entity) = if let Some(change) = plan.rename(*file) {
                let (directory, binding) = rename::prepare(
                    target, change, replica.entity,
                    storage_contract::Sequence::new(plan.budget_state().sequence),
                )?;
                rename = Some(binding);
                (directory, replica.entity)
            } else if plan.additions().contains_key(&(*file, *id)) {
                let named = target.path()
                    .join(layout::namespace::container(&record.namespace).as_path());
                if target.optional_metadata(&named)?.is_some() {
                    return Err(Failure::ContainerIdentity.into());
                }
                let (directory, identity) = prefix::create_exclusive(handle.management_root()?,
                    &layout::container::preparation(replica.preparation))?;
                directory.private_dir(&directory.path().join(layout::VERSIONS.as_path()))?;
                directory.sync_dir(directory.path())?;
                (directory, ContainerEntity::new(identity.device, identity.inode))
            } else {
                let directory = open_record_container(target, record, replica.entity)?;
                let _ = installed.set(());
                (directory, replica.entity)
            };
            held.insert((*file, *id), ObservedContainerReplica {
                file: *file, _reservation: Arc::clone(&reservation),
                disk: Arc::clone(handle), record: record.clone(),
                    observed_name: record.namespace.name.clone(), parent,
                held: Arc::new(directory), entity, draft: None, unborn: None,
                first: None, rename, installed,
            });
        }
        draft::populate(&mut held, input, context.config.max_file_bytes, plan.budget_state())?;
        working::populate(&mut held, context.config.max_file_bytes)?;
        retain_first_sources(&mut held, plan.first_sources())?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, directories, config: context.config.clone(),
            resources: Arc::clone(context.disk_resources),
            birth: plan.birth().map(Arc::clone), _reservation: reservation,
            draft_reads: plan.draft_reads().to_vec(),
        };
        result.verify()?;
        Ok(result)
    }

    /// Read-only observation of the actual hash-bound original snapshot. The
    /// caller retains the admitted recovery stream; only its later decision
    /// installer can expose this exact preparation under the user name.
    pub(crate) fn observe_candidate(
        context: &crate::engine::PlanningContext<'_>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
    ) -> Result<Self> {
        Self::observe_candidate_from_prefix(
            context.config, context.disk_resources, context.state, context.snapshot,
            candidate, original,
        )
    }

    pub(crate) fn observe_candidate_with_locations(
        context: &crate::engine::PlanningContext<'_>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        locations: &crate::engine::fresh_layout::containers::RecoveryRenameLocations<'_>,
    ) -> Result<Self> {
        Self::observe_candidate_inner(
            context.config, context.disk_resources, context.state, context.snapshot,
            candidate, original, CandidateAvailability::Required, Some(locations), None,
        )
    }

    /// Actual recovery prefix, not a synthesized PlanningContext or an inferred
    /// directory source. This remains read-only and cannot expose preparations.
    pub(crate) fn observe_candidate_from_prefix(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        previous: &State,
        previous_snapshot: &Option<String>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
    ) -> Result<Self> {
        Self::observe_candidate_inner(
            config, resources, previous, previous_snapshot, candidate, original,
            CandidateAvailability::Required, None, None,
        )
    }

    /// Preserve read-only discovery on the actual available candidate routes.
    /// No missing resource becomes absence evidence or completion permission.
    pub(crate) fn observe_available_candidate_from_prefix(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        previous: &State,
        previous_snapshot: &Option<String>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
    ) -> Result<AvailableCandidateContainers> {
        let observed = Self::observe_candidate_inner(
            config, resources, previous, previous_snapshot, candidate, original,
            CandidateAvailability::Available, None, None,
        )?;
        Ok(AvailableCandidateContainers { observed })
    }

    /// The same original-snapshot observer, with the independently completed
    /// directory native replay instead of an unrecognized ordinary working mode.
    pub(crate) fn observe_directory_candidate(
        context: &crate::engine::PlanningContext<'_>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        proof: &crate::snapshot_worktree::drafts::VerifiedDirectoryDraft<'_>,
    ) -> Result<Self> {
        Self::observe_candidate_inner(
            context.config, context.disk_resources, context.state, context.snapshot,
            candidate, original, CandidateAvailability::Required, None, Some(proof),
        )
    }

    pub(crate) fn observe_directory_candidate_from_prefix(
        source: &crate::engine::DirectoryReplayPrefix<'_>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        proof: &crate::snapshot_worktree::drafts::VerifiedDirectoryDraft<'_>,
    ) -> Result<Self> {
        Self::observe_candidate_inner(
            source.config(), source.resources(), source.previous(), source.snapshot(),
            candidate, original, CandidateAvailability::Required, None, Some(proof),
        )
    }

    fn observe_candidate_inner(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        previous: &State,
        previous_snapshot: &Option<String>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        availability: CandidateAvailability,
        locations: Option<&crate::engine::fresh_layout::containers::RecoveryRenameLocations<'_>>,
        directory: Option<&crate::snapshot_worktree::drafts::VerifiedDirectoryDraft<'_>>,
    ) -> Result<Self> {
        let next = original.for_candidate(candidate)?.state();
        if &candidate.previous_snapshot != previous_snapshot
            || next.sequence != candidate.sequence
        {
            return Err(Failure::ContainerScope.into());
        }
        let transition = match directory {
            Some(proof) => crate::engine::fresh_layout::containers::verify_directory_transition(
                previous, next, candidate, proof,
            )?,
            None => crate::engine::fresh_layout::containers::verify_transition_with_working_source(
                config, resources, previous, next, candidate, locations,
            )?,
        };
        let targets = match availability {
            CandidateAvailability::Required => None,
            CandidateAvailability::Available => {
                let mut targets = BTreeSet::new();
                for evidence in &candidate.versions {
                    let file = evidence.version.file_id.parse::<FileId>()?;
                    for slot in &evidence.placement.route_slot_ids {
                        let disk = config.disks.iter()
                            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
                            .ok_or(Failure::Registration)?;
                        if resources.available(disk).is_some() {
                            targets.insert((file, disk.disk_id.parse::<DiskId>()?));
                        }
                    }
                }
                Some(targets)
            }
        };
        let directory_targets = directories::targets(next, targets.as_ref(), None)?;
        let count = descriptor_count(next, targets.as_ref(), &directory_targets)?;
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(count).ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
        let mut directories = HeldDirectories::new();
        for (entry, disk) in directory_targets {
            directories::bind(config, resources, previous, next, entry, disk,
                Observation::Candidate(previous), &reservation, &mut directories,
                &mut prefixes)?;
        }
        for (file, record) in next.file_containers.iter() {
            if targets.as_ref().is_some_and(|targets| {
                !record.replicas.keys().any(|id| targets.contains(&(*file, *id)))
            }) {
                continue;
            }
            crate::engine::fresh_layout::containers::require_current_container(next, *file)?;
            for (id, replica) in &record.replicas {
                if targets.as_ref().is_some_and(|targets| !targets.contains(&(*file, *id))) {
                    continue;
                }
                let disk = configured_disk(config, *id)?;
                let handle = resources.get(disk)?;
                let parent = bind_read_parent(config, resources, previous, record, *id,
                    Observation::Candidate(previous), &mut prefixes)?;
                let parent = attach_directory_parent(config, resources, next,
                    record, *id, parent, Observation::Candidate(previous),
                    &reservation, &mut directories)?;
                let common = parent.root()?;
                let management = handle.management_root()?;
                let named = common.path()
                    .join(layout::namespace::container(&record.namespace).as_path());
                let stage = management.path()
                    .join(layout::container::preparation(replica.preparation).as_path());
                let identity = FileIdentity {
                    device: replica.entity.device(), inode: replica.entity.inode(),
                };
                let installed = OnceLock::new();
                let mut rename = None;
                let old = previous.file_containers.get(file);
                let directory = if let Some(old) = old.filter(|old| {
                    old.namespace.name != record.namespace.name
                }) {
                    if !parent.installed() {
                        return Err(Failure::ContainerIdentity.into());
                    }
                    let permit_target = locations.map(|locations| {
                        locations.permits_target(previous, next, candidate, *file, *id)
                    }).transpose()?.unwrap_or(false);
                    let (directory, binding, at_target) = rename::recover(
                        common, old, record, replica.entity,
                        storage_contract::Sequence::new(candidate.sequence), permit_target,
                    )?;
                    rename = Some(binding);
                    if at_target {
                        let _ = installed.set(());
                    }
                    directory
                } else if previous.file_containers.get(file)
                    .is_some_and(|old| old.replicas.contains_key(id))
                {
                    if !parent.installed() {
                        return Err(Failure::ContainerIdentity.into());
                    }
                    let _ = installed.set(());
                    common.private_child_matching(&named, identity)?
                } else {
                    let at_name = common.optional_metadata(&named)?.is_some();
                    let at_stage = management.optional_metadata(&stage)?.is_some();
                    match (at_name, at_stage) {
                        (false, true) => management.private_child_matching(&stage, identity)?,
                        (true, false) if parent.installed() => {
                            let directory = common.private_child_matching(&named, identity)?;
                            let _ = installed.set(());
                            directory
                        }
                        (true, true) | (false, false) | (true, false) => {
                            return Err(Failure::ContainerIdentity.into());
                        }
                    }
                };
                held.insert((*file, *id), ObservedContainerReplica {
                    file: *file, _reservation: Arc::clone(&reservation),
                    disk: Arc::clone(handle), record: record.clone(),
                    observed_name: record.namespace.name.clone(), parent,
                    held: Arc::new(directory), entity: replica.entity,
                    draft: None, unborn: None, first: None, rename, installed,
                });
            }
        }
        draft::populate(&mut held, None, config.max_file_bytes, next)?;
        working::populate(&mut held, config.max_file_bytes)?;
        retain_first_sources(&mut held, transition.first_sources())?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, directories, config: config.clone(),
            resources: Arc::clone(resources),
            birth: transition.birth().map(Arc::clone), _reservation: reservation,
            draft_reads: transition.draft_reads().to_vec(),
        };
        result.verify()?;
        Ok(result)
    }

    pub(crate) fn published(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        state: &State,
    ) -> Result<Self> {
        crate::engine::fresh_layout::containers::validate_state(state)?;
        let directory_targets = directories::targets(state, None, None)?;
        let count = descriptor_count(state, None, &directory_targets)?;
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(count).ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
        let mut directories = HeldDirectories::new();
        for (entry, id) in directory_targets {
            let disk = configured_disk(config, id)?;
            if resources.available(disk).is_some() {
                directories::bind(config, resources, state, state, entry, id,
                    Observation::Published, &reservation, &mut directories, &mut prefixes)?;
            }
        }
        for (file, record) in state.file_containers.iter() {
            crate::engine::fresh_layout::containers::require_current_container(state, *file)?;
            for (id, replica) in &record.replicas {
                let disk = config.disks.iter().find(|disk| disk.disk_id == id.to_string())
                    .ok_or(Failure::Registration)?;
                let handle = if record.working.draft().is_some() {
                    // Dirty state names one owner-only body. Missing that owner
                    // cannot turn the complete draft observation into no work.
                    resources.get(disk)?
                } else {
                    let Some(handle) = resources.available(disk) else { continue; };
                    handle
                };
                let source = crate::engine::fresh_layout::containers::published_source(
                    config, resources, state, *file, disk,
                )?;
                let (_, _, observed_handle, namespace, _) = source.into_parts();
                if !Arc::ptr_eq(handle, observed_handle) {
                    return Err(Failure::ContainerScope.into());
                }
                let parent = match namespace {
                    crate::engine::fresh_layout::containers::ReplicaNamespace::Owner(source) => {
                        if !std::ptr::eq(source.common()?, handle.content_root()?) {
                            return Err(Failure::ContainerScope.into());
                        }
                        HeldParent::Common(Arc::clone(handle))
                    }
                    crate::engine::fresh_layout::containers::ReplicaNamespace::Worktree(source) => {
                        if !std::ptr::eq(source.deleted()?, handle.deleted_root()?) {
                            return Err(Failure::ContainerScope.into());
                        }
                        HeldParent::Deleted(Arc::clone(handle))
                    }
                    crate::engine::fresh_layout::containers::ReplicaNamespace::Backup(source) => {
                        prefix::bind_published_chain(
                            &source, handle, &replica.prefixes, &mut prefixes,
                        )?
                    }
                };
                let parent = attach_directory_parent(config, resources, state,
                    record, *id, parent, Observation::Published, &reservation, &mut directories)?;
                let directory = open_record_container(parent.root()?, record, replica.entity)?;
                let installed = OnceLock::new();
                let _ = installed.set(());
                held.insert((*file, *id), ObservedContainerReplica {
                    file: *file, _reservation: Arc::clone(&reservation),
                    disk: Arc::clone(handle), record: record.clone(),
                    observed_name: record.namespace.name.clone(), parent,
                    held: Arc::new(directory), entity: replica.entity,
                    draft: None, unborn: None, first: None, rename: None, installed,
                });
            }
        }
        draft::populate(&mut held, None, config.max_file_bytes, state)?;
        working::populate(&mut held, config.max_file_bytes)?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, directories, config: config.clone(), resources: Arc::clone(resources),
            birth: None, _reservation: reservation, draft_reads: Vec::new(),
        };
        result.verify()?;
        Ok(result)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        crate::engine::fresh_layout::verify_available(&self.config, &self.resources)?;
        for read in &self.draft_reads { read.require_source()?; }
        for prefix in self.prefixes.values() {
            prefix.verify()?;
        }
        for directory in self.directories.values() {
            directory.verify()?;
        }
        for container in self.held.values() {
            container.verify()?;
        }
        Ok(())
    }

    /// Capacity accounting for the already held non-Version material. This
    /// does not select a root or grant an additional installation target.
    pub(crate) fn retains_draft_on(&self, disk: DiskId) -> bool {
        self.held.iter().any(|((_, id), container)| {
            *id == disk && container.draft.is_some()
        }) || self.draft_reads.iter().any(|read| read.owner() == disk)
    }

    pub(crate) fn directory_record(
        &self, entry: storage_contract::EntryId, disk: DiskId,
    ) -> Result<storage_contract::file_container::NamespaceDirectoryReplica> {
        let mut record = self.directories.get(&(entry, disk))
            .ok_or(Failure::ContainerScope)?.record()?;
        for source in &mut record.source_prefixes {
            *source = prefix::record(&self.prefixes, disk, source.source_disk)?;
        }
        Ok(record)
    }

    pub(crate) fn entity(&self, file: FileId, disk: DiskId) -> Result<ContainerEntity> {
        let container = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        Ok(container.entity)
    }

    pub(crate) fn prefix_record(
        &self, disk: DiskId, source: DiskId,
    ) -> Result<storage_contract::file_container::ContainerPrefix> {
        prefix::record(&self.prefixes, disk, source)
    }

    pub(in crate::container_storage) fn birth_file(
        &self, file: FileId, disk: DiskId,
    ) -> Result<Option<UnbornBirthFile<'_>>> {
        let Some(proof) = self.birth.as_ref().filter(|proof| proof.file() == file) else {
            return Ok(None);
        };
        if disk != proof.owner() { return Err(Failure::ContainerScope.into()); }
        let scope = self.scope(file, disk)?;
        Ok(Some(UnbornBirthFile::observe(proof, scope)?))
    }

    pub(crate) fn scope(&self, file: FileId, disk: DiskId) -> Result<&ObservedContainerReplica> {
        let scope = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        scope.verify()?;
        Ok(scope)
    }

    pub(crate) fn retain_scope(
        &self, file: FileId, disk: DiskId,
    ) -> Result<Arc<ObservedContainerReplica>> {
        let scope = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        scope.verify()?;
        Ok(Arc::clone(scope))
    }

    pub(crate) fn target(&self, file: FileId, disk: DiskId) -> Result<&DiskRoot> {
        let container = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        Ok(&container.held)
    }

    pub(crate) fn name(&self, file: FileId, disk: DiskId) -> Result<&name_policy::LookupName> {
        let container = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        Ok(&container.record.namespace.name)
    }

    pub(super) fn published_path(&self, file: FileId, disk: DiskId) -> Result<PathBuf> {
        let container = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        Ok(container.parent.published_path()?
            .join(layout::namespace::container(&container.record.namespace).as_path()))
    }

    /// The actual target occurrence names its direct source, which need not
    /// be the preceding target in a fanout's deterministic publication order.
    pub(crate) fn direct_source(&self, file: FileId, target: DiskId) -> Result<Option<DiskId>> {
        let container = self.held.get(&(file, target)).ok_or(Failure::ContainerScope)?;
        let source = container.record.replicas.get(&target)
            .ok_or(Failure::ContainerScope)?.prefixes.first().map(|prefix| prefix.source_disk);
        self.require_direct_source(file, target, source)?;
        Ok(source)
    }

    /// The producer supplies the identity attached to an actually installed
    /// direct-source BODY descriptor, not a route-neighbor or prefix string.
    pub(crate) fn require_direct_source(
        &self,
        file: FileId,
        target: DiskId,
        previous: Option<DiskId>,
    ) -> Result<()> {
        let container = self.held.get(&(file, target)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        let replica = container.record.replicas.get(&target).ok_or(Failure::ContainerScope)?;
        let expected = replica.prefixes.first().map(|prefix| prefix.source_disk);
        if previous != expected || (previous.is_none() && target != container.record.owner_disk) {
            return Err(Failure::ContainerScope.into());
        }
        if let Some(previous) = previous {
            // A C destination B/A must copy the same file's held B/A occurrence,
            // not an A upload or another equal-byte source selected by pathname.
            let upstream = self.held.get(&(file, previous)).ok_or(Failure::ContainerScope)?;
            upstream.verify()?;
            let origin = upstream.record.replicas.get(&previous)
                .ok_or(Failure::ContainerScope)?;
            if upstream.record.owner_disk != container.record.owner_disk
                || upstream.record.history_tree_id != container.record.history_tree_id
                || replica.prefixes.iter().skip(1).map(|prefix| prefix.source_disk)
                    .ne(origin.prefixes.iter().map(|prefix| prefix.source_disk))
            {
                return Err(Failure::ContainerScope.into());
            }
        }
        Ok(())
    }

    pub(crate) fn open_draft(&self, file: FileId) -> Result<ReadFile> {
        let mut targets = self.held.iter().filter(|((id, _), _)| *id == file);
        let (_, container) = targets.next().ok_or(Failure::ContainerScope)?;
        if targets.next().is_some() || container.record.replicas.len() != 1 {
            return Err(Failure::ContainerScope.into());
        }
        self.verify()?;
        container.open_working_draft()
    }

    pub(crate) fn body_path(
        &self, file: FileId, disk: DiskId, version: &FileVersion,
    ) -> Result<PathBuf> {
        self.scope(file, disk)?.body_path(version)
    }

    pub(super) fn share_target(&self, file: FileId, disk: DiskId) -> Result<Arc<DiskRoot>> {
        let container = self.held.get(&(file, disk)).ok_or(Failure::ContainerScope)?;
        container.verify()?;
        Ok(Arc::clone(&container.held))
    }

    pub(super) fn install_decided(
        &self, observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        self.verify()?;
        for directory in self.directories.values() {
            directory.install()?;
        }
        for container in self.held.values() {
            container.install(observe)?;
        }
        self.verify()
    }
}

// All ordinary descendants are bound with SameMount, so a held target root
// has the destination's limit even before its new parent directories exist.
fn require_introduced_user_name(root: &DiskRoot, name: &name_policy::LookupName) -> Result<()> {
    let maximum = root.component_name_maximum()?;
    crate::name_budget::NameBudget::from_maximum(maximum)
        .and_then(|budget| budget.component(name.as_str(), name.as_str()))
        .map_err(Error::ConversionNameBudget)
}

fn open_record_container(
    root: &DiskRoot, record: &FileContainerRecord, expected: ContainerEntity,
) -> Result<DiskRoot> {
    let path = root.path().join(layout::namespace::container(&record.namespace).as_path());
    root.private_child_matching(&path, FileIdentity {
        device: expected.device(), inode: expected.inode(),
    })
}

pub(crate) fn open_published_container(
    root: &DiskRoot,
    name: &name_policy::LookupName,
    expected: ContainerEntity,
) -> Result<DiskRoot> {
    let path = root.path().join(layout::namespace::directory(name).as_path());
    root.private_child_matching(&path, FileIdentity {
        device: expected.device(), inode: expected.inode(),
    })
}

pub(crate) fn ordinary_body_path(
    container: &std::path::Path,
    version: &FileVersion,
    name: &name_policy::LookupName,
) -> Result<PathBuf> {
    Ok(container.join(layout::VERSIONS.as_path()).join(
        installed_version_leaf(version.storage_id, &name_policy::Name::parse(name.as_str())?)?
            .as_path(),
    ))
}

fn configured_disk(config: &Config, id: DiskId) -> Result<&DiskConfig> {
    config.disks.iter().find(|disk| disk.disk_id == id.to_string())
        .ok_or_else(|| Failure::Registration.into())
}

fn require_owner(
    context: &crate::engine::PlanningContext<'_>,
    record: &FileContainerRecord,
    id: DiskId,
) -> Result<()> {
    let source = context.namespace_host_root(record.namespace.repository, record.namespace.branch)?;
    if record.owner_disk != id || source.disk().disk_id.parse::<DiskId>()? != id {
        return Err(Failure::NamespaceSource.into());
    }
    if record.namespace.parent.is_worktree() { source.deleted()?; } else { source.common()?; }
    Ok(())
}

fn bind_parent(
    context: &crate::engine::PlanningContext<'_>,
    record: &FileContainerRecord,
    id: DiskId,
    mode: Observation<'_>,
    held: &mut HeldPrefixes,
) -> Result<HeldParent> {
    if !matches!(mode, Observation::Prepare) {
        return bind_read_parent(
            context.config, context.disk_resources, context.state, record, id, mode, held,
        );
    }
    let replica = record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
    let disk = configured_disk(context.config, id)?;
    let handle = context.disk_resources.get(disk)?;
    if id == record.owner_disk {
        require_owner(context, record, id)?;
        if !replica.prefixes.is_empty() {
            return Err(Failure::ContainerScope.into());
        }
        return Ok(if record.namespace.parent.is_worktree() {
            HeldParent::Deleted(Arc::clone(handle))
        } else { HeldParent::Common(Arc::clone(handle)) });
    }
    let source = context.prepare_backup_source_path(
        record.namespace.repository, record.namespace.branch, id,
    )?;
    source.require_context(context)?;
    prefix::bind_chain(&source, handle, &replica.prefixes, mode, held)
}

fn bind_read_parent(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    previous: &State,
    record: &FileContainerRecord,
    id: DiskId,
    mode: Observation<'_>,
    held: &mut HeldPrefixes,
) -> Result<HeldParent> {
    if matches!(mode, Observation::Prepare) {
        return Err(Failure::ContainerScope.into());
    }
    let replica = record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
    let disk = configured_disk(config, id)?;
    let handle = resources.get(disk)?;
    if matches!(mode, Observation::Published) && id != record.owner_disk {
        let source = crate::engine::fresh_layout::containers::published_backup_source(
            config, resources, previous, record, id,
        )?;
        return prefix::bind_published_chain(&source, handle, &replica.prefixes, held);
    }
    let owner = crate::engine::fresh_layout::containers::read_container_owner(
        config, resources, previous, record,
    )?;
    if id == record.owner_disk {
        let same = if record.namespace.parent.is_worktree() {
            std::ptr::eq(owner.deleted()?, handle.deleted_root()?)
        } else { std::ptr::eq(owner.common()?, handle.content_root()?) };
        if !replica.prefixes.is_empty() || !same {
            return Err(Failure::ContainerScope.into());
        }
        return Ok(if record.namespace.parent.is_worktree() {
            HeldParent::Deleted(Arc::clone(handle))
        } else { HeldParent::Common(Arc::clone(handle)) });
    }
    let source = crate::engine::fresh_layout::containers::read_backup_source(
        config, resources, previous, record, id,
    )?;
    source.require_source(previous, config, resources)?;
    prefix::bind_chain(&source, handle, &replica.prefixes, mode, held)
}

fn descriptor_count(
    state: &State,
    targets: Option<&std::collections::BTreeSet<(FileId, DiskId)>>,
    directories: &BTreeSet<(storage_contract::EntryId, DiskId)>,
) -> Result<usize> {
    let mut prefixes = std::collections::BTreeSet::new();
    let mut containers = 0usize;
    for (file, record) in state.file_containers.iter() {
        for (id, replica) in &record.replicas {
            if targets.is_some_and(|targets| !targets.contains(&(*file, *id))) {
                continue;
            }
            let count = if record.working.draft().is_some() { 2 } else { 1 };
            containers = containers.checked_add(count).ok_or(Failure::ContainerBudget)?;
            prefixes.extend(replica.prefixes.iter().map(|prefix| (*id, prefix.source_disk)));
        }
    }
    for (entry, disk) in directories {
        let replica = state.namespace_directories.get(&entry.to_string())
            .and_then(|record| record.replicas.get(disk)).ok_or(Failure::NamespaceUnmapped)?;
        prefixes.extend(replica.source_prefixes.iter().map(|prefix| (*disk, prefix.source_disk)));
    }
    let prefix_descriptors = prefixes.len().checked_mul(6).ok_or(Failure::ContainerBudget)?;
    containers.checked_add(prefix_descriptors)
        .and_then(|count| count.checked_add(directories.len()))
        .ok_or_else(|| Failure::ContainerBudget.into())
}

fn attach_directory_parent(
    config: &Config, resources: &DiskResources, material: &State,
    record: &FileContainerRecord, disk: DiskId, parent: HeldParent, mode: Observation<'_>,
    reservation: &Arc<crate::engine::PayloadProofReservation>, directories: &mut HeldDirectories,
) -> Result<HeldParent> {
    match &record.namespace.parent {
        ContainerParent::Root | ContainerParent::Worktree { .. } => Ok(parent),
        ContainerParent::Directory(entry) => {
            let logical = material.namespace_directories.get(&entry.to_string())
                .ok_or(Failure::NamespaceUnmapped)?;
            if logical.owner_disk != record.owner_disk
                || logical.repository != record.namespace.repository
                || logical.branch != record.namespace.branch
            {
                return Err(Failure::NamespaceSource.into());
            }
            let handle = resources.get(configured_disk(config, disk)?)?;
            directories::attach(config, material, *entry, disk, handle, parent, mode,
                reservation, directories)
        }
    }
}

/// Read-only ordinary directory observation used by the shared snapshot namespace
/// index. No snapshot reservation can replace this original Mkdir occurrence.
pub(crate) fn verify_namespace_directory(
    config: &Config, resources: &DiskResources, state: &State,
    entry: storage_contract::EntryId, disk: DiskId,
) -> Result<()> {
    let chain = crate::engine::fresh_layout::directories::chain(state, entry)?;
    let record = *chain.first().ok_or(Failure::NamespaceUnmapped)?;
    let prefixes = if disk == record.owner_disk {
        0
    } else {
        record.replicas.get(&disk).ok_or(Failure::NamespaceUnmapped)?.source_prefixes.len()
    };
    let count = prefixes.checked_mul(6).and_then(|count| count.checked_add(chain.len()))
        .ok_or(Failure::ContainerBudget)?;
    let reservation = Arc::new(crate::engine::payload_proof_budget().reserve(count)
        .ok_or(Failure::ContainerBudget)?);
    directories::bind(config, resources, state, state, entry, disk,
        Observation::Published, &reservation, &mut HeldDirectories::new(),
        &mut HeldPrefixes::new())?.root()?;
    Ok(())
}

/// Retain the actual predecessor material on the same one-owner occurrence.
/// Every existing payload/name verification therefore keeps it alive through
/// the native decision. Available-only discovery cannot manufacture a full set.
fn retain_first_sources<'a>(
    held: &mut BTreeMap<(FileId, DiskId), ObservedContainerReplica>,
    first: impl Iterator<
        Item = &'a Arc<crate::engine::fresh_layout::containers::VerifiedUnbornFirst>,
    >,
) -> Result<()> {
    for first in first {
        let scope = held.get_mut(&(first.file(), first.owner()))
            .ok_or(Failure::ContainerScope)?;
        first.require_record(scope.file, &scope.record)?;
        scope.first = Some(Arc::clone(first));
    }
    Ok(())
}
