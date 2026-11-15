//! Read-only automatic backup geometry from one admitted namespace occurrence.
//! Held common roots do not authorize copying a BODY, creating a prefix, or
//! installing a FILE. Those consumers retain their own operation/entity gates.
use super::{Failure, RegisteredNamespaceRoot, registered_root};
use crate::disk::{DiskHandle, DiskResources, DiskRoot, decode_seal, validate_stored_seal};
use crate::disk_names::configuration::{ConfiguredDiskNames, ConfiguredSourcePath};
use crate::engine::PlanningContext;
use crate::{Config, DiskConfig, Error, Result, State};
use std::sync::Arc;
use storage_contract::{BranchId, DiskId, RepositoryId};
use storage_contract::working_tree::{DiskRegistrationIdentity, RootBinding};

pub(crate) struct PreparedBackupSourcePath<'a> {
    source: RegisteredNamespaceRoot<'a>,
    target: &'a DiskConfig,
    target_binding: &'a RootBinding,
    direct: &'a DiskConfig,
    direct_binding: &'a RootBinding,
    target_handle: &'a Arc<DiskHandle>,
    direct_handle: &'a Arc<DiskHandle>,
    destination: ConfiguredSourcePath<'a>,
    origin: ConfiguredSourcePath<'a>,
}

impl<'a> PreparedBackupSourcePath<'a> {
    // The only constructor consumes the actual registered namespace source.
    // A decoded Config, named path or caller's manual prefixes cannot mint it.
    fn observe(source: RegisteredNamespaceRoot<'a>, target: DiskId) -> Result<Self> {
        let config = source.config;
        let names = ConfiguredDiskNames::new(config).map_err(Error::ConfiguredDiskName)?;
        let destination = names.source_path(target).map_err(Error::ConfiguredDiskName)?;
        let owner = source.disk.disk_id.parse::<DiskId>()?;
        let chain = destination.source_registrations().collect::<Vec<_>>();
        if chain.last() != Some(&source.binding().registration) || target == owner {
            return Err(Failure::NamespaceSource.into());
        }
        let direct_id = destination.direct_source().ok_or(Failure::NamespaceSource)?;
        let target_disk = definition(config, target)?;
        let direct = definition(config, direct_id)?;
        let target_binding = config.storage_roots.get(&target).ok_or(Failure::Registration)?;
        let direct_binding = config.storage_roots.get(&direct_id)
            .ok_or(Failure::Registration)?;
        let origin = names.source_path(direct_id).map_err(Error::ConfiguredDiskName)?;
        if target_binding.registration != destination.target_registration()
            || target_binding.source_chain != chain
            || direct_binding.registration != origin.target_registration()
            || direct_binding.source_chain != origin.source_registrations().collect::<Vec<_>>()
            || chain.first() != Some(&direct_binding.registration)
            || chain[1..] != direct_binding.source_chain[..]
        {
            return Err(Failure::Registration.into());
        }
        // All selected ancestors retain their actual registration and root.
        // Neither siblings nor unrelated inactive definitions are opened.
        for registration in &chain {
            let disk = definition(config, registration.disk_id.parse()?)?;
            let binding = config.storage_roots.get(&registration.disk_id.parse::<DiskId>()?)
                .ok_or(Failure::Registration)?;
            if &binding.registration != registration {
                return Err(Failure::Registration.into());
            }
            verify_held(config, disk, source.resources.get(disk)?, binding)?;
        }
        let target_handle = source.resources.get(target_disk)?;
        let direct_handle = source.resources.get(direct)?;
        verify_held(config, target_disk, target_handle, target_binding)?;
        Ok(Self {
            source,
            target: target_disk,
            target_binding,
            direct,
            direct_binding,
            target_handle,
            direct_handle,
            destination,
            origin,
        })
    }

    pub(crate) fn require_source(
        &self,
        state: &State,
        config: &Config,
        resources: &DiskResources,
    ) -> Result<()> {
        if !std::ptr::eq(self.source._state, state)
            || !std::ptr::eq(self.source.config, config)
            || !std::ptr::eq(self.source.resources, resources)
        {
            return Err(Failure::NamespaceSource.into());
        }
        self.source.common()?;
        verify_held(config, self.direct, self.direct_handle, self.direct_binding)?;
        verify_held(config, self.target, self.target_handle, self.target_binding)
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.require_source(context.state, context.config, context.disk_resources)
    }

    pub(crate) fn target_registration(&self) -> &DiskRegistrationIdentity {
        &self.target_binding.registration
    }

