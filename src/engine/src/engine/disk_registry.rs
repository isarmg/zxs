//! Current registration provenance, deliberately separate from physical health.
//!
//! The initial view comes from admitted configuration. After a lifecycle
//! publication, only the jointly installed verified registry decides current
//! registration; Config continues to describe historical storage identities.
use super::{Engine, ValidatedConfig};
use crate::DiskConfig;
use data_serialization::{Deserialize, Serialize};

pub(in crate::engine) mod current;
mod namespace_snapshot;
pub use namespace_snapshot::{
    DirectoryNamespaceView, NamespaceEntries, NamespaceSnapshot, NamespaceSnapshotWitness,
};
pub(in crate::engine) mod published;
pub use published::{
    DiskRegistryCommitment, HistoricalDiskDefinition, PreparedDiskRegistry, RegistrationMode,
    RegistryExitEffect, RegistryExitRequest, VerifiedDiskRegistry, VerifiedRegistryCandidate,
    VerifiedRegistryPublication,
};

/// Exact logical registration identity. A path is not a binding identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskRegistrationIdentity {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub binding_generation: u64,
}

/// Borrowed metadata only; neither the disk root nor any I/O capability escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredDisk<'a> {
    pub pool_id: &'a str,
    pub disk_id: &'a str,
    pub slot_id: &'a str,
    pub binding_generation: u64,
    pub level: u32,
    pub upstream_slot_id: Option<&'a str>,
    pub metadata_member: bool,
    pub registration_mode: RegistrationMode,
}
impl RegisteredDisk<'_> {
    pub fn identity(self) -> DiskRegistrationIdentity {
        DiskRegistrationIdentity {
            pool_id: self.pool_id.into(),
            disk_id: self.disk_id.into(),
            slot_id: self.slot_id.into(),
            binding_generation: self.binding_generation,
        }
    }
}

#[derive(Clone, Copy)]
enum RegistryView<'a> {
    Initial,
    AwaitingPublication,
    Published(&'a VerifiedDiskRegistry),
}

#[derive(Clone, Copy)]
pub(super) struct CurrentDiskRegistry<'a> {
    config: &'a ValidatedConfig,
    view: RegistryView<'a>,
}
impl<'a> CurrentDiskRegistry<'a> {
    pub(super) fn from_engine(engine: &'a Engine) -> Self {
        Self {
            config: &engine.config,
            view: match engine.current_disk_registry() {
                Some(registry) => RegistryView::Published(registry),
                None if engine
                    .config
                    .generation()
                    .is_some_and(|generation| generation.portable.exit_registry().is_some()) =>
                {
                    RegistryView::AwaitingPublication
                }
                None => RegistryView::Initial,
            },
        }
    }
    pub(in crate::engine) fn from_config(config: &'a ValidatedConfig) -> Self {
        Self {
            config,
            view: RegistryView::Initial,
        }
    }
    pub(in crate::engine) fn from_verified(
        config: &'a ValidatedConfig,
        registry: &'a VerifiedDiskRegistry,
    ) -> Self {
        Self {
            config,
            view: RegistryView::Published(registry),
        }
    }

    pub(super) fn historical_identity(self, slot: &str) -> Option<DiskRegistrationIdentity> {
        match self.view {
            RegistryView::Initial => self
                .config
                .disks
                .iter()
                .find(|disk| disk.slot_id == slot)
                .map(|disk| self.project(disk).identity()),
            RegistryView::Published(registry) => registry
                .historical_topology()
                .find(|disk| disk.identity.slot_id == slot)
                .map(|disk| disk.identity.clone()),
            RegistryView::AwaitingPublication => None,
        }
    }

    pub(super) fn generation_hash(self) -> Option<&'a str> {
        self.config.generation_hash()
    }
    pub(super) fn disks(self) -> Box<dyn ExactSizeIterator<Item = RegisteredDisk<'a>> + 'a> {
        match self.view {
            RegistryView::Initial => {
                Box::new(self.config.disks.iter().map(move |disk| self.project(disk)))
            }
            RegistryView::AwaitingPublication => Box::new(std::iter::empty()),
            RegistryView::Published(registry) => Box::new(registry.registered_definitions().map(
                |(disk, mode, voter)| RegisteredDisk {
                    pool_id: &disk.identity.pool_id,
                    disk_id: &disk.identity.disk_id,
                    slot_id: &disk.identity.slot_id,
                    binding_generation: disk.identity.binding_generation,
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.as_deref(),
                    metadata_member: voter,
                    registration_mode: mode,
                },
            )),
        }
    }
    pub(super) fn slot(self, slot: &str) -> Option<RegisteredDisk<'a>> {
        // Exact canonical placement identity, not a case-insensitive user lookup.
        self.disks().find(|disk| disk.slot_id == slot)
    }
    fn project(self, disk: &'a DiskConfig) -> RegisteredDisk<'a> {
        RegisteredDisk {
            pool_id: &self.config.pool_id,
            disk_id: &disk.disk_id,
            slot_id: &disk.slot_id,
            binding_generation: disk.binding_generation,
            level: disk.level,
            upstream_slot_id: disk.upstream_slot_id.as_deref(),
            metadata_member: disk.metadata_member,
            registration_mode: RegistrationMode::Enabled,
        }
    }
}

impl Engine {
    /// Current registrations, including Disabled but excluding Detached bindings.
    /// Historical Config/Placement records deliberately retain their identities.
    pub fn registered_disks(&self) -> impl ExactSizeIterator<Item = RegisteredDisk<'_>> {
        CurrentDiskRegistry::from_engine(self).disks()
    }

    pub fn registered_disk(&self, disk_id: &str) -> Option<RegisteredDisk<'_>> {
        self.registered_disks().find(|disk| disk.disk_id == disk_id)
    }

    pub fn current_disk_registry_commitment(&self) -> Option<&DiskRegistryCommitment> {
        self.current_namespaces.as_ref().map(|view| view.registry())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/disk_deregistration_plan_tests.rs"]
mod tests;
