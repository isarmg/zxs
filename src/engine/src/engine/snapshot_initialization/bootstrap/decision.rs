//! Permanent bootstrap registration uses the normal member decision protocol.
//! The installation marker carries that decision, never an independent authority.
use crate::engine::snapshot_initialization::bootstrap::{
    RootRecord, MAX_BYTES, failure,
};
use crate::authority_selection::FreshBootstrapError;
use crate::journal::{Candidate, Publication};
use crate::{Engine, OperationResult, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::working_tree::MaterialCommitment;

const KIND: &str = "storage.initialize";
const PRINCIPAL: &str = "system:storage-initializer";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct BootstrapDecision {
    pub(super) roots: MaterialCommitment,
    candidate: Candidate,
    publication: Publication,
}

fn rejected() -> crate::Error {
    failure(FreshBootstrapError::Changed)
}

fn roots_commitment(record: &RootRecord) -> crate::Result<MaterialCommitment> {
    let bytes = record.bytes().map_err(failure)?;
    let mut hash = blake3::Hasher::new_derive_key("zxs unified storage root registration v1");
    hash.update(&bytes);
    Ok(MaterialCommitment::from_digest(hash.finalize()))
}

fn initial_result(record: &RootRecord) -> crate::Result<OperationResult> {
    Ok(OperationResult {
        operation_id: record.operation,
        kind: KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: crate::CommitState::Published,
        protection_at_publish: crate::ProtectionAtPublish::NotApplicable,
        current_health: crate::CurrentHealth::Healthy,
        request_digest: roots_commitment(record)?.hash().to_string(),
        sequence: 1,
        result: storage_contract::decode_strict(&record.bytes().map_err(failure)?)?,
    })
}

impl BootstrapDecision {
    /// A membership archive may carry the original decision forward, never
    /// replace its operation result or principal with a newly synthesized one.
    pub(super) fn require_retained_operation(&self, state: &State) -> crate::Result<()> {
        let operation = self.candidate.operation_id;
        if state.operations.get(&operation) != Some(&self.publication.result)
            || state.operation_principals.get(&operation).map(String::as_str) != Some(PRINCIPAL)
        {
            return Err(rejected());
        }
        Ok(())
    }

    pub(super) fn require_generation_ancestor(
        &self,
        generation: &crate::engine::membership_transition::pending::activation::Generation,
    ) -> crate::Result<()> {
        // Generation verifies the exact candidate hash in its authenticated
        // frozen history, not merely a matching operation ID in a State map.
        generation.verify_candidate(&self.candidate)?;
        self.require_retained_operation(&generation.base)
    }

    pub(super) fn require_initial_candidate(&self, candidate: &Candidate) -> crate::Result<()> {
        if storage_contract::canonical_json(&self.candidate)?
            != storage_contract::canonical_json(candidate)?
        {
            return Err(rejected());
        }
        Ok(())
    }

    pub(super) fn admit_source(bytes: &[u8]) -> crate::Result<(Self, RootRecord)> {
        let proof: Self = storage_contract::decode_strict(bytes)?;
        let record = storage_contract::decode_strict(
            &storage_contract::canonical_json(&proof.publication.result.result)?,
        )?;
        let admitted = Self::decode(bytes, &record)?;
        Ok((admitted, record))
    }

    pub(super) fn verify_membership(&self, config: &crate::ValidatedConfig) -> crate::Result<()> {
        config.verify_candidate_membership(&self.candidate)
    }

    pub(super) fn from_decided(
        candidate: Candidate,
        publication: Publication,
        record: &RootRecord,
    ) -> crate::Result<Self> {
        let proof = Self { roots: roots_commitment(record)?, candidate, publication };
        proof.verify(record)?;
        Ok(proof)
    }

    pub(super) fn decode(bytes: &[u8], record: &RootRecord) -> crate::Result<Self> {
        let proof: Self = storage_contract::decode_strict(bytes)?;
        if storage_contract::canonical_json(&proof)? != bytes
            || proof.roots != roots_commitment(record)?
        {
            return Err(rejected());
        }
        proof.verify(record)?;
        Ok(proof)
    }

    pub(super) fn verify(&self, record: &RootRecord) -> crate::Result<()> {
        crate::journal::verify_finalized_publication(&self.candidate, &self.publication)?;
        let result = &self.publication.result;
        let retained: RootRecord = storage_contract::decode_strict(
            &storage_contract::canonical_json(&result.result)?,
        )?;
        if retained != *record
            || result.kind != KIND
            || result.operation_id != record.operation
            || self.candidate.operation_id != record.operation
            || self.candidate.sequence != 1
            || self.candidate.previous_snapshot.is_some()
            || self.candidate.generation().is_some()
            || self.candidate.directory_material().is_some()
            || !self.candidate.versions.is_empty()
            || self.publication.recovery.is_some()
            || result.request_digest != roots_commitment(record)?.hash().to_string()
        {
            return Err(rejected());
        }
        Ok(())
    }
}

impl Engine {
    pub(in crate::engine) fn publish_storage_bootstrap(
        &mut self,
        record: &RootRecord,
    ) -> crate::Result<()> {
        if self.state.sequence != 0 || !self.state.operations.is_empty()
            || self.snapshot.is_some() || self.next_slot != 1
        {
            return Err(rejected());
        }
        let mut next = (*self.state).clone();
        next.sequence = 1;
        let result = initial_result(record)?;
        next.operations.insert(record.operation, result.clone());
        next.operation_principals.insert(record.operation, PRINCIPAL.to_owned());
        let prepared = self.prepare_publication(next, Vec::new(), result, MAX_BYTES)?;
        self.decide_publication(prepared)?;
        Ok(())
    }

    fn storage_bootstrap_decision(
        &self,
        candidate: &Candidate,
        next: &State,
        publication: &Publication,
    ) -> crate::Result<Option<BootstrapDecision>> {
        if publication.result.kind != KIND {
            return Ok(None);
        }
        if next.operations.get(&candidate.operation_id) != Some(&publication.result)
            || next.operation_principals.get(&candidate.operation_id)
                .map(String::as_str) != Some(PRINCIPAL)
        {
            return Err(rejected());
        }
        let record: RootRecord = storage_contract::decode_strict(
            &storage_contract::canonical_json(&publication.result.result)?,
        )?;
        let proof = BootstrapDecision {
            roots: roots_commitment(&record)?,
            candidate: candidate.clone(),
            publication: publication.clone(),
        };
        proof.verify(&record)?;
        Ok(Some(proof))
    }
}

impl BootstrapDecision {
    pub(super) fn verify_installed(
        &self,
        config: &crate::ValidatedConfig,
        resources: &crate::disk::DiskResources,
        record: &RootRecord,
        bytes: &[u8],
    ) -> crate::Result<()> {
        self.verify(record)?;
        config.verify_candidate_membership(&self.candidate)?;
        let original = config.original_storage_configuration()?;
        for disk in &original.disks {
            let handle = resources.get(disk)?;
            let actual = handle.root.read_bytes(
                &disk.root.join(crate::persist::layout::STORAGE_INITIALIZATION_PROOF.as_path()),
                MAX_BYTES,
            )?;
            if actual != bytes {
                return Err(rejected());
            }
            if disk.metadata_member {
                let seal = handle.root.read_json_observed(
                    &crate::journal::publication_seal_path(disk, &self.publication.candidate_hash),
                    &mut |_| Ok(()),
                )?;
                crate::journal::verify_publication_seal(
                    &self.candidate, &self.publication, &seal,
                )?;
                let state = read_initial_state(&handle.root, disk, &self.candidate, record)?;
                if state.operations.get(&record.operation) != Some(&self.publication.result) {
                    return Err(rejected());
                }
            }
        }
        Ok(())
    }
}

impl Engine {
    pub(in crate::engine) fn complete_storage_bootstrap(
        &self,
        candidate: &Candidate,
        next: &State,
        publication: &Publication,
    ) -> crate::Result<()> {
        let Some(proof) = self.storage_bootstrap_decision(candidate, next, publication)? else {
            return Ok(());
        };
        let system = self._locks.selected_directory()?;
        if crate::authority_selection::carrier::RecoverySelection::read(&system)
            .map_err(failure)?.is_some()
        {
            return crate::engine::snapshot_initialization::bootstrap::system_recovery
                ::verify_original_publication(&system, candidate, next, publication);
        }
        let roots = system.read_bounded(&super::name(super::ROOTS).map_err(failure)?, MAX_BYTES)?;
        let record = RootRecord::decode(&roots).map_err(failure)?;
        proof.verify(&record)?;
        let selector = crate::authority_selection::FreshSelection::read(&system)
            .map_err(failure)?.ok_or_else(rejected)?;
        selector.require_directory(&system).map_err(failure)?;
        selector.require_config(&self.config.original_storage_configuration()?)?;
        if record.selection != selector.commitment()? || record.operation != selector.operation {
            return Err(rejected());
        }
        super::verify_roots(
            &self.config, &record, &selector.canonical_bytes()?, Some(&self.disk_resources),
        ).map_err(failure)?;
        let bytes = storage_contract::canonical_json(&proof)?;
        if bytes.len() > MAX_BYTES {
            return Err(failure(FreshBootstrapError::Format));
        }
        for disk in &self.config.disks {
            let handle = self.disk_resources.get(disk)?;
            self._locks.verify_disk(disk, &handle.root)?;
            handle.root.write_json(
                &disk.root.join(crate::persist::layout::STORAGE_INITIALIZATION_PROOF.as_path()),
                &proof,
                false,
            )?;
        }
        proof.verify_installed(&self.config, &self.disk_resources, &record, &bytes)?;
        let name = super::name(super::COMPLETE).map_err(failure)?;
        match system.read_bounded(&name, MAX_BYTES) {
            Ok(original) if original == bytes => {}
            Ok(_) => return Err(rejected()),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                platform_io::fs_safety::AtomicFile::create(&system, &name, &bytes)?;
            }
            Err(error) => return Err(error.into()),
        }
        system.verify_identity()?;
        Ok(())
    }
}

