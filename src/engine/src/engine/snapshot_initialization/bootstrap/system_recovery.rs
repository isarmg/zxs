//! SYSTEM-loss source admission. This stage never creates a SYSTEM, selector,
//! journal record or cache. The returned owner retains the original member
//! descriptors and writer locks for the later explicitly authorized decision.
use crate::authority_selection::{FreshBootstrapError, FreshSelection};
use crate::disk::{DiskResources, DiskRoot, DiskWriter, FenceOwner};
use crate::engine::snapshot_initialization::bootstrap::{
    MAX_BYTES, RootRecord, decision as bootstrap_decision, failure, verify_scoped_roots,
};
use crate::journal::{Candidate, OpenJournal, Publication, PublicationSeal, Receipt, RecordType};
use crate::persist::layout;
use crate::{Config, Engine, State, ValidatedConfig};
use platform_io::fs_safety::PrivateDirectory;
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
mod decision;
pub mod control;
mod execute;
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

/// An opaque live source proof, not a deserializable recovery permission.
/// No destination mutation is authorized by obtaining this value alone.
pub struct StorageRecoveryAdmission {
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    writers: BTreeMap<String, DiskWriter>,
    journals: BTreeMap<String, OpenJournal>,
    original: RootRecord,
    origin: FreshSelection,
    source: Candidate,
    state: State,
    evidence: MaterialCommitment,
}

fn refused() -> crate::Error {
    failure(FreshBootstrapError::Changed)
}

impl StorageRecoveryAdmission {
    /// Bind a durable control recovery intent to these exact admitted sources.
    pub fn evidence_commitment(&self) -> MaterialCommitment {
        self.evidence
    }

    pub fn origin_operation(&self) -> crate::OperationId {
        self.original.operation
    }

    /// Compare a retained control initialization intent with the unchanged
    /// original storage authority, without admitting a new configuration.
    pub fn require_initialization_origin(
        &self, operation: crate::OperationId, config: &Config,
    ) -> crate::Result<()> {
        if operation != self.original.operation
            || storage_contract::canonical_json(config)?
                != storage_contract::canonical_json(&self.config)?
        {
            return Err(refused());
        }
        Ok(())
    }

    /// Bind the retained control owner to the source's original SYSTEM entity.
    pub fn require_initialization_binding(
        &self, operation: crate::OperationId, config: &Config, system: LocalEntityIdentity,
    ) -> crate::Result<()> {
        self.require_initialization_origin(operation, config)?;
        if self.origin.system != system {
            return Err(refused());
        }
        Ok(())
    }

    /// A completed retry keeps the original source evidence from its permanent
    /// recovery receipt. It must not compare that evidence with the later tip.
    pub fn require_recovery_intent_source(
        &self,
        operation: crate::OperationId,
        principal: &storage_contract::Principal,
        source_evidence: MaterialCommitment,
        control_intent: MaterialCommitment,
    ) -> crate::Result<()> {
        let Some(result) = self.state.operations.get(&operation) else {
            return if source_evidence == self.evidence { Ok(()) } else { Err(refused()) };
        };
        if result.kind != decision::KIND {
            return Err(refused());
        }
        let binding: decision::RecoveryBinding = storage_contract::decode_strict(
            &storage_contract::canonical_json(&result.result)?,
        )?;
        if binding.request.operation != operation || &binding.request.principal != principal
            || binding.request.source_evidence != source_evidence
            || binding.request.control_intent != control_intent
            || result != &binding.result(result.sequence)?
        {
            return Err(refused());
        }
        Ok(())
    }
}

