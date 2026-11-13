//! The permanent recovery operation binds a new carrier, not a new origin.
use crate::authority_selection::carrier::{RecoveryRequest, RecoverySelection};
use crate::authority_selection::FreshBootstrapError;
use crate::engine::snapshot_initialization::bootstrap::{
    CompletionProof, MAX_BYTES, RootRecord, completion_commitment, decision::BootstrapDecision,
    failure, name, ROOTS, COMPLETE,
};
use crate::journal::{Candidate, Publication};
use crate::{Engine, OperationResult, State, ValidatedConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AtomicFile, PrivateDirectory};
use storage_contract::working_tree::LocalEntityIdentity;

pub(super) const KIND: &str = "storage.recover-system";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RecoveryBinding {
    pub(super) request: RecoveryRequest,
    pub(super) system_writer: LocalEntityIdentity,
    pub(super) bootstrap_root: LocalEntityIdentity,
    pub(super) bootstrap_writer: LocalEntityIdentity,
    pub(super) origin: BootstrapDecision,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RecoveryCompletion {
    pub(super) binding: RecoveryBinding,
    pub(super) candidate: Candidate,
    pub(super) publication: Publication,
}

fn refused() -> crate::Error {
    failure(FreshBootstrapError::Changed)
}

impl RecoveryBinding {
    pub(super) fn result(&self, sequence: u64) -> crate::Result<OperationResult> {
        Ok(OperationResult {
            operation_id: self.request.operation,
            kind: KIND.into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: crate::CommitState::Published,
            protection_at_publish: crate::ProtectionAtPublish::NotApplicable,
            current_health: crate::CurrentHealth::Healthy,
            request_digest: storage_contract::object_hash("system-recovery-binding", self)?,
            sequence,
            result: storage_contract::decode_strict(&storage_contract::canonical_json(self)?)?,
        })
    }
}

impl RecoveryCompletion {
    pub(super) fn decode(bytes: &[u8]) -> crate::Result<Self> {
        let complete: Self = storage_contract::decode_strict(bytes)?;
        if storage_contract::canonical_json(&complete)? != bytes || bytes.len() > MAX_BYTES {
            return Err(refused());
        }
        crate::journal::verify_finalized_publication(&complete.candidate, &complete.publication)?;
        let expected = complete.binding.result(complete.candidate.sequence)?;
        if complete.publication.result != expected
            || complete.candidate.operation_id != complete.binding.request.operation
            || complete.candidate.previous_snapshot.as_deref()
                != Some(complete.binding.request.source_snapshot.to_string().as_str())
            || complete.candidate.request_digest != expected.request_digest
            || complete.candidate.generation().is_some()
            || complete.candidate.directory_material().is_some()
            || !complete.candidate.versions.is_empty()
            || complete.publication.recovery.is_some()
            || !complete.publication.payload_receipts.is_empty()
        {
            return Err(refused());
        }
        Ok(complete)
    }

    pub(super) fn require_selection(&self, selected: &RecoverySelection) -> crate::Result<()> {
        if self.binding.request != selected.request {
            return Err(refused());
        }
        Ok(())
    }

    pub(super) fn require_transition(&self, previous: &State, next: &State) -> crate::Result<()> {
        let result = self.binding.result(self.candidate.sequence)?;
        let mut expected = previous.clone();
        if previous.operations.contains_key(&result.operation_id)
            || previous.sequence.checked_add(1) != Some(self.candidate.sequence)
        {
            return Err(refused());
        }
        expected.sequence = self.candidate.sequence;
        expected.operations.insert(result.operation_id, result.clone());
        expected.operation_principals.insert(result.operation_id,
            self.binding.request.principal.as_str().to_owned());
        if &expected != next {
            return Err(refused());
        }
        Ok(())
    }
}

pub(super) fn require_complete(
    system: &PrivateDirectory,
    config: &ValidatedConfig,
    resources: &crate::disk::DiskResources,
) -> crate::Result<(CompletionProof, RootRecord)> {
    let selected = RecoverySelection::read(system).map_err(failure)?.ok_or_else(refused)?;
    selected.require_directory(system).map_err(failure)?;
    let original = config.original_storage_configuration()?;
    selected.origin.require_config(&original)?;
    let bytes = system.read_bounded(&name(COMPLETE).map_err(failure)?, MAX_BYTES)?;
    let completed = RecoveryCompletion::decode(&bytes)?;
    completed.require_selection(&selected)?;
    let origin_bytes = storage_contract::canonical_json(&completed.binding.origin)?;
    let (_, record) = BootstrapDecision::admit_source(&origin_bytes)?;
    if system.read_bounded(&name(ROOTS).map_err(failure)?, MAX_BYTES)?
        != record.bytes().map_err(failure)?
        || config.fresh_bootstrap() != Some(record.selection)
    {
        return Err(refused());
    }
    completed.binding.origin.verify_installed(config, resources, &record, &origin_bytes)?;
    crate::engine::snapshot_initialization::bootstrap::verify_roots(
        config, &record, &selected.origin.canonical_bytes()?, Some(resources),
    ).map_err(failure)?;
    let system_writer = system.open_payload(&crate::persist::layout::CONTROL_WRITER_LOCK,
        platform_io::fs_safety::PayloadOpenMode::Existing)?;
    let writer = platform_io::fs_safety::linux::FileIdentity::from_fd(&system_writer)?;
    let (bootstrap_root, bootstrap_writer) =
        crate::engine::snapshot_initialization::bootstrap::bootstrap_identity(system)
            .map_err(failure)?;
    if system_writer.metadata()?.len() != 0
        || completed.binding.system_writer != LocalEntityIdentity::new(writer.device, writer.inode)
        || completed.binding.bootstrap_root != bootstrap_root
        || completed.binding.bootstrap_writer != bootstrap_writer
    {
        return Err(refused());
    }
    config.verify_candidate_membership(&completed.candidate)?;
    let members = original.metadata_members().cloned().collect::<Vec<_>>();
    for disk in &members {
        let root = &resources.get(disk)?.root;
        let seal = root.read_json_observed(
            &crate::journal::publication_seal_path(disk, &completed.publication.candidate_hash),
            &mut |_| Ok(()),
        )?;
        crate::journal::verify_publication_seal(&completed.candidate, &completed.publication, &seal)?;
    }
    let source: Candidate = crate::publication_evidence::verified_object_at(
        resources, &members, "candidate", &completed.binding.request.source_candidate.to_string(),
    )?;
    config.verify_candidate_membership(&source)?;
    if source.snapshot_hash != completed.binding.request.source_snapshot.to_string()
        || source.sequence.checked_add(1) != Some(completed.candidate.sequence)
    {
        return Err(refused());
    }
    let previous = crate::publication_evidence::verified_snapshot_at(
        resources, &members, &completed.binding.request.source_snapshot.to_string(),
    )?.into_state();
    let next = crate::publication_evidence::verified_snapshot_at(
        resources, &members, &completed.candidate.snapshot_hash,
    )?.into_state();
    completed.require_transition(&previous, &next)?;
    Ok((CompletionProof {
        commitment: completion_commitment(&bytes),
        system: selected.request.system,
        bootstrap_root,
        bootstrap_writer,
    }, record))
}

impl Engine {
    pub(in crate::engine) fn complete_system_recovery(
        &self, candidate: &Candidate, next: &State, publication: &Publication,
    ) -> crate::Result<()> {
        if publication.result.kind != KIND {
            return Ok(());
        }
        let binding: RecoveryBinding = storage_contract::decode_strict(
            &storage_contract::canonical_json(&publication.result.result)?,
        )?;
        let complete = RecoveryCompletion {
            binding, candidate: candidate.clone(), publication: publication.clone(),
        };
        let bytes = storage_contract::canonical_json(&complete)?;
        let complete = RecoveryCompletion::decode(&bytes)?;
        complete.require_transition(&self.state, next)?;
        let system = self._locks.selected_directory()?;
        let selected = RecoverySelection::read(&system).map_err(failure)?.ok_or_else(refused)?;
        if complete.binding.request.operation != selected.request.operation {
            // A later SYSTEM-loss recovery preserves earlier recovery receipts.
            // Their old carrier inode is historical, not authority to install
            // today's carrier. Verify the same origin and exact State delta,
            // but never rewrite today's Complete using an ancestor result.
            let origin_bytes = storage_contract::canonical_json(&complete.binding.origin)?;
            let (_, record) = BootstrapDecision::admit_source(&origin_bytes)?;
            if record.selection != selected.origin.commitment()? {
                return Err(refused());
            }
            return complete.binding.origin.verify_installed(
                &self.config, &self.disk_resources, &record, &origin_bytes,
            );
        }
        complete.require_selection(&selected)?;
        selected.require_directory(&system).map_err(failure)?;
        let output = name(COMPLETE).map_err(failure)?;
        match system.read_bounded(&output, MAX_BYTES) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => return Err(refused()),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {
                    AtomicFile::create(&system, &output, &bytes)?;
                }
            Err(error) => return Err(error.into()),
        }
        require_complete(&system, &self.config, &self.disk_resources)?;
        Ok(())
    }
}

