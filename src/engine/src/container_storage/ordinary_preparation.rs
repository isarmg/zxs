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
mod areas;
mod draft;
use prefix::{HeldParent, HeldPrefixes, Observation};

pub(crate) struct ObservedContainerReplica {
    file: FileId,
    disk: Arc<DiskHandle>,
    record: FileContainerRecord,
    parent: HeldParent,
    held: Arc<DiskRoot>,
    entity: ContainerEntity,
    draft: Option<draft::HeldDraft>,
    // Initial publication and actual recovery may observe the same inode at
    // either end of the one decided rename. A published-only reader cannot.
    installed: OnceLock<()>,
    // Every shared descriptor keeps the one actual admission charged until its
    // last consumer closes. This Arc shares accounting, not a second reservation.
    _reservation: Arc<crate::engine::PayloadProofReservation>,
}
impl ObservedContainerReplica {
    pub(crate) fn observe(
        source: crate::engine::fresh_layout::containers::PublishedReplicaSource<'_>,
    ) -> Result<Self> {
        use crate::engine::fresh_layout::containers::ReplicaNamespace;
        let (file, record, disk, namespace) = source.into_parts();
        let id = disk.seal.disk_id.parse::<DiskId>()?;
        let replica = record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
        let container_descriptors = if record.working.draft().is_some() { 2 } else { 1 };
        let count = replica.prefixes.len().checked_mul(6)
            .and_then(|count| count.checked_add(container_descriptors))
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
            ReplicaNamespace::Backup(source) => {
                prefix::published_parent(&source, disk, &replica.prefixes)?
            }
        };
        let held = open_published_container(
            parent.root()?, &record.namespace.name, replica.entity,
        )?;
        let installed = OnceLock::new();
        let _ = installed.set(());
        let mut result = Self {
            file, _reservation: reservation,
            disk: Arc::clone(disk), record: record.clone(), parent,
            held: Arc::new(held), entity: replica.entity, draft: None, installed,
        };
        if let Some((_, payload)) = record.working.draft() {
            result.draft = Some(draft::HeldDraft::capture(&result, *payload)?);
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
        &self.record.namespace.name
    }

    pub(crate) fn body_path(&self, version: &FileVersion) -> Result<PathBuf> {
        self.require_version(version)?;
        ordinary_body_path(self.held.path(), version, self.name())
    }

    fn identity(&self) -> FileIdentity {
        FileIdentity { device: self.entity.device(), inode: self.entity.inode() }
    }

    fn named_path(&self) -> Result<PathBuf> {
        Ok(self.parent.root()?.path()
            .join(layout::namespace::directory(&self.record.namespace.name).as_path()))
    }

    fn preparation_path(&self) -> Result<PathBuf> {
        let id = self.disk.seal.disk_id.parse::<DiskId>()?;
        let replica = self.record.replicas.get(&id).ok_or(Failure::ContainerScope)?;
        Ok(self.disk.management_root()?.path()
            .join(layout::container::preparation(replica.preparation).as_path()))
    }

    fn verify_entity(&self) -> Result<()> {
        let (root, path) = if self.installed.get().is_some() {
            (self.parent.root()?, self.named_path()?)
        } else {
            (self.disk.management_root()?, self.preparation_path()?)
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
        }
    }

    /// Only PreparedNames consumes this after the real Accept/Finalize gate.
    fn install(&self) -> Result<()> {
        self.verify()?;
        self.parent.install()?;
        if self.installed.get().is_some() {
            // A restart may observe the rename after a failed directory sync.
            // The actual decision installer must complete both durability tails.
            let target = self.parent.root()?;
            target.sync_dir(target.path())?;
            let source = self.disk.management_root()?;
            source.sync_dir(source.path())?;
            return Ok(());
        }
        let source = self.disk.management_root()?;
        let target = self.parent.root()?;
        let from = self.preparation_path()?;
        let to = self.named_path()?;
        // Never remove, exchange or infer ownership of a colliding user entry.
        if target.optional_metadata(&to)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        source.rename_to_held_synced(&from, target, &to)?;
        target.private_child_matching(&to, self.identity())?;
        self.installed.set(()).map_err(|_| Failure::ContainerIdentity)?;
        self.verify()
    }
}