impl Engine {
    /// Admit an explicitly supplied original metadata quorum without touching
    /// unlisted roots or any destination SYSTEM. Pending/inconsistent prefixes
    /// are preserved and refused, not repaired by this read-only stage.
    pub fn admit_storage_recovery(
        metadata_directories: Vec<PrivateDirectory>,
    ) -> crate::Result<StorageRecoveryAdmission> {
        let mut originals = BTreeMap::new();
        let mut agreed: Option<(ValidatedConfig, Vec<u8>, FreshSelection)> = None;
        let mut identities = BTreeSet::new();
        for directory in metadata_directories {
            directory.verify_identity()?;
            let path = directory.path().to_owned();
            let held = OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)?;
            let physical = held.identity();
            if !identities.insert((physical.device, physical.inode)) {
                return Err(refused());
            }
            let root = DiskRoot::from_directory(path.clone(), held.into_directory())?;
            let seal = crate::disk::decode_seal(&root.read_entry_bytes(
                &layout::DISK_IDENTITY, MAX_BYTES,
            )?)?;
            if seal.device != physical.device || seal.inode != physical.inode {
                return Err(refused());
            }
            let saved: Config = root.read_json_observed(
                &path.join(crate::persist::layout::GENESIS_CONFIG.as_path()), &mut |_| Ok(()),
            )?;
            let selector = root.read_bytes(
                &path.join(crate::authority_selection::FRESH_DISK_SELECTOR), MAX_BYTES,
            )?;
            let config = FreshSelection::admit_disk(
                ValidatedConfig::try_from(saved)?, &seal, &selector,
            )?;
            let member = config.disks.iter().find(|disk| disk.disk_id == seal.disk_id)
                .ok_or_else(refused)?;
            if !member.metadata_member || member.root != path {
                return Err(refused());
            }
            let bytes = storage_contract::canonical_json(&config)?;
            if let Some((_, first, original)) = &agreed {
                if first != &bytes || original.canonical_bytes()? != selector {
                    return Err(refused());
                }
            } else {
                let original = storage_contract::decode_strict(&selector)?;
                agreed = Some((config, bytes, original));
            }
            if originals.insert(seal.disk_id, root).is_some() {
                return Err(refused());
            }
        }
        let (config, configuration, origin) = agreed.ok_or_else(refused)?;
        if originals.len() < config.metadata_quorum() {
            return Err(failure(FreshBootstrapError::Incomplete));
        }
        let config = config.restrict_io_disks(originals.keys().cloned().collect())?;
        let mut files = Vec::new();
        for disk in config.io_disks() {
            let root = originals.get(&disk.disk_id).ok_or_else(refused)?;
            files.push(root.directory(root.path())?);
        }
        let resources = Arc::new(DiskResources::recover_held(&config, files)?);
        let mut writers = BTreeMap::new();
        // Existing locks only. Hold all selected members before reading the
        // authoritative prefix; an online writer cannot race the admitted tip.
        for disk in config.io_disks() {
            let root = &resources.get(disk)?.root;
            let file = root.open_regular_entry(&layout::DISK_WRITER_LOCK)?;
            crate::engine::config::acquire_writer(&file)?;
            writers.insert(disk.disk_id.clone(), DiskWriter::adopt(
                root, FenceOwner::new(file, None),
            )?);
        }
        let mut journals = BTreeMap::new();
        let mut common: Option<(RootRecord, Candidate, State, Vec<String>, Vec<u8>)> = None;
        for disk in config.io_disks() {
            let root = &resources.get(disk)?.root;
            let anchor: Config = root.read_json_observed(
                &disk.root.join(crate::persist::layout::GENESIS_CONFIG.as_path()), &mut |_| Ok(()),
            )?;
            if storage_contract::canonical_json(&anchor)? != configuration {
                return Err(refused());
            }
            let bytes = root.read_bytes(
                &disk.root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path()), MAX_BYTES,
            )?;
            let (bootstrap, record) = bootstrap_decision::BootstrapDecision::admit_source(&bytes)?;
            bootstrap.verify_membership(&config)?;
            if Some(record.selection) != config.fresh_bootstrap() {
                return Err(refused());
            }
            let selector = root.read_bytes(
                &disk.root.join(crate::authority_selection::FRESH_DISK_SELECTOR), MAX_BYTES,
            )?;
            verify_scoped_roots(
                &config, &record, &selector, Some(&resources), &[disk],
            ).map_err(failure)?;
            let position = config.disks.iter().position(|member| member.disk_id == disk.disk_id)
                .ok_or_else(refused)?;
            let writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK)?;
            let identity = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)?;
            if record.disk_writers.get(position)
                != Some(&LocalEntityIdentity::new(identity.device, identity.inode))
            {
                return Err(refused());
            }
            let opened = OpenJournal::scan_initial_at(
                root, disk, &config, crate::disk::MAX_METADATA,
            )?;
            if opened.scan.torn_tail || opened.scan.records.is_empty()
                || opened.scan.records.len() % 4 != 0
            {
                return Err(failure(FreshBootstrapError::Incomplete));
            }
            let mut previous = None;
            let mut hashes = Vec::new();
            let mut tip = None;
            for rows in opened.scan.records.chunks_exact(4) {
                let candidate: Candidate = crate::journal::load_object_at(
                    root, disk, "candidate", &rows[0].0.candidate_hash,
                )?;
                config.verify_candidate_membership(&candidate)?;
                if candidate.previous_snapshot != previous {
                    return Err(refused());
                }
                if hashes.is_empty() {
                    bootstrap.require_initial_candidate(&candidate)?;
                    bootstrap_decision::read_initial_state(root, disk, &candidate, &record)?;
                }
                let publication: Publication = crate::journal::load_object_at(
                    root, disk, "publication", &rows[3].0.object_hash,
                )?;
                crate::journal::verify_finalized_publication(&candidate, &publication)?;
                let seal: PublicationSeal = root.read_json_observed(
                    &crate::journal::publication_seal_path(disk, &publication.candidate_hash),
                    &mut |_| Ok(()),
                )?;
                crate::journal::verify_publication_seal(&candidate, &publication, &seal)?;
                for (index, kind) in [RecordType::Prepare, RecordType::Accept,
                    RecordType::Finalize, RecordType::Publication].into_iter().enumerate()
                {
                    if rows[index].0.record_type != kind
                        || rows[index].0.sequence != candidate.sequence
                        || rows[index].0.candidate_hash != publication.candidate_hash
                    {
                        return Err(refused());
                    }
                }
                if rows[0].0.object_hash != candidate.snapshot_hash
                    || rows[1].0.object_hash != publication.candidate_hash
                    || rows[2].0.object_hash
                        != storage_contract::object_hash("finalize", &publication.finalize)?
                {
                    return Err(refused());
                }
                for (index, receipts) in [
                    (1, &publication.finalize.accepts),
                    (2, &publication.finalize_receipts),
                    (3, &seal.receipts),
                ] {
                    let receipt = Receipt {
                        disk_id: disk.disk_id.clone(), slot_id: disk.slot_id.clone(),
                        record: rows[index].0.clone(), frame_hash: rows[index].1.clone(),
                    };
                    if !receipts.contains(&receipt) {
                        return Err(refused());
                    }
                }
                previous = Some(candidate.snapshot_hash.clone());
                hashes.push(publication.candidate_hash);
                tip = Some(candidate);
            }
            // A retained publication barrier outside this prefix disproves
            // its completeness. Never silently restore an older selected tip.
            let seals = disk.root.join("metadata/publication-seals");
            for entry in root.entries(&seals)? {
                let entry = entry?;
                let hash = entry.as_os_str().to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .ok_or_else(refused)?;
                if !hashes.iter().any(|known| known == hash) {
                    return Err(refused());
                }
            }
            let candidate = tip.ok_or_else(refused)?;
            let state = crate::publication_evidence::verified_snapshot_observed_at(
                &resources, std::slice::from_ref(disk), &candidate.snapshot_hash, &mut |_| {},
            )?.into_state();
            let first = bootstrap_decision::initial_state(&record)?;
            if state.operations.get(&record.operation) != first.operations.get(&record.operation)
                || state.operation_principals.get(&record.operation)
                    != first.operation_principals.get(&record.operation)
                || state.sequence != candidate.sequence
            {
                return Err(refused());
            }
            if let Some((original, first_tip, first_state, prefix, proof)) = &common {
                if original != &record || first_state != &state || prefix != &hashes
                    || proof != &bytes || first_tip.snapshot_hash != candidate.snapshot_hash
                {
                    return Err(refused());
                }
            } else {
                common = Some((record, candidate, state, hashes, bytes));
            }
            journals.insert(disk.disk_id.clone(), opened);
        }
        let (original, source, state, prefix, proof) = common.ok_or_else(refused)?;
        let evidence_bytes = storage_contract::canonical_json(&(
            configuration, proof, prefix, identities,
        ))?;
        let mut digest = blake3::Hasher::new_derive_key("zxs SYSTEM recovery source admission v1");
        digest.update(&evidence_bytes);
        Ok(StorageRecoveryAdmission {
            config, resources, writers, journals, original, origin, source, state,
            evidence: MaterialCommitment::from_digest(digest.finalize()),
        })
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/system_recovery_admission_tests.rs"]
mod tests;