impl Engine {
    pub(in crate::engine) fn require_system_recovery_transition(
        &self, candidate: &Candidate, next: &State,
    ) -> crate::Result<()> {
        let result = next.operations.get(&candidate.operation_id).ok_or_else(refused)?;
        if result.kind != KIND {
            return Err(refused());
        }
        let binding: RecoveryBinding = storage_contract::decode_strict(
            &storage_contract::canonical_json(&result.result)?,
        )?;
        let expected_result = binding.result(candidate.sequence)?;
        let system = self._locks.selected_directory()?;
        let selected = RecoverySelection::read(&system).map_err(failure)?.ok_or_else(refused)?;
        selected.require_directory(&system).map_err(failure)?;
        if binding.request != selected.request || result != &expected_result
            || candidate.operation_id != binding.request.operation
            || candidate.previous_snapshot.as_deref()
                != Some(binding.request.source_snapshot.to_string().as_str())
            || self.snapshot != candidate.previous_snapshot
            || candidate.request_digest != expected_result.request_digest
            || candidate.generation().is_some() || candidate.directory_material().is_some()
            || !candidate.versions.is_empty()
            || self.state.sequence.checked_add(1) != Some(candidate.sequence)
            || self.state.operations.contains_key(&candidate.operation_id)
        {
            return Err(refused());
        }
        let writer = self._locks.system_identity()?;
        let (bootstrap_root, bootstrap_writer) =
            crate::engine::snapshot_initialization::bootstrap::bootstrap_identity(&system)
                .map_err(failure)?;
        if binding.system_writer != LocalEntityIdentity::new(writer.device, writer.inode)
            || binding.bootstrap_root != bootstrap_root || binding.bootstrap_writer != bootstrap_writer
        {
            return Err(refused());
        }
        let members = self.members();
        let source: Candidate = crate::publication_evidence::verified_object_at(
            &self.disk_resources, &members, "candidate", &binding.request.source_candidate.to_string(),
        )?;
        self.config.verify_candidate_membership(&source)?;
        if source.snapshot_hash != binding.request.source_snapshot.to_string()
            || source.sequence != self.state.sequence
        {
            return Err(refused());
        }
        let mut expected = (*self.state).clone();
        expected.sequence = candidate.sequence;
        expected.operations.insert(candidate.operation_id, expected_result);
        expected.operation_principals.insert(candidate.operation_id,
            binding.request.principal.as_str().to_owned());
        if &expected != next {
            return Err(refused());
        }
        let origin_bytes = storage_contract::canonical_json(&binding.origin)?;
        let (_, record) = BootstrapDecision::admit_source(&origin_bytes)?;
        binding.origin.verify_installed(&self.config, &self.disk_resources, &record, &origin_bytes)?;
        Ok(())
    }
}