pub(crate) struct PreparedContainerEntities {
    held: BTreeMap<(FileId, DiskId), Arc<ObservedContainerReplica>>,
    prefixes: HeldPrefixes,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
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
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(descriptor_count(plan.budget_state(), Some(plan.targets()))?)
            .ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
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
            let common = handle.content_root()?;
            if let Some((_, payload)) = record.working.draft() {
                version_names::check_target_name(common, &layout::container::DRAFTS)?;
                version_names::check_target_name(common,
                    &layout::container::draft_payload(payload.payload_id()))?;
            }
            version_names::check_target_name(
                common, &layout::namespace::directory(&record.namespace.name),
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
        for (file, id) in plan.targets() {
            crate::engine::fresh_layout::containers::require_current_container(
                plan.budget_state(), *file,
            )?;
            let record = &plan.budget_state().file_containers[file];
            let disk = configured_disk(context.config, *id)?;
            let handle = context.disk_resources.get(disk)?;
            let parent = bind_parent(context, record, *id, Observation::Prepare, &mut prefixes)?;
            let target = parent.root()?;
            let replica = record.replicas.get(id).ok_or(Failure::ContainerScope)?;
            let installed = OnceLock::new();
            let (directory, entity) = if plan.additions().contains_key(&(*file, *id)) {
                let named = target.path()
                    .join(layout::namespace::directory(&record.namespace.name).as_path());
                if target.optional_metadata(&named)?.is_some() {
                    return Err(Failure::ContainerIdentity.into());
                }
                let (directory, identity) = prefix::create_exclusive(handle.management_root()?,
                    &layout::container::preparation(replica.preparation))?;
                directory.private_dir(&directory.path().join(layout::VERSIONS.as_path()))?;
                directory.sync_dir(directory.path())?;
                (directory, ContainerEntity::new(identity.device, identity.inode))
            } else {
                let directory = open_published_container(target, &record.namespace.name,
                    replica.entity)?;
                let _ = installed.set(());
                (directory, replica.entity)
            };
            held.insert((*file, *id), ObservedContainerReplica {
                file: *file, _reservation: Arc::clone(&reservation),
                disk: Arc::clone(handle), record: record.clone(), parent,
                held: Arc::new(directory), entity, draft: None, installed,
            });
        }
        draft::populate(&mut held, input, context.config.max_file_bytes)?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, config: context.config.clone(),
            resources: Arc::clone(context.disk_resources), _reservation: reservation,
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
            CandidateAvailability::Required,
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
            CandidateAvailability::Available,
        )?;
        Ok(AvailableCandidateContainers { observed })
    }

    fn observe_candidate_inner(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        previous: &State,
        previous_snapshot: &Option<String>,
        candidate: &crate::journal::Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        availability: CandidateAvailability,
    ) -> Result<Self> {
        let next = original.for_candidate(candidate)?.state();
        if &candidate.previous_snapshot != previous_snapshot
            || next.sequence != candidate.sequence
        {
            return Err(Failure::ContainerScope.into());
        }
        crate::engine::fresh_layout::containers::verify_transition(
            config, previous, next, candidate,
        )?;
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
        let count = descriptor_count(next, targets.as_ref())?;
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(count).ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
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
                let common = parent.root()?;
                let management = handle.management_root()?;
                let named = common.path()
                    .join(layout::namespace::directory(&record.namespace.name).as_path());
                let stage = management.path()
                    .join(layout::container::preparation(replica.preparation).as_path());
                let identity = FileIdentity {
                    device: replica.entity.device(), inode: replica.entity.inode(),
                };
                let installed = OnceLock::new();
                let directory = if previous.file_containers.get(file)
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
                    disk: Arc::clone(handle), record: record.clone(), parent,
                    held: Arc::new(directory), entity: replica.entity, draft: None, installed,
                });
            }
        }
        draft::populate(&mut held, None, config.max_file_bytes)?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, config: config.clone(),
            resources: Arc::clone(resources),
            _reservation: reservation,
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
        let count = descriptor_count(state, None)?;
        let reservation = Arc::new(crate::engine::payload_proof_budget()
            .reserve(count).ok_or(Failure::ContainerBudget)?);
        let mut held = BTreeMap::new();
        let mut prefixes = HeldPrefixes::new();
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
                let (_, _, observed_handle, namespace) = source.into_parts();
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
                    crate::engine::fresh_layout::containers::ReplicaNamespace::Backup(source) => {
                        prefix::bind_chain(&source, handle, &replica.prefixes,
                            Observation::Published, &mut prefixes)?
                    }
                };
                let directory = open_published_container(parent.root()?, &record.namespace.name,
                    replica.entity)?;
                let installed = OnceLock::new();
                let _ = installed.set(());
                held.insert((*file, *id), ObservedContainerReplica {
                    file: *file, _reservation: Arc::clone(&reservation),
                    disk: Arc::clone(handle), record: record.clone(), parent,
                    held: Arc::new(directory), entity: replica.entity, draft: None, installed,
                });
            }
        }
        draft::populate(&mut held, None, config.max_file_bytes)?;
        let result = Self {
            held: held.into_iter().map(|(key, value)| (key, Arc::new(value))).collect(),
            prefixes, config: config.clone(), resources: Arc::clone(resources),
            _reservation: reservation,
        };
        result.verify()?;
        Ok(result)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        crate::engine::fresh_layout::verify_available(&self.config, &self.resources)?;
        for prefix in self.prefixes.values() {
            prefix.verify()?;
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
        })
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
            .join(layout::namespace::directory(&container.record.namespace.name).as_path()))
    }

    /// The producer supplies the identity attached to the preceding installed
    /// BODY descriptor, not an inferred source parsed from a prefix string.
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
        container.draft.as_ref().ok_or(Failure::ContainerScope)?.open(container)
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

    pub(super) fn install_decided(&self) -> Result<()> {
        self.verify()?;
        for container in self.held.values() {
            container.install()?;
        }
        self.verify()
    }
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
    source.common()?;
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
        return Ok(HeldParent::Common(Arc::clone(handle)));
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
    let owner = crate::engine::fresh_layout::containers::read_container_owner(
        config, resources, previous, record,
    )?;
    if id == record.owner_disk {
        if !replica.prefixes.is_empty() || !std::ptr::eq(owner.common()?, handle.content_root()?) {
            return Err(Failure::ContainerScope.into());
        }
        return Ok(HeldParent::Common(Arc::clone(handle)));
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
    let prefix_descriptors = prefixes.len().checked_mul(6).ok_or(Failure::ContainerBudget)?;
    containers.checked_add(prefix_descriptors).ok_or_else(|| Failure::ContainerBudget.into())
}