/// Only an explicit retry of the original selected operation can restore a
/// missing SYSTEM completion marker from every installed DISK decision proof.
/// Missing/partial proof sets remain fenced; no directory or identity is adopted.
/// Permission to restore only the original installation marker. This is not
/// ordinary replay authority and cannot be constructed from a local Complete.
pub(in crate::engine) struct BootstrapCompletionAdmission {
    selected: crate::authority_selection::FreshSelection,
}

impl BootstrapCompletionAdmission {
    pub(in crate::engine) fn admit(
        engine: &Engine,
        prefix: crate::engine::recovery::workspace_receipts::PublishedCarrierPrefix<'_>,
        record: &RootRecord,
    ) -> crate::Result<Self> {
        if prefix.recovery().is_some()
            || prefix.state().operations.values().any(|result| {
                result.kind == "storage.recover-system"
            })
        {
            return Err(rejected());
        }
        let initial = initial_state(record)?;
        if prefix.state().operations.get(&record.operation)
                != initial.operations.get(&record.operation)
            || prefix.state().operation_principals.get(&record.operation)
                != initial.operation_principals.get(&record.operation)
        {
            return Err(rejected());
        }
        engine._locks.require_selected()?;
        let system = engine._locks.selected_directory()?;
        let selected = crate::authority_selection::FreshSelection::read(&system)
            .map_err(failure)?.ok_or_else(rejected)?;
        selected.require_directory(&system).map_err(failure)?;
        selected.require_config(&engine.config)?;
        if selected.operation != record.operation || selected.commitment()? != record.selection {
            return Err(rejected());
        }
        Ok(Self { selected })
    }

