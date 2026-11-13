//! Preserve the immutable initialization decision across membership handoff.
//! This admission is necessary but does not itself admit successor seals or
//! replace the membership publication, held-root, and cold-discovery gates.
use crate::authority_selection::FreshBootstrapError;
use crate::engine::membership_transition::pending::activation::Generation;
use crate::engine::snapshot_initialization::bootstrap::{
    MAX_BYTES, decision::BootstrapDecision, failure, require_complete_resources,
};
use crate::persist::layout;
use crate::Engine;

impl Engine {
    pub(in crate::engine) fn require_unified_membership_ancestor(
        &self,
        generation: &Generation,
    ) -> crate::Result<()> {
        if self.config.fresh_bootstrap().is_none() {
            // Legacy generation callers retain their existing independent
            // authority gates. This does not grant them unified format proof.
            return Ok(());
        }
        let system = self._locks.selected_directory()?;
        require_complete_resources(&system, &self.config, Some(&self.disk_resources))?;
        let origin = match self.config.generation() {
            Some(generation) => generation.original(),
            None => &self.config,
        };
        if storage_contract::canonical_json(origin)?
            != storage_contract::canonical_json(generation.original())?
        {
            return Err(failure(FreshBootstrapError::Changed));
        }
        let disk = origin.disks.first().ok_or_else(|| failure(FreshBootstrapError::Changed))?;
        let root = &self.disk_resources.get(disk)?.root;
        let bytes = root.read_bytes(
            &disk.root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path()), MAX_BYTES,
        )?;
        let (decision, _) = BootstrapDecision::admit_source(&bytes)?;
        decision.require_generation_ancestor(generation)
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/unified_membership_ancestor_tests.rs"]
mod tests;

/// Exact immutable origin evidence for a NEW unified membership intent. The
/// intent hash makes all these bytes reachable from Proposal and certificates.
/// Existing bootstrap publications are referenced, never rewritten.
#[derive(Clone, Debug, data_serialization::Serialize, data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct UnifiedOriginEvidence {
    configuration: crate::Config,
    selection: crate::authority_selection::FreshSelection,
    decision: BootstrapDecision,
}

impl UnifiedOriginEvidence {
    pub(in crate::engine) fn verify(&self) -> crate::Result<()> {
        let config = crate::ValidatedConfig::try_from(self.configuration.clone())?;
        self.selection.require_config(&config)?;
        let bytes = storage_contract::canonical_json(&self.decision)?;
        let (proof, record) = BootstrapDecision::admit_source(&bytes)?;
        proof.verify_membership(&config)?;
        if record.selection != self.selection.commitment()?
            || record.operation != self.selection.operation
        {
            return Err(failure(FreshBootstrapError::Changed));
        }
        Ok(())
    }

    pub(in crate::engine) fn require_genesis(&self, config: &crate::ValidatedConfig)
        -> crate::Result<()>
    {
        self.verify()?;
        self.selection.require_config(config)
    }

    pub(in crate::engine) fn configuration(&self) -> crate::Result<crate::ValidatedConfig> {
        self.verify()?;
        crate::ValidatedConfig::try_from(self.configuration.clone())?
            .with_fresh_bootstrap(&self.selection)
    }

    pub(in crate::engine) fn require_pool(&self, config: &crate::ValidatedConfig) -> crate::Result<()> {
        self.verify()?;
        crate::authority_selection::FreshSelection::require_primary(config)
            .map_err(failure)?;
        if self.configuration.pool_id != config.pool_id {
            return Err(failure(FreshBootstrapError::Changed));
        }
        Ok(())
    }

    pub(in crate::engine) fn successor_seal_format(&self, hash: &str)
        -> crate::Result<crate::disk::DiskSealFormat>
    {
        self.verify()?;
        Ok(crate::disk::DiskSealFormat::UnifiedStorageSuccessor {
            bootstrap: self.selection.commitment()?,
            generation: parse_authority(hash)?,
        })
    }

    pub(in crate::engine) fn require_original_seal(&self, seal: &crate::disk::DiskSeal)
        -> crate::Result<()>
    {
        self.verify()?;
        let config = crate::ValidatedConfig::try_from(self.configuration.clone())?;
        crate::authority_selection::FreshSelection::admit_disk(
            config, seal, &self.selection.canonical_bytes()?,
        )?;
        Ok(())
    }

    pub(in crate::engine) fn original_seal_format(&self) -> crate::Result<crate::disk::DiskSealFormat> {
        self.verify()?;
        Ok(crate::disk::DiskSealFormat::UnifiedStorage {
            bootstrap: self.selection.commitment()?,
        })
    }
}