pub(in crate::engine) fn completed_carrier(
    system: &PrivateDirectory,
    config: &ValidatedConfig,
    resources: &DiskResources,
) -> crate::Result<(crate::engine::snapshot_initialization::bootstrap::CompletionProof, RootRecord)> {
    decision::require_complete(system, config, resources)
}

pub(in crate::engine) fn verify_original_publication(
    system: &PrivateDirectory,
    candidate: &Candidate,
    next: &State,
    publication: &Publication,
) -> crate::Result<()> {
    let complete = decision::RecoveryCompletion::decode(&system.read_bounded(
        &crate::engine::snapshot_initialization::bootstrap::name(
            crate::engine::snapshot_initialization::bootstrap::COMPLETE,
        ).map_err(failure)?, MAX_BYTES,
    )?)?;
    let original = &complete.binding.origin;
    original.require_initial_candidate(candidate)?;
    let bytes = storage_contract::canonical_json(original)?;
    let (_, record) = bootstrap_decision::BootstrapDecision::admit_source(&bytes)?;
    let rebuilt = bootstrap_decision::BootstrapDecision::from_decided(
        candidate.clone(), publication.clone(), &record,
    )?;
    if storage_contract::canonical_json(&rebuilt)? != bytes
        || next != &bootstrap_decision::initial_state(&record)?
    {
        return Err(refused());
    }
    Ok(())
}

