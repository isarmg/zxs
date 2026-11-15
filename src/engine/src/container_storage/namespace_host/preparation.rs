//! Exclusive preparation owns its actual descriptors until publication decides.
//! Existing material needs an independent completed registration; EXCL failure
//! never becomes a same-operation retry by comparing an inode or local marker.
use crate::container_storage::namespace_host::{
    Component, NamespaceHost, NamespaceHostError, NamespaceOrigin, require_component, verify_chain,
};
use crate::disk::{DirectoryCreateOutcome, DiskResources, DiskRoot};
use crate::name_budget::NameBudget;
use crate::persist::snapshot_worktree::{HostEvidence, NamespaceDirectoryBinding};
use crate::snapshot_worktree::{DirectoryRegistration, RegisteredNamespaceDirectories};
use crate::working_tree::records::{file_identity, local_entity};
use crate::{BranchState, Engine, NamespaceSnapshot, State, ValidatedConfig};
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::snapshot_worktree::{SnapshotHost, SnapshotNamespace};
use storage_contract::working_tree::{BindingLifecycle, DirectoryBinding, DirectoryId, RootId};
use storage_contract::{DiskId, Generation, OperationId};

mod snapshot;
pub(crate) use snapshot::PreparedNamespaceNode;

#[derive(Debug, error_definition::Error)]
pub(crate) enum NamespacePreparationError {
    #[error("ordinary namespace preparation was rejected before directory creation")]
    Rejected(#[source] NamespaceHostError),
    #[error("ordinary namespace preparation {operation} may retain unpublished directories")]
    Incomplete {
        operation: OperationId,
        #[source]
        source: NamespaceHostError,
    },
}

enum PlannedDirectory {
    Registered(NamespaceDirectoryBinding),
    Create {
        component: Component,
        directory: DirectoryId,
        parent: DirectoryId,
    },
}

/// No raw descriptor or arbitrary mutation is exposed. Only one successful
/// complete preparation constructs this object; a rejected retry cannot rebuild
/// it from path existence. Dropping it preserves unregistered material.
pub(crate) struct PreparedNamespaceHost {
    occurrence: Arc<()>,
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    current: Option<Arc<crate::engine::CurrentNamespaces>>,
    snapshot: NamespaceSnapshot,
    source_request: SnapshotNamespace,
    operation: OperationId,
    root: RootId,
    generation: Generation,
    disk: DiskId,
    evidence: HostEvidence,
    directories: Vec<DiskRoot>,
}

pub(crate) struct NamespacePreparationObservation {
    occurrence: Arc<()>,
}

impl NamespacePreparationObservation {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.occurrence, &other.occurrence)
    }
}