impl Engine {
    pub(in crate::engine) fn capture_unified_origin(&self) -> crate::Result<UnifiedOriginEvidence> {
        let system = self._locks.selected_directory()?;
        require_complete_resources(&system, &self.config, Some(&self.disk_resources))?;
        let selection = match crate::authority_selection::carrier::RecoverySelection::read(&system)
            .map_err(failure)?
        {
            Some(recovery) => recovery.origin,
            None => crate::authority_selection::FreshSelection::read(&system)
                .map_err(failure)?.ok_or_else(|| failure(FreshBootstrapError::Incomplete))?,
        };
        let config = match self.config.generation() {
            Some(generation) => generation.original(),
            None => &self.config,
        };
        let disk = config.disks.first().ok_or_else(|| failure(FreshBootstrapError::Changed))?;
        let bytes = self.disk_resources.get(disk)?.root.read_bytes(
            &disk.root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path()), MAX_BYTES,
        )?;
        let (decision, _) = BootstrapDecision::admit_source(&bytes)?;
        decision.require_retained_operation(&self.state)?;
        let evidence = UnifiedOriginEvidence {
            configuration: (**config).clone(), selection, decision,
        };
        evidence.verify()?;
        Ok(evidence)
    }
}

/// Runtime-only closure of immutable origin, current carrier and independently
/// verified generation. Deserializing Config can never construct this proof.
#[derive(Clone, Debug)]
pub(crate) struct UnifiedSuccessorAdmission {
    origin: UnifiedOriginEvidence,
    generation: storage_contract::working_tree::MaterialCommitment,
    completion: crate::engine::snapshot_initialization::bootstrap::CompletionProof,
}

impl UnifiedSuccessorAdmission {
    pub(in crate::engine) fn new(
        origin: UnifiedOriginEvidence,
        generation: &Generation,
        completion: crate::engine::snapshot_initialization::bootstrap::CompletionProof,
    ) -> crate::Result<Self> {
        origin.require_genesis(generation.original())?;
        origin.decision.require_generation_ancestor(generation)?;
        let generation = parse_authority(&generation.hash)?;
        Ok(Self { origin, generation, completion })
    }

    pub(crate) fn generation(&self) -> storage_contract::working_tree::MaterialCommitment {
        self.generation
    }

    pub(in crate::engine) fn origin(&self) -> &UnifiedOriginEvidence {
        &self.origin
    }

    pub(in crate::engine) fn bootstrap(&self)
        -> crate::Result<storage_contract::working_tree::MaterialCommitment>
    {
        self.origin.selection.commitment()
    }

    pub(in crate::engine) fn completion(&self)
        -> &crate::engine::snapshot_initialization::bootstrap::CompletionProof
    {
        &self.completion
    }
}

/// The existing immutable DISK Genesis anchor supplies bytes, not authority.
/// Only a locator-authorized held root may be read, and the original selector
/// must authenticate the full Config before any member resource is opened.
pub(in crate::engine) fn read_original_configuration(
    store: &crate::engine::disk_lifecycle::bootstrap::store::BootstrapStore,
    projected: &crate::ValidatedConfig,
    origin: &crate::authority_selection::FreshSelection,
) -> crate::Result<crate::ValidatedConfig> {
    let locator = store.reload(&projected.pool_id)
        .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)?;
    let admissions = store.readmission_probe_admissions(&locator)?;
    let probes = locator.select_readmission_probes(projected, &admissions)
        .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)?;
    let mut original = None;
    probes.try_for_each(|probe| -> crate::Result<()> {
        if original.is_some() || !origin.disks.iter().any(|disk| disk.path == probe.path()) {
            return Ok(());
        }
        let root = crate::disk::DiskRoot::open(probe.path())?;
        let physical = platform_io::fs_safety::linux::FileIdentity::from_fd(
            root.directory(root.path())?,
        )?;
        if physical != probe.expected_identity() {
            return Err(failure(FreshBootstrapError::Changed));
        }
        let bytes = root.read_bytes(&probe.path().join(layout::GENESIS_CONFIG.as_path()), MAX_BYTES)?;
        let saved: storage_contract::Config = storage_contract::decode_strict(&bytes)?;
        let config = crate::ValidatedConfig::try_from(saved)?;
        origin.require_config(&config)?;
        original = Some(config.with_fresh_bootstrap(origin)?);
        Ok(())
    })?;
    original.ok_or_else(|| failure(FreshBootstrapError::Incomplete))
}

fn parse_authority(value: &str)
    -> crate::Result<storage_contract::working_tree::MaterialCommitment>
{
    let digest = value.parse::<storage_contract::ContentHash>()
        .map_err(|_| failure(FreshBootstrapError::Changed))?;
    Ok(storage_contract::working_tree::MaterialCommitment::from_digest(digest.digest()))
}