    pub(crate) fn target_binding(&self) -> &RootBinding {
        self.target_binding
    }

    pub(crate) fn direct_source_registration(&self) -> &DiskRegistrationIdentity {
        &self.direct_binding.registration
    }

    /// Immediate upstream first. C stores B/A, never A directly or a sibling.
    pub(crate) fn destination(&self) -> &ConfiguredSourcePath<'a> {
        &self.destination
    }

    /// The same content's prefix within the immediate source: empty on A,
    /// A's name/UUID on B. This is not a source BODY completeness claim.
    pub(crate) fn origin(&self) -> &ConfiguredSourcePath<'a> {
        &self.origin
    }

    pub(crate) fn target_common(&self) -> Result<&DiskRoot> {
        verify_held(self.source.config, self.target, self.target_handle, self.target_binding)?;
        self.target_handle.content_root()
    }

    pub(crate) fn direct_source_common(&self) -> Result<&DiskRoot> {
        verify_held(self.source.config, self.direct, self.direct_handle, self.direct_binding)?;
        self.direct_handle.content_root()
    }

    /// Call only for an actual introduction. Existing prefix observations must
    /// not retroactively charge their components as newly created names.
    pub(crate) fn verify_new_component_budget(&self) -> Result<()> {
        let maximum = self.target_common()?.component_name_maximum()?;
        self.destination.verify_component_budget(maximum).map_err(Error::ConfiguredDiskName)
    }
}

fn definition(config: &Config, id: DiskId) -> Result<&DiskConfig> {
    config.disks.iter().find(|disk| disk.disk_id == id.to_string())
        .ok_or_else(|| Failure::Registration.into())
}

fn verify_held(
    config: &Config,
    disk: &DiskConfig,
    handle: &DiskHandle,
    binding: &RootBinding,
) -> Result<()> {
    super::super::verify_bound_root(config, disk, &handle.root)?;
    let seal = decode_seal(&handle.root.read_bytes(
        &disk.root.join(crate::persist::layout::DISK_IDENTITY.as_path()),
        crate::disk::MAX_METADATA,
    )?)?;
    validate_stored_seal(&seal, &handle.seal)?;
    if binding.registration.pool_id != seal.pool_id
        || binding.registration.disk_id != seal.disk_id
        || binding.registration.slot_id != seal.slot_id
        || binding.registration.binding_generation != seal.binding_generation
    {
        return Err(Failure::RootIdentity.into());
    }
    let common = handle.content_root()?;
    if common.directory_identity(common.path())?
        != crate::working_tree::records::file_identity(binding.roots.common_data.entity)
    {
        return Err(Failure::RootIdentity.into());
    }
    Ok(())
}

impl PlanningContext<'_> {
    /// A source observation for a bound publication replay. It does not test
    /// write availability or allow preparation, creation or installation.
    pub(crate) fn observe_backup_source_path(
        &self,
        repository: RepositoryId,
        branch: BranchId,
        target: DiskId,
    ) -> Result<PreparedBackupSourcePath<'_>> {
        PreparedBackupSourcePath::observe(self.namespace_host_root(repository, branch)?, target)
    }

    pub(crate) fn prepare_backup_source_path(
        &self,
        repository: RepositoryId,
        branch: BranchId,
        target: DiskId,
    ) -> Result<PreparedBackupSourcePath<'_>> {
        self.require_write()?;
        if self.current_namespaces.is_none() && self.config.generation().is_some() {
            return Err(Failure::NamespaceSource.into());
        }
        let names = ConfiguredDiskNames::new(self.config).map_err(Error::ConfiguredDiskName)?;
        let path = names.source_path(target).map_err(Error::ConfiguredDiskName)?;
        if let Some(current) = self.current_namespaces {
            for identity in std::iter::once(path.target_registration())
                .chain(path.source_registrations())
            {
                current.require_write_slot(&identity.slot_id)?;
            }
        }
        PreparedBackupSourcePath::observe(self.namespace_host_root(repository, branch)?, target)
    }
}

/// Existing cold replica admission calls this through the same namespace
/// family. It performs no write check and returns no publication permission.
pub(in crate::engine::fresh_layout) fn read_backup_source_path<'a>(
    state: &'a State,
    config: &'a Config,
    resources: &'a DiskResources,
    repository: RepositoryId,
    branch: BranchId,
    target: DiskId,
) -> Result<PreparedBackupSourcePath<'a>> {
    PreparedBackupSourcePath::observe(
        registered_root(state, config, resources, repository, branch)?, target,
    )
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/disk_names/backup_source_tests.rs"]
mod tests;