impl NamespaceHost<'_> {
    fn prepare(
        self,
        registered: &RegisteredNamespaceDirectories<'_>,
        root: RootId,
        generation: Generation,
        operation: OperationId,
    ) -> Result<PreparedNamespaceHost, NamespacePreparationError> {
        let reject = NamespacePreparationError::Rejected;
        let NamespaceOrigin::Request(source_request) = &self.origin else {
            return Err(reject(NamespaceHostError::Changed));
        };
        self.engine.require_write().map_err(NamespaceHostError::Source).map_err(reject)?;
        if !std::ptr::eq(registered.source(), self.engine.state())
            || self.engine.state().operations.contains_key(&operation)
            || source_request.branch_id != self.address.branch_id
        {
            return Err(reject(NamespaceHostError::Allocated));
        }
        require_namespace_write(self.engine, self.address.branch_id, self.root.disk())
            .map_err(reject)?;
        let components = self.components().map_err(reject)?;
        let mut parent = self.root.binding().roots.common_data.directory_id;
        let mut identities = self.engine.config().storage_roots.values().flat_map(|root| [
            root.roots.common_data.directory_id,
            root.roots.deleted_data.directory_id,
            root.roots.zxs_data.directory_id,
            root.roots.zxs_lifecycles.common_data.directory_id,
            root.roots.zxs_lifecycles.deleted_data.directory_id,
        ]).collect::<BTreeSet<_>>();
        let mut plan = Vec::with_capacity(components.len());
        for component in components {
            match registered.get(root, self.address.repository_id, component.entry) {
                Some(DirectoryRegistration::Published(binding)) => {
                    require_component(root, parent, &component, binding).map_err(reject)?;
                    parent = binding.directory.directory_id;
                    if !identities.insert(parent) {
                        return Err(reject(NamespaceHostError::Allocated));
                    }
                    plan.push(PlannedDirectory::Registered(binding.clone()));
                }
                Some(DirectoryRegistration::Pending(operation)) => {
                    return Err(reject(NamespaceHostError::Pending(operation)));
                }
                Some(DirectoryRegistration::Conflict) => {
                    return Err(reject(NamespaceHostError::Changed));
                }
                None => {
                    let directory = DirectoryId::generate()
                        .map_err(NamespaceHostError::Identity).map_err(reject)?;
                    if registered.contains_id(directory) || !identities.insert(directory) {
                        return Err(reject(NamespaceHostError::Allocated));
                    }
                    plan.push(PlannedDirectory::Create { component, directory, parent });
                    parent = directory;
                }
            }
        }
        let common = self.held_root(root, generation).map_err(reject)?;
        // Only introduced physical components use today's root limit. Existing
        // unchanged bindings are not re-admitted against a new naming policy.
        if plan.iter().any(|step| matches!(step, PlannedDirectory::Create { .. })) {
            if common.available().map_err(NamespaceHostError::Source).map_err(reject)?
                < self.engine.config().metadata_reserve_bytes
            {
                return Err(reject(NamespaceHostError::Reserve));
            }
            let maximum = common.component_name_maximum()
                .map_err(NamespaceHostError::Source).map_err(reject)?;
            let budget = NameBudget::from_maximum(maximum)
                .map_err(NamespaceHostError::NameBudget).map_err(reject)?;
            for step in &plan {
                if let PlannedDirectory::Create { component, .. } = step {
                    budget.component(component.name.as_str(), component.name.as_str())
                        .map_err(NamespaceHostError::NameBudget).map_err(reject)?;
                }
            }
        }
        let mut prepared = PreparedNamespaceHost {
            occurrence: Arc::new(()),
            state: Arc::clone(&self.engine.state),
            config: self.engine.config.clone(),
            resources: Arc::clone(&self.engine.disk_resources),
            current: self.engine.current_namespaces.clone(),
            snapshot: self.snapshot.clone(),
            source_request: source_request.clone(),
            operation,
            root,
            generation,
            disk: self.root.disk().disk_id.parse()
                .map_err(NamespaceHostError::Identity).map_err(reject)?,
            evidence: HostEvidence {
                namespace: self.address,
                registration: self.root.binding().registration.clone(),
                root_entity: self.root.binding().root_entity,
                common_data: self.root.binding().roots.common_data.clone(),
                directories: Vec::with_capacity(plan.len()),
            },
            directories: Vec::with_capacity(plan.len()),
        };
        prepared.materialize(common, plan).map_err(|source| {
            NamespacePreparationError::Incomplete { operation, source }
        })?;
        prepared.require_engine(self.engine).map_err(|source| {
            NamespacePreparationError::Incomplete { operation, source }
        })?;
        Ok(prepared)
    }
}

