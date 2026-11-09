//! Admit only the original unaccepted tail, then recheck its full preparation.
use super::*;
use crate::journal::Candidate;

impl Engine {
    /// Before opening recovery writers, retain only one original PREPARE-only
    /// tail. This establishes denial/identity, not a decision or payload proof.
    pub(in crate::engine) fn admit_selected_move_prepares(
        &mut self,
        records: &crate::engine::recovery_checkpoint::Records,
        torn_tail: bool,
    ) -> Result<()> {
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: selected move recovery has its original choice");
        if torn_tail {
            return Err(invalid(
                "selected move journal has a torn tail; preserve it without truncation",
            ));
        }
        let mut observed = None;
        for (sequence, candidates) in records {
            if *sequence <= choice.source_sequence() {
                continue;
            }
            if *sequence != choice.decision_slot() || candidates.len() != 1 {
                return Err(invalid(
                    "journal extends beyond the original move PREPARE slot",
                ));
            }
            let mut seen = BTreeSet::new();
            for (hash, receipts) in candidates {
                for receipt in receipts {
                    let disk = self
                        .config
                        .disks
                        .iter()
                        .find(|disk| {
                            disk.metadata_member
                                && disk.disk_id == receipt.disk_id
                                && disk.slot_id == receipt.slot_id
                        })
                        .ok_or_else(|| {
                            invalid("move PREPARE owner is not an original metadata member")
                        })?;
                    if !seen.insert(&receipt.disk_id) {
                        return Err(invalid(
                            "one member has multiple records in the move PREPARE slot",
                        ));
                    }
                    let root = &self.disk_resources.get(disk)?.root;
                    let candidate: Candidate =
                        crate::journal::load_object_at(root, disk, "candidate", hash)?;
                    self.verify_candidate_membership(&candidate)?;
                    choice.require_prepare_candidate(&candidate)?;
                    let binding = PrepareBinding {
                        operation: candidate.operation_id,
                        sequence: candidate.sequence,
                        candidate_hash: hash.clone(),
                        snapshot_hash: candidate.snapshot_hash,
                    };
                    binding.require_record(&receipt.record)?;
                    if observed
                        .as_ref()
                        .is_some_and(|original| original != &binding)
                    {
                        return Err(invalid(
                            "move PREPARE members disagree on the original candidate",
                        ));
                    }
                    observed = Some(binding);
                }
            }
        }
        if self
            .history_move_prepare
            .as_ref()
            .is_some_and(|original| observed.as_ref() != Some(original))
        {
            return Err(invalid(
                "retained move PREPARE disappeared or changed during recovery",
            ));
        }
        self.history_move_prepare = observed;
        Ok(())
    }

    /// Run after workspace loading and before disposable index rebuilding. Cold
    /// admission cannot repair envelopes, bodies, pending pairs or journal votes.
    pub(in crate::engine) fn verify_selected_move_prepare(&self) -> Result<()> {
        if self.history_move_prepare.is_none() {
            return Ok(());
        }
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: retained move PREPARE has its original choice");
        let original = self
            .inspect_file_history_move_recovery()?
            .ok_or_else(|| invalid("retained move PREPARE has no original recovery envelope"))?;
        if !choice.matches_recovery(&original) {
            return Err(invalid(
                "move PREPARE recovery envelope differs from its original choice",
            ));
        }
        let cancellation = HistoryWindowCancellation::default();
        let projection = self.read_original_move_projection(
            original.operation_id(),
            original.digest(),
            choice.principal(),
            &cancellation,
        )?;
        self.read_file_history_move_journal(&projection, original.digest(), &cancellation)?;
        Ok(())
    }
}