    fn require_current(&self, engine: &Engine) -> crate::Result<()> {
        engine._locks.require_selected()?;
        let system = engine._locks.selected_directory()?;
        let current = crate::authority_selection::FreshSelection::read(&system)
            .map_err(failure)?.ok_or_else(rejected)?;
        current.require_directory(&system).map_err(failure)?;
        if current != self.selected {
            return Err(rejected());
        }
        Ok(())
    }
}

pub(super) fn restore_completion(
    engine: &Engine,
    admitted: BootstrapCompletionAdmission,
    system: &platform_io::fs_safety::PrivateDirectory,
    config: &crate::ValidatedConfig,
    resources: &crate::disk::DiskResources,
    record: &RootRecord,
) -> crate::Result<()> {
    admitted.require_current(engine)?;
    let selector = crate::authority_selection::FreshSelection::read(system)
        .map_err(failure)?.ok_or_else(rejected)?;
    selector.require_directory(system).map_err(failure)?;
    selector.require_config(config)?;
    if selector.operation != record.operation || selector.commitment()? != record.selection {
        return Err(rejected());
    }
    super::verify_roots(config, record, &selector.canonical_bytes()?, Some(resources))
        .map_err(failure)?;
    let writer = system.open_payload(
        &crate::persist::layout::CONTROL_WRITER_LOCK,
        platform_io::fs_safety::PayloadOpenMode::Existing,
    )?;
    let writer = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)?;
    if record.system_writer != storage_contract::working_tree::LocalEntityIdentity::new(
        writer.device, writer.inode,
    ) {
        return Err(rejected());
    }
    let (root, writer) = super::bootstrap_identity(system).map_err(failure)?;
    if root != record.bootstrap_root || writer != record.bootstrap_writer {
        return Err(rejected());
    }
    let disk = config.disks.first().ok_or_else(rejected)?;
    let bytes = resources.get(disk)?.root.read_bytes(
        &disk.root.join(crate::persist::layout::STORAGE_INITIALIZATION_PROOF.as_path()),
        MAX_BYTES,
    )?;
    let proof = BootstrapDecision::decode(&bytes, record)?;
    proof.verify_installed(config, resources, record, &bytes)?;
    admitted.require_current(engine)?;
    platform_io::fs_safety::AtomicFile::create(
        system, &super::name(super::COMPLETE).map_err(failure)?, &bytes,
    )?;
    system.verify_identity()?;
    Ok(())
}

