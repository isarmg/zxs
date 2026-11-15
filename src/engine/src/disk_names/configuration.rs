//! Configured names and direct-source geometry, with no root or write authority.
//! The caller supplies its actual admitted genesis/Add configuration;
//! parsing this pure projection alone does not authenticate that configuration.
use crate::{Config, DiskConfig, ErrorCode, name_budget::NameBudget};
use name_policy::Name;
use std::collections::BTreeMap;
use storage_contract::{DiskId, PoolId, working_tree::DiskRegistrationIdentity};

#[derive(Debug, error_definition::Error)]
pub enum ConfiguredDiskNameError {
    #[error("disk-name configuration has an invalid identity: {source}")]
    Identity {
        #[source]
        source: storage_contract::Error,
    },
    #[error("disk-name configuration repeats disk {disk_id}")]
    DuplicateDisk { disk_id: DiskId },
    #[error("disk names for {first} and {second} have the same lookup name {name}")]
    DuplicateName {
        first: DiskId,
        second: DiskId,
        name: Name,
    },
    #[error("disk-name configuration has no target {disk_id}")]
    UnknownDisk { disk_id: DiskId },
    #[error("disk-name source topology is invalid: {source}")]
    Topology {
        #[source]
        source: Box<crate::Error>,
    },
    #[error("target {target} has an unusable source-name budget: {source}")]
    TargetBudget {
        target: DiskId,
        #[source]
        source: crate::name_budget::NameBudgetError,
    },
    #[error("source {disk_id} component exceeds target {target}'s name budget: {source}")]
    ComponentBudget {
        target: DiskId,
        disk_id: DiskId,
        #[source]
        source: crate::name_budget::NameBudgetError,
    },
}

impl ConfiguredDiskNameError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Identity { source } => source.code(),
            Self::DuplicateDisk { .. } => ErrorCode::ConfigInvalid,
            Self::DuplicateName { .. } => ErrorCode::NameConflict,
            Self::UnknownDisk { .. } => ErrorCode::DiskNotFound,
            Self::Topology { source } => source.code(),
            Self::TargetBudget { .. } | Self::ComponentBudget { .. } => ErrorCode::NameTooLong,
        }
    }
}

type Result<T> = std::result::Result<T, ConfiguredDiskNameError>;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct NameKey(String);

fn insert_name(
    names: &mut BTreeMap<NameKey, DiskId>,
    disk_id: DiskId,
    name: &Name,
) -> Result<()> {
    if let Some(first) = names.insert(NameKey(name.key()), disk_id) {
        return Err(ConfiguredDiskNameError::DuplicateName {
            first,
            second: disk_id,
            name: name.clone(),
        });
    }
    Ok(())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct SlotKey(String);

impl SlotKey {
    fn new(slot: &str) -> Self {
        Self(slot.to_ascii_lowercase())
    }
}

/// The exact borrowed configuration, not a caller-supplied prefixes table.
/// Names stay reserved for every definition in this source, including a disk
/// whose physical root is unavailable. No root is opened to make this index.
pub(crate) struct ConfiguredDiskNames<'a> {
    pool: PoolId,
    disks: BTreeMap<DiskId, &'a DiskConfig>,
    slots: BTreeMap<SlotKey, DiskId>,
    topology: crate::route_topology::Topology<'a>,
}

impl<'a> ConfiguredDiskNames<'a> {
    pub(crate) fn new(config: &'a Config) -> Result<Self> {
        let pool = config.pool_id.parse()
            .map_err(|source| ConfiguredDiskNameError::Identity { source })?;
        let topology = crate::route_topology::Topology::new(config.disks.iter().map(|disk| {
            crate::route_topology::Node {
                slot: &disk.slot_id,
                upstream: disk.upstream_slot_id.as_deref(),
                level: disk.level,
            }
        }))
        .map_err(|source| ConfiguredDiskNameError::Topology { source: Box::new(source) })?;
        let mut disks = BTreeMap::new();
        let mut slots = BTreeMap::new();
        let mut names = BTreeMap::new();
        for disk in &config.disks {
            let disk_id = disk.disk_id.parse()
                .map_err(|source| ConfiguredDiskNameError::Identity { source })?;
            if disks.insert(disk_id, disk).is_some() {
                return Err(ConfiguredDiskNameError::DuplicateDisk { disk_id });
            }
            insert_name(&mut names, disk_id, &disk.name)?;
            // Topology::new rejected duplicate canonical slots above.
            slots.insert(SlotKey::new(&disk.slot_id), disk_id);
        }
        Ok(Self {
            pool,
            disks,
            slots,
            topology,
        })
    }

