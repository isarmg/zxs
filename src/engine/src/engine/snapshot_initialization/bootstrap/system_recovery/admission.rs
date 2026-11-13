//! Read-only carrier selection from the authenticated complete publication prefix.
use crate::authority_selection::carrier::RecoverySelection;
use crate::authority_selection::{FreshBootstrapError, FreshSelection};
use crate::engine::snapshot_initialization::bootstrap::{
    COMPLETE, MAX_BYTES, decision::BootstrapDecision, failure, name,
};
use crate::engine::snapshot_initialization::bootstrap::system_recovery::decision::{
    KIND, RecoveryBinding, RecoveryCompletion,
};
use crate::Engine;
use crate::engine::recovery::workspace_receipts::PublishedCarrierPrefix;

/// Retained only by one prepared recovery and rechecked before journal writers.
/// Neither a caller-provided State nor a local Complete can construct this proof.
pub(in crate::engine) struct CarrierAdmission {
    selected: SelectedCarrier,
    completion: crate::engine::snapshot_initialization::bootstrap::CompletionProof,
}

enum SelectedCarrier {
    Original(FreshSelection),
    Recovered(RecoverySelection),
}

fn refused() -> crate::Error {
    failure(FreshBootstrapError::Changed)
}

impl CarrierAdmission {
    fn admit(
        engine: &Engine,
        prefix: PublishedCarrierPrefix<'_>,
    ) -> crate::Result<Self> {
        let state = prefix.state();
        engine._locks.require_selected()?;
        let system = engine._locks.selected_directory()?;
        let selected = RecoverySelection::read(&system).map_err(failure)?;
        let mut latest = None;
        for result in state.operations.values().filter(|result| result.kind == KIND) {
            let binding: RecoveryBinding = crate::decode_strict(
                &crate::canonical_json(&result.result)?,
            )?;
            if result != &binding.result(result.sequence)?
                || state.operation_principals.get(&result.operation_id)
                    .map(String::as_str) != Some(binding.request.principal.as_str())
            {
                return Err(refused());
            }
            let bytes = crate::canonical_json(&binding.origin)?;
            let (_, record) = BootstrapDecision::admit_source(&bytes)?;
            binding.origin.verify_membership(&engine.config.original_storage_configuration()?)?;
            let initial = crate::engine::snapshot_initialization::bootstrap::decision
                ::initial_state(&record)?;
            if state.operations.get(&record.operation) != initial.operations.get(&record.operation)
                || state.operation_principals.get(&record.operation)
                    != initial.operation_principals.get(&record.operation)
                || engine.config.fresh_bootstrap() != Some(record.selection)
            {
                return Err(refused());
            }
            match &latest {
                Some((previous, _)) if *previous == result.sequence => return Err(refused()),
                Some((previous, _)) if *previous > result.sequence => {}
                _ => latest = Some((result.sequence, binding)),
            }
        }
        // The tip cannot erase or replace a carrier decision encountered in the
        // authenticated prefix (or its authenticated generation archive seed).
        let retained = latest.as_ref().map(|(sequence, binding)| {
            binding.result(*sequence)
        }).transpose()?;
        if retained.as_ref() != prefix.recovery() {
            return Err(refused());
        }
        let admitted = match (latest, selected) {
            (None, None) => {
                let original = FreshSelection::read(&system).map_err(failure)?
                    .ok_or_else(refused)?;
                original.require_directory(&system).map_err(failure)?;
                if engine.config.fresh_bootstrap() != Some(original.commitment()?) {
                    return Err(refused());
                }
                SelectedCarrier::Original(original)
            }
            (Some((sequence, binding)), Some(selected)) => {
                selected.require_directory(&system).map_err(failure)?;
                if binding.request != selected.request {
                    return Err(refused());
                }
                let bytes = system.read_bounded(&name(COMPLETE).map_err(failure)?, MAX_BYTES)?;
                let complete = RecoveryCompletion::decode(&bytes)?;
                complete.require_selection(&selected)?;
                if complete.candidate.sequence != sequence
                    || crate::canonical_json(&complete.binding)? != crate::canonical_json(&binding)?
                {
                    return Err(refused());
                }
                SelectedCarrier::Recovered(selected)
            }
            _ => return Err(refused()),
        };
        let (completion, _) = crate::engine::snapshot_initialization::bootstrap
            ::admit_carrier_system(&system)?;
        let admitted = Self { selected: admitted, completion };
        admitted.require_current(engine)?;
        Ok(admitted)
    }

    pub(in crate::engine) fn require_current(&self, engine: &Engine) -> crate::Result<()> {
        engine._locks.require_selected()?;
        let system = engine._locks.selected_directory()?;
        let (completion, origin) = crate::engine::snapshot_initialization::bootstrap
            ::admit_carrier_system(&system)?;
        if completion != self.completion {
            return Err(refused());
        }
        match &self.selected {
            SelectedCarrier::Original(expected) => {
                if &origin != expected
                    || RecoverySelection::read(&system).map_err(failure)?.is_some()
                {
                    return Err(refused());
                }
            }
            SelectedCarrier::Recovered(expected) => {
                let actual = RecoverySelection::read(&system).map_err(failure)?
                    .ok_or_else(refused)?;
                if actual.bytes().map_err(failure)? != expected.bytes().map_err(failure)? {
                    return Err(refused());
                }
            }
        }
        Ok(())
    }
}

impl Engine {
    // The replay seed is either Genesis or the complete authenticated frozen
    // generation archive, not a checkpoint or caller-supplied latest State.
    pub(in crate::engine) fn recovery_prefix_seed(&self) -> Option<crate::OperationResult> {
        self.state.operations.values().filter(|result| result.kind == KIND)
            .max_by_key(|result| result.sequence).cloned()
    }

    /// A retained recovery decision needs its explicit original-operation
    /// installer, never generic replay of earlier FILE slots on the old carrier.
    pub(in crate::engine) fn require_no_pending_carrier_replay(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> crate::Result<()> {
        let next = crate::publication_evidence::verified_snapshot_at(
            &self.disk_resources, &self.members(), &candidate.snapshot_hash,
        )?;
        let result = next.state().operations.get(&candidate.operation_id).ok_or_else(refused)?;
        if result.kind == KIND {
            return Err(failure(FreshBootstrapError::Incomplete));
        }
        Ok(())
    }

    pub(in crate::engine) fn admit_recovery_carrier_prefix(
        &self,
        prefix: PublishedCarrierPrefix<'_>,
    ) -> crate::Result<CarrierAdmission> {
        CarrierAdmission::admit(self, prefix)
    }
}