impl Engine {
    pub(in crate::engine) fn require_storage_bootstrap_transition(
        &self,
        candidate: &Candidate,
        next: &State,
    ) -> crate::Result<()> {
        let result = next.operations.get(&candidate.operation_id).ok_or_else(rejected)?;
        if result.kind == "storage.recover-system" {
            return self.require_system_recovery_transition(candidate, next);
        }
        if result.kind != KIND {
            return crate::engine::container_drafts::format::require_manual_storage(&self.config);
        }
        let record: RootRecord = storage_contract::decode_strict(
            &storage_contract::canonical_json(&result.result)?,
        )?;
        let mut expected = (*self.state).clone();
        expected.sequence = 1;
        expected.operations.insert(record.operation, result.clone());
        expected.operation_principals.insert(record.operation, PRINCIPAL.to_owned());
        if self.config.fresh_bootstrap() != Some(record.selection)
            || self.config.fresh_completion().is_some()
            || self.state.sequence != 0
            || !self.state.operations.is_empty()
            || self.snapshot.is_some()
            || self.next_slot != 1
            || candidate.operation_id != record.operation
            || candidate.sequence != 1
            || candidate.previous_snapshot.is_some()
            || candidate.generation().is_some()
            || candidate.directory_material().is_some()
            || !candidate.versions.is_empty()
            || candidate.request_digest != roots_commitment(&record)?.hash().to_string()
            || &expected != next
        {
            return Err(rejected());
        }
        let system = platform_io::fs_safety::PrivateDirectory::open_existing(&self.root)?;
        let bytes = system.read_bounded(&super::name(super::ROOTS).map_err(failure)?, MAX_BYTES)?;
        if bytes != record.bytes().map_err(failure)? {
            return Err(rejected());
        }
        let selector = crate::authority_selection::FreshSelection::read(&system)
            .map_err(failure)?.ok_or_else(rejected)?;
        selector.require_directory(&system).map_err(failure)?;
        selector.require_config(&self.config.original_storage_configuration()?)?;
        if selector.operation != record.operation || selector.commitment()? != record.selection {
            return Err(rejected());
        }
        super::verify_roots(
            &self.config, &record, &selector.canonical_bytes()?, Some(&self.disk_resources),
        ).map_err(failure)
    }
}

/// Resolve and admit the complete initial graph under a per-member byte budget.
/// No caller can authorize a different root set with a matching operation ID.
pub(super) fn read_initial_state(
    root: &crate::disk::DiskRoot,
    disk: &crate::DiskConfig,
    candidate: &Candidate,
    record: &RootRecord,
) -> crate::Result<State> {
    if candidate.operation_id != record.operation || candidate.sequence != 1
        || candidate.previous_snapshot.is_some() || candidate.generation().is_some()
        || candidate.directory_material().is_some() || !candidate.versions.is_empty()
        || candidate.request_digest != roots_commitment(record)?.hash().to_string()
    {
        return Err(rejected());
    }
    let mut used = 0usize;
    let mut charge = |count: usize| -> crate::Result<()> {
        used = used.checked_add(count).ok_or_else(|| failure(FreshBootstrapError::Format))?;
        if used > MAX_BYTES {
            return Err(failure(FreshBootstrapError::Format));
        }
        Ok(())
    };
    let snapshot = crate::journal::load_object_observed_at(
        root, disk, "snapshot", &candidate.snapshot_hash, &mut charge,
    )?;
    let state = crate::journal::resolve_admitted_snapshot(snapshot, |kind, hash| {
        crate::journal::load_authenticated_object_observed_at(root, disk, kind, hash, &mut charge)
    })?;
    let mut expected = State { topology_generation: 1, sequence: 1, ..State::default() };
    expected.operations.insert(record.operation, initial_result(record)?);
    expected.operation_principals.insert(record.operation, PRINCIPAL.to_owned());
    if state.state() != &expected {
        return Err(rejected());
    }
    Ok(state.into_state())
}

pub(super) fn initial_state(record: &RootRecord) -> crate::Result<State> {
    let mut expected = State { topology_generation: 1, sequence: 1, ..State::default() };
    expected.operations.insert(record.operation, initial_result(record)?);
    expected.operation_principals.insert(record.operation, PRINCIPAL.to_owned());
    Ok(expected)
}