impl PreparedNamespaceHost {
    pub(crate) fn namespace_snapshot<'a>(
        &'a self,
        engine: &Engine,
    ) -> Result<&'a NamespaceSnapshot, NamespaceHostError> {
        self.require_engine(engine)?;
        Ok(&self.snapshot)
    }

    pub(crate) fn observation(&self) -> NamespacePreparationObservation {
        NamespacePreparationObservation { occurrence: Arc::clone(&self.occurrence) }
    }

    pub(crate) fn require_request(
        &self,
        request: &storage_contract::snapshot_worktree::InitializeSnapshot,
    ) -> Result<(), NamespaceHostError> {
        if request.operation_id != self.operation || request.source != self.source_request {
            return Err(NamespaceHostError::Changed);
        }
        Ok(())
    }

    /// The one live entrance retains exclusive Engine access throughout all
    /// filesystem effects. A shared namespace reader cannot invoke preparation.
    pub(crate) fn prepare(
        engine: &mut Engine,
        registered: &RegisteredNamespaceDirectories<'_>,
        source: &SnapshotNamespace,
        root: RootId,
        generation: Generation,
        operation: OperationId,
    ) -> Result<Self, NamespacePreparationError> {
        NamespaceHost::for_request(engine, source)
            .map_err(NamespacePreparationError::Rejected)?
            .prepare(registered, root, generation, operation)
    }

    fn materialize(
        &mut self,
        common: &DiskRoot,
        plan: Vec<PlannedDirectory>,
    ) -> Result<(), NamespaceHostError> {
        for step in plan {
            let parent = self.directories.last().unwrap_or(common);
            let (binding, directory) = match step {
                PlannedDirectory::Registered(binding) => {
                    let name = crate::persist::layout::namespace::directory(
                        &binding.directory.name,
                    );
                    let directory = parent.private_entry_matching(
                        &name, file_identity(binding.directory.entity),
                    ).map_err(NamespaceHostError::Physical)?;
                    (binding, directory)
                }
                PlannedDirectory::Create { component, directory: id, parent: parent_id } => {
                    let name = crate::persist::layout::namespace::directory(&component.name);
                    let (directory, identity) = match parent.create_private_entry(&name) {
                        DirectoryCreateOutcome::Rejected(source)
                        | DirectoryCreateOutcome::Uncertain { source, .. } => {
                            return Err(NamespaceHostError::Physical(source));
                        }
                        DirectoryCreateOutcome::Created { directory, identity, durability } => {
                            durability.map_err(NamespaceHostError::Physical)?;
                            if directory.held_identity().map_err(NamespaceHostError::Physical)?
                                != identity
                            {
                                return Err(NamespaceHostError::Changed);
                            }
                            parent.sync_private_entry_matching(&name, identity)
                                .map_err(NamespaceHostError::Physical)?;
                            (directory, identity)
                        }
                    };
                    (NamespaceDirectoryBinding {
                        entry: component.entry,
                        directory: DirectoryBinding {
                            directory_id: id,
                            root_id: self.root,
                            parent_directory_id: parent_id,
                            name: component.name,
                            entity: local_entity(identity),
                            lifecycle: BindingLifecycle::Current,
                            operation_id: self.operation,
                        },
                    }, directory)
                }
            };
            self.evidence.directories.push(binding);
            self.directories.push(directory);
        }
        verify_chain(common, &self.evidence.directories, &self.directories)?;
        self.verify_created_inventories()
    }

    pub(crate) fn require_engine(&self, engine: &Engine) -> Result<(), NamespaceHostError> {
        let current_matches = match (&self.current, &engine.current_namespaces) {
            (None, None) => true,
            (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
            (None, Some(_)) | (Some(_), None) => false,
        };
        if !Arc::ptr_eq(&self.state, &engine.state) || !current_matches
            || !self.config.same_admission(&engine.config)
            || !Arc::ptr_eq(&self.resources, &engine.disk_resources)
            || !engine.namespace_snapshot_matches(&self.snapshot.witness())
        {
            return Err(NamespaceHostError::Changed);
        }
        engine.require_write().map_err(NamespaceHostError::Source)?;
        let namespace = NamespaceHost::for_request(engine, &self.source_request)?;
        let disk: DiskId = namespace.root.disk().disk_id.parse()
            .map_err(NamespaceHostError::Identity)?;
        if namespace.address != self.evidence.namespace || disk != self.disk {
            return Err(NamespaceHostError::Changed);
        }
        require_namespace_write(engine, namespace.address.branch_id, namespace.root.disk())?;
        let common = namespace.held_root(self.root, self.generation)?;
        verify_chain(
            common,
            &self.evidence.directories, &self.directories,
        )?;
        self.verify_created_inventories()
    }

    fn verify_created_inventories(&self) -> Result<(), NamespaceHostError> {
        for (index, (record, held)) in self.evidence.directories.iter()
            .zip(&self.directories).enumerate()
        {
            if record.directory.operation_id != self.operation {
                continue;
            }
            let mut expected = self.evidence.directories.get(index + 1).map(|child| {
                crate::persist::layout::namespace::directory(&child.directory.name)
            });
            for name in held.root_entries().map_err(NamespaceHostError::Physical)? {
                let name = name.map_err(|source| {
                    NamespaceHostError::Physical(crate::disk::DirectoryIoError::Filesystem(source))
                })?;
                if expected.take().as_ref() != Some(&name) {
                    return Err(NamespaceHostError::Changed);
                }
            }
            if expected.is_some() {
                return Err(NamespaceHostError::Changed);
            }
        }
        Ok(())
    }

    /// The live permanent publisher must consume this same prepared object and
    /// recheck it at its decision point. These bytes alone grant no later I/O.
    pub(crate) fn reservation_evidence(
        &self,
        engine: &Engine,
    ) -> Result<(SnapshotHost, HostEvidence), NamespaceHostError> {
        self.require_engine(engine)?;
        Ok((SnapshotHost {
            root_id: self.root,
            directory_id: self.evidence.host().directory_id,
            expected_root_generation: self.generation,
        }, self.evidence.clone()))
    }
}

pub(super) fn require_namespace_write(
    engine: &Engine,
    branch: storage_contract::BranchId,
    disk: &crate::DiskConfig,
) -> Result<(), NamespaceHostError> {
    let branch = branch.to_string();
    if engine.state().branches.get(&branch)
        .is_none_or(|branch| branch.state != BranchState::Active)
    {
        return Err(NamespaceHostError::Main);
    }
    if let Some(current) = engine.admitted_current_namespaces()
        .map_err(NamespaceHostError::Source)?
    {
        current.require_branch_write(engine.state(), &branch).map_err(NamespaceHostError::Source)?;
        current.require_write_slot(&disk.slot_id).map_err(NamespaceHostError::Source)?;
    }
    engine.route(&disk.slot_id, 1).map_err(NamespaceHostError::Source)?;
    Ok(())
}
