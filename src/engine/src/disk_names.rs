//! Read-only configured-name observations and prospective source-prefix geometry.
//! No result is a persistent name record, current locator, or rename capability.
use crate::{
    DiskRegistryCommitment, Engine, Error, ErrorCode, RegisteredDisk, RegistrationMode,
};
use data_serialization::Serialize;
use name_policy::{Name, SlotId};
use storage_contract::{DiskId, PoolId};

mod layout;
mod plan;

const MAX_DISKS: usize = 256;
const MAX_SLICES: usize = 4096;
const MAX_CHAIN_IDENTITIES: usize = 8192;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, error_definition::Error)]
pub enum DiskSourceNameError {
    #[error(transparent)]
    Engine(#[from] crate::Error),
    #[error("disk {disk_id} is not currently registered")]
    UnregisteredDisk { disk_id: DiskId },
    #[error("disk source-name preview exceeds its bounded complete-result budget")]
    ResourceLimit,
}

impl DiskSourceNameError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Engine(source) => source.code(),
            Self::UnregisteredDisk { .. } => ErrorCode::DiskNotFound,
            Self::ResourceLimit => ErrorCode::ResourceLimit,
        }
    }
}

type Result<T> = std::result::Result<T, DiskSourceNameError>;

fn limit() -> DiskSourceNameError {
    DiskSourceNameError::ResourceLimit
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskSourceIdentity {
    pub pool_id: PoolId,
    pub disk_id: DiskId,
    pub slot_id: SlotId,
    #[serialization(with = "storage_contract::u64_string")]
    pub binding_generation: u64,
}

#[derive(Debug, Serialize)]
pub struct DiskNameObservation {
    pub identity: DiskSourceIdentity,
    pub registration_mode: RegistrationMode,
    pub level: u32,
    pub upstream_slot_id: Option<SlotId>,
    pub observed_name: Option<Name>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub component_maximum: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskNameObservationSource {
    ConfiguredRootObservation,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskNamePreviewExecution {
    PreviewOnly,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskNamePreviewPhase {
    Observed,
    Proposed,
}

/// A prospective source prefix, not a path accepted by any storage writer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub struct DiskSourcePath(String);

impl DiskSourcePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Serialize)]
pub struct DiskSourceSlice {
    pub target: DiskId,
    pub source: DiskId,
    pub chain: Vec<DiskId>,
    pub before: Option<DiskSourcePath>,
    pub after: Option<DiskSourcePath>,
    pub affected: bool,
}

#[derive(Debug, Serialize)]
pub struct DiskSourceRenameRoot {
    pub target: DiskId,
    pub source: DiskId,
    pub before: Option<DiskSourcePath>,
    pub after: Option<DiskSourcePath>,
}

#[derive(Debug, Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub enum DiskNamePreviewBlocker {
    ConfigIdentityMismatch { disk_id: DiskId },
    NameUnavailable { disk_id: DiskId },
    InvalidName {
        disk_id: DiskId,
        #[serialization(skip)]
        source: name_policy::Error,
    },
    ResourceIdentityMismatch { disk_id: DiskId },
    BudgetUnavailable {
        disk_id: DiskId,
        code: ErrorCode,
        #[serialization(skip)]
        source: Box<Error>,
    },
    NameConflict {
        disk_id: DiskId,
        other_disk_id: DiskId,
        phase: DiskNamePreviewPhase,
    },
    MissingUpstream { target: DiskId, slot: SlotId },
    InvalidTopology { target: DiskId },
    ComponentBudget {
        target: DiskId,
        source: DiskId,
        phase: DiskNamePreviewPhase,
        bytes: usize,
        #[serialization(with = "storage_contract::u64_string")]
        maximum: u64,
    },
    ProjectedPathBudget {
        target: DiskId,
        source: DiskId,
        phase: DiskNamePreviewPhase,
        bytes: usize,
        components: usize,
    },
}

#[derive(Debug, Serialize)]
pub struct DiskSourceNamePreview {
    pub source: DiskNameObservationSource,
    pub execution: DiskNamePreviewExecution,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
    pub registry: Option<DiskRegistryCommitment>,
    pub requested_disk_id: DiskId,
    pub proposed_name: Name,
    pub disks: Vec<DiskNameObservation>,
    pub slices: Vec<DiskSourceSlice>,
    pub rename_roots: Vec<DiskSourceRenameRoot>,
    pub blockers: Vec<DiskNamePreviewBlocker>,
}

impl Engine {
    /// Observe every current registration, including Disabled, without opening
    /// new roots or modifying capture bytes, State, configuration, or directories.
    /// Configured basenames are explicitly observations, not authenticated names.
    pub fn preview_disk_source_names(
        &self,
        disk_id: DiskId,
        proposed_name: &Name,
    ) -> std::result::Result<DiskSourceNamePreview, DiskSourceNameError> {
        self.admitted_current_namespaces()?;
        let registered = self.registered_disks();
        if registered.len() > MAX_DISKS || self.config().disks.len() > MAX_DISKS {
            return Err(limit());
        }
        let mut blockers = Vec::new();
        let mut disks = Vec::with_capacity(registered.len());
        for registration in registered {
            disks.push(observe(self, registration, &mut blockers)?);
        }
        let mut preview = DiskSourceNamePreview {
            source: DiskNameObservationSource::ConfiguredRootObservation,
            execution: DiskNamePreviewExecution::PreviewOnly,
            topology_generation: self.state().topology_generation,
            registry: self.current_disk_registry_commitment().cloned(),
            requested_disk_id: disk_id,
            proposed_name: proposed_name.clone(),
            disks,
            slices: Vec::new(),
            rename_roots: Vec::new(),
            blockers,
        };
        plan::build(&mut preview)?;
        // Disk, slice and ancestry bounds constrain allocation before encoding.
        // The final cap leaves room for the HTTP envelope's independent bound.
        if crate::canonical_json(&preview)
            .map_err(crate::Error::from)?
            .len()
            > MAX_RESPONSE_BYTES
        {
            return Err(limit());
        }
        Ok(preview)
    }
}

fn observe(
    engine: &Engine,
    registered: RegisteredDisk<'_>,
    blockers: &mut Vec<DiskNamePreviewBlocker>,
) -> Result<DiskNameObservation> {
    let identity = DiskSourceIdentity {
        pool_id: registered
            .pool_id
            .parse::<PoolId>()
            .map_err(crate::Error::from)?,
        disk_id: registered
            .disk_id
            .parse::<DiskId>()
            .map_err(crate::Error::from)?,
        slot_id: SlotId::parse(registered.slot_id).map_err(crate::Error::from)?,
        binding_generation: registered.binding_generation,
    };
    let mut observation = DiskNameObservation {
        identity,
        registration_mode: registered.registration_mode,
        level: registered.level,
        upstream_slot_id: registered
            .upstream_slot_id
            .map(SlotId::parse)
            .transpose()
            .map_err(crate::Error::from)?,
        observed_name: None,
        component_maximum: None,
    };
    let disk_id = observation.identity.disk_id;
    let configured = engine.config().disks.iter().find(|disk| {
        registered.pool_id == engine.config().pool_id
            && disk.disk_id == registered.disk_id
            && disk.slot_id == registered.slot_id
            && disk.binding_generation == registered.binding_generation
            && disk.level == registered.level
            && disk.upstream_slot_id.as_deref() == registered.upstream_slot_id
    });
    let Some(configured) = configured else {
        blockers.push(DiskNamePreviewBlocker::ConfigIdentityMismatch { disk_id });
        return Ok(observation);
    };
    match configured.root.file_name().and_then(|name| name.to_str()) {
        Some(name) => match Name::parse(name) {
            Ok(name) => observation.observed_name = Some(name),
            Err(source) => blockers.push(DiskNamePreviewBlocker::InvalidName { disk_id, source }),
        },
        None => blockers.push(DiskNamePreviewBlocker::NameUnavailable { disk_id }),
    }
    match engine.disk_resources.get(configured) {
        Ok(handle) => {
            let seal = &handle.seal;
            if seal.pool_id != registered.pool_id
                || seal.disk_id != registered.disk_id
                || seal.slot_id != registered.slot_id
                || seal.binding_generation != registered.binding_generation
            {
                blockers.push(DiskNamePreviewBlocker::ResourceIdentityMismatch { disk_id });
            } else {
                match handle.root.component_name_maximum() {
                    Ok(maximum) => observation.component_maximum = Some(maximum),
                    Err(source) => blockers.push(DiskNamePreviewBlocker::BudgetUnavailable {
                        disk_id,
                        code: source.code(),
                        source: Box::new(source),
                    }),
                }
            }
        }
        Err(source) => blockers.push(DiskNamePreviewBlocker::BudgetUnavailable {
            disk_id,
            code: source.code(),
            source: Box::new(source),
        }),
    }
    Ok(observation)
}

#[cfg(test)]
#[path = "../../../test/engine/unit/disk_names_tests.rs"]
mod tests;
