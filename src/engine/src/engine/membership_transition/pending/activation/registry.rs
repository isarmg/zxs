//! Registry-bearing successor authority. Legacy Add cores omit this component
//! completely, preserving their existing bytes and hashes.
use super::*;
use crate::engine::disk_registry::objects::{GraphObjects, add_objects, read_objects};
use crate::engine::disk_registry::published::continuation::RegistryBoundary;
use crate::engine::disk_registry::{DiskRegistryCommitment, VerifiedDiskRegistry};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct RegistryContinuation {
    parent: DiskRegistryCommitment,
    registry: DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    source_business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    source_topology_generation: u64,
    objects: GraphObjects,
}

impl RegistryContinuation {
    pub(super) fn prepare(
        previous: &VerifiedDiskRegistry,
        intent: &TransitionIntent,
        source: &State,
        target: &ValidatedConfig,
    ) -> Result<Self> {
        let boundary = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "registry continuation has no source publication",
            )
        })?;
        if source.sequence != boundary.sequence {
            return Err(Error::new(
                ErrorCode::LogFork,
                "registry continuation source sequence differs",
            ));
        }
        let authority = intent.origin.predecessor().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "registry continuation has no parent authority",
            )
        })?;
        let prepared = previous.prepare_continuation(
            intent.origin.config(),
            target,
            RegistryBoundary {
                authority,
                snapshot: &boundary.snapshot_hash,
                sequence: boundary.sequence,
                topology_generation: source.topology_generation,
            },
        )?;
        let mut objects = BTreeMap::new();
        add_objects(
            &mut objects,
            previous.prepared_copy()?.objects(),
            ErrorCode::ObjectCorrupt,
        )?;
        add_objects(&mut objects, prepared.objects(), ErrorCode::ObjectCorrupt)?;
        let core = Self {
            parent: previous.commitment().clone(),
            registry: prepared.commitment().clone(),
            source_business_generation: source.business_generation,
            source_topology_generation: source.topology_generation,
            objects,
        };
        core.verify(intent, target)?;
        Ok(core)
    }

    pub(super) fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }
    pub(super) fn parent(&self) -> &DiskRegistryCommitment {
        &self.parent
    }
    pub(super) fn source_business_generation(&self) -> u64 {
        self.source_business_generation
    }
    pub(super) fn source_topology_generation(&self) -> u64 {
        self.source_topology_generation
    }

    /// Internal graph verification is insufficient on its own. PortableAuthority
    /// must also match parent() to the exact preceding core in its verified chain.
    pub(super) fn verify(
        &self,
        intent: &TransitionIntent,
        target: &ValidatedConfig,
    ) -> Result<VerifiedDiskRegistry> {
        let boundary = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "registry continuation lacks boundary",
            )
        })?;
        let authority = intent.origin.predecessor().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "registry continuation lacks predecessor",
            )
        })?;
        let previous = VerifiedDiskRegistry::read_graph(&self.parent, |kind, hash| {
            read_objects(&self.objects, kind, hash)
        })?;
        previous.verify_continuation(
            intent.origin.config(),
            target,
            RegistryBoundary {
                authority,
                snapshot: &boundary.snapshot_hash,
                sequence: boundary.sequence,
                topology_generation: self.source_topology_generation,
            },
            &self.registry,
            |kind, hash| read_objects(&self.objects, kind, hash),
        )
    }

    pub(super) fn verify_source(&self, intent: &TransitionIntent, source: &State) -> Result<()> {
        if source.sequence
            != intent
                .boundary
                .as_ref()
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "registry continuation lacks source",
                    )
                })?
                .sequence
            || source.business_generation != self.source_business_generation
            || source.topology_generation != self.source_topology_generation
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "registry continuation source state differs",
            ));
        }
        Ok(())
    }
}