/// SYSTEM-only evidence. This does not admit member I/O or confer write authority.
pub(in crate::engine) fn admit_carrier_system(
    system: &PrivateDirectory,
) -> crate::Result<(super::CompletionProof, FreshSelection)> {
    use crate::authority_selection::carrier::RecoverySelection;
    let selected = RecoverySelection::read(system).map_err(failure)?.ok_or_else(refused)?;
    selected.require_directory(system).map_err(failure)?;
    let bytes = system.read_bounded(&super::name(super::COMPLETE).map_err(failure)?, MAX_BYTES)?;
    let completed = decision::RecoveryCompletion::decode(&bytes)?;
    completed.require_selection(&selected)?;
    let origin_bytes = storage_contract::canonical_json(&completed.binding.origin)?;
    let (_, original) = bootstrap_decision::BootstrapDecision::admit_source(&origin_bytes)?;
    if original.selection != selected.origin.commitment()?
        || system.read_bounded(&super::name(super::ROOTS).map_err(failure)?, MAX_BYTES)?
            != original.bytes().map_err(failure)?
    { return Err(refused()); }
    let writer = system.open_payload(&crate::persist::layout::CONTROL_WRITER_LOCK,
        platform_io::fs_safety::PayloadOpenMode::Existing)?;
    let identity = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)?;
    let (bootstrap_root, bootstrap_writer) = super::bootstrap_identity(system).map_err(failure)?;
    if writer.metadata()?.len() != 0
        || completed.binding.system_writer != LocalEntityIdentity::new(identity.device, identity.inode)
        || completed.binding.bootstrap_root != bootstrap_root
        || completed.binding.bootstrap_writer != bootstrap_writer
    { return Err(refused()); }
    Ok((super::CompletionProof {
        commitment: super::completion_commitment(&bytes), system: selected.request.system,
        bootstrap_root, bootstrap_writer,
    }, selected.origin))
}

impl Engine {
    /// Recheck the exact control lineage under this Engine's original held SYSTEM.
    pub fn require_recovery_control(&self, control: Option<&crate::StorageRecoveryControl>) -> crate::Result<()> {
        self._locks.require_selected()?;
        let system = self._locks.selected_directory()?;
        let selected = crate::authority_selection::carrier::RecoverySelection::read(&system)
            .map_err(failure)?;
        let Some(control) = control else {
            if selected.is_some() { return Err(refused()); }
            let original = FreshSelection::read(&system).map_err(failure)?.ok_or_else(refused)?;
            original.require_directory(&system).map_err(failure)?;
            if self.config.fresh_bootstrap() != Some(original.commitment()?) {
                return Err(refused());
            }
            return Ok(());
        };
        Self::probe_recovery_control(&system, control)?;
        let result = self.state.operations.get(&control.binding.operation).ok_or_else(refused)?;
        let selected = crate::authority_selection::carrier::RecoverySelection::read(&system)
            .map_err(failure)?.ok_or_else(refused)?;
        let binding: decision::RecoveryBinding = storage_contract::decode_strict(
            &storage_contract::canonical_json(&result.result)?,
        )?;
        if binding.request != selected.request || result != &binding.result(result.sequence)?
            || self.state.operation_principals.get(&control.binding.operation)
                != Some(&control.principal.as_str().to_owned())
        { return Err(refused()); }
        Ok(())
    }

    /// Control/SYSTEM-only preflight; ordinary Engine opening still verifies members.
    pub fn probe_recovery_control(
        system: &PrivateDirectory, control: &crate::StorageRecoveryControl,
    ) -> crate::Result<()> {
        admit_carrier_system(system)?;
        let selected = crate::authority_selection::carrier::RecoverySelection::read(system)
            .map_err(failure)?.ok_or_else(refused)?;
        if storage_contract::canonical_json(&selected.control)?
            != storage_contract::canonical_json(control)?
        { return Err(refused()); }
        Ok(())
    }
}