    /// Prospective registration names are checked before retaining a task.
    /// Existing names stay reserved, including unavailable and replaced disks.
    pub(crate) fn require_new_names<'b>(
        &self,
        targets: impl IntoIterator<Item = (&'b str, &'b Name)>,
    ) -> Result<()> {
        let mut names = self.disks.iter()
            .map(|(id, disk)| (NameKey(disk.name.key()), *id))
            .collect();
        for (identity, name) in targets {
            let id = identity.parse()
                .map_err(|source| ConfiguredDiskNameError::Identity { source })?;
            insert_name(&mut names, id, name)?;
        }
        Ok(())
    }

    /// Only the selected target's real ancestors contribute components.
    /// A sibling never becomes a hop because it follows this disk in a vector.
    pub(crate) fn source_path(&self, target: DiskId) -> Result<ConfiguredSourcePath<'a>> {
        let definition = *self.disks.get(&target)
            .ok_or(ConfiguredDiskNameError::UnknownDisk { disk_id: target })?;
        let chain = self.topology.upstream_chain(&definition.slot_id).map_err(|source| {
            ConfiguredDiskNameError::Topology { source: Box::new(source) }
        })?;
        let mut sources = Vec::with_capacity(chain.len());
        let mut relative = String::new();
        for node in chain {
            let disk_id = self.slots[&SlotKey::new(node.slot)];
            let disk = self.disks[&disk_id];
            // The typed DiskId renderer owns the complete canonical UUID text.
            // Never recover an identity by parsing a directory component.
            let identity = disk_id.to_string();
            let component = format!("{}--{}", disk.name, &identity["disk_".len()..]);
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(&component);
            sources.push(ConfiguredSource {
                disk_id,
                definition: disk,
                component: component.into_boxed_str(),
            });
        }
        Ok(ConfiguredSourcePath {
            pool: self.pool,
            target,
            definition,
            sources,
            relative: relative.into_boxed_str(),
        })
    }
}

struct ConfiguredSource<'a> {
    disk_id: DiskId,
    definition: &'a DiskConfig,
    component: Box<str>,
}

/// Safe relative source components, not a physical locator or an authenticated
/// source. Primary roots have no source prefix. Generated components are not
/// user PortableNames: their UUID suffix is charged to the actual target budget.
pub(crate) struct ConfiguredSourcePath<'a> {
    pool: PoolId,
    target: DiskId,
    definition: &'a DiskConfig,
    sources: Vec<ConfiguredSource<'a>>,
    relative: Box<str>,
}

impl ConfiguredSourcePath<'_> {
    pub(crate) fn target(&self) -> DiskId {
        self.target
    }

    pub(crate) fn direct_source(&self) -> Option<DiskId> {
        self.sources.first().map(|source| source.disk_id)
    }

    /// Immediate source first, original primary last.
    pub(crate) fn source_ids(&self) -> impl ExactSizeIterator<Item = DiskId> + '_ {
        self.sources.iter().map(|source| source.disk_id)
    }

    pub(crate) fn target_registration(&self) -> DiskRegistrationIdentity {
        self.registration(self.target, self.definition)
    }

    pub(crate) fn source_registrations(
        &self,
    ) -> impl ExactSizeIterator<Item = DiskRegistrationIdentity> + '_ {
        self.sources.iter().map(|source| self.registration(source.disk_id, source.definition))
    }

    fn registration(&self, disk_id: DiskId, disk: &DiskConfig) -> DiskRegistrationIdentity {
        DiskRegistrationIdentity {
            pool_id: self.pool.to_string(),
            disk_id: disk_id.to_string(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        }
    }

    pub(crate) fn relative_prefix(&self) -> Option<&str> {
        if self.sources.is_empty() {
            None
        } else {
            Some(&self.relative)
        }
    }

    pub(crate) fn components(&self) -> impl ExactSizeIterator<Item = &str> + '_ {
        self.sources.iter().map(|source| source.component.as_ref())
    }

    /// The actual held-target consumer supplies its freshly read maximum.
    /// Success proves only these new components fit; it cannot substitute for
    /// that consumer's registration/entity/collision or complete object checks.
    pub(crate) fn verify_component_budget(&self, maximum: u64) -> Result<()> {
        if self.sources.is_empty() {
            return Ok(());
        }
        let budget = NameBudget::from_maximum(maximum).map_err(|source| {
            ConfiguredDiskNameError::TargetBudget { target: self.target, source }
        })?;
        for component in &self.sources {
            budget.component(&component.component, &self.relative).map_err(|source| {
                ConfiguredDiskNameError::ComponentBudget {
                    target: self.target,
                    disk_id: component.disk_id,
                    source,
                }
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk_names/configuration_tests.rs"]
mod tests;
