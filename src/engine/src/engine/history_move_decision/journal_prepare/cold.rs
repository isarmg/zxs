//! Admit only the original selected decision tail; PREPARE remains unpublished.
use super::*;
use crate::journal::Candidate;

impl Engine {
    /// Before recovery writers, bind one original candidate and ordered local
    /// phases. This is identity/denial, not completion or a payload proof.
    pub(in crate::engine) fn admit_selected_move_prepares(
        &mut self,
        records: &crate::engine::recovery_checkpoint::Records,
        _torn_tail: bool,
    ) -> Result<()> {
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: selected move recovery has its original choice");
        let mut observed: Option<PrepareBinding> = None;
        for (sequence, candidates) in records {
            if *sequence <= choice.source_sequence() {
                continue;
            }
            if *sequence < choice.decision_slot() {
                // Ordinary PREPARE consumes a slot even when no decision follows.
                // Preserve such gaps; an accepted competing slot is never ignored.
                if candidates.len() != 1 {
                    return Err(invalid("earlier move gap has conflicting candidates"));
                }
                let mut owners = BTreeSet::new();
                for receipts in candidates.values() {
                    for receipt in receipts {
                        if receipt.record.record_type != RecordType::Prepare
                            || !owners.insert(&receipt.disk_id)
                            || !self.config.disks.iter().any(|disk| {
                                disk.metadata_member
                                    && disk.disk_id == receipt.disk_id
                                    && disk.slot_id == receipt.slot_id
                            })
                        {
                            return Err(invalid(
                                "earlier move slot is not an original unaccepted PREPARE",
                            ));
                        }
                    }
                }
                continue;
            }
            if *sequence != choice.decision_slot() || candidates.len() != 1 {
                return Err(invalid("journal extends beyond the original move slot"));
            }
            for (hash, receipts) in candidates {
                let mut phases = BTreeMap::<String, u8>::new();
                let mut objects = BTreeMap::<u8, String>::new();
                let mut accepted = false;
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
                        .ok_or_else(|| invalid("move record is outside its original membership"))?;
                    let root = &self.disk_resources.get(disk)?.root;
                    let candidate: Candidate =
                        crate::journal::load_object_at(root, disk, "candidate", hash)?;
                    self.verify_candidate_membership(&candidate)?;
                    choice.require_prepare_candidate(&self.config, &candidate)?;
                    let phase = match receipt.record.record_type {
                        RecordType::Prepare => 1,
                        RecordType::Accept => 2,
                        RecordType::Finalize => 3,
                        RecordType::Publication => 4,
                        RecordType::Reprotect
                        | RecordType::MembershipFence
                        | RecordType::MembershipControlBootstrap
                        | RecordType::MembershipJointAccept => {
                            return Err(invalid("selected move slot contains another protocol"));
                        }
                    };
                    let previous = phases.entry(receipt.disk_id.clone()).or_default();
                    if phase != *previous + 1 {
                        return Err(invalid("move journal skips or repeats a decision phase"));
                    }
                    *previous = phase;
                    if (phase == 1 && receipt.record.object_hash != candidate.snapshot_hash)
                        || (phase == 2 && receipt.record.object_hash != *hash)
                    {
                        return Err(invalid(
                            "move journal changed its original candidate/snapshot",
                        ));
                    }
                    if let Some(original) =
                        objects.insert(phase, receipt.record.object_hash.clone())
                        && original != receipt.record.object_hash
                    {
                        return Err(invalid("move members have conflicting decision objects"));
                    }
                    accepted |= phase >= 2;
                    let binding = PrepareBinding {
                        operation: candidate.operation_id,
                        sequence: candidate.sequence,
                        candidate_hash: hash.clone(),
                        snapshot_hash: candidate.snapshot_hash.clone(),
                        decided: false,
                    };
                    if observed.as_ref().is_some_and(|old| old != &binding) {
                        return Err(invalid(
                            "move members disagree on their original selected candidate",
                        ));
                    }
                    observed = Some(binding);
                }
                let latest = phases
                    .values()
                    .copied()
                    .max()
                    .expect("invariant: recorded candidate has a local phase");
                if accepted
                    && (phases.len() != self.members().len()
                        || phases.values().any(|phase| *phase < latest - 1))
                {
                    return Err(invalid(
                        "move advanced without every original member completing the preceding phase",
                    ));
                }
                if accepted {
                    observed
                        .as_mut()
                        .expect("invariant: accepted receipt has a binding")
                        .mark_decided();
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

    /// The retained choice rules out a new candidate. Reconstruct every original
    /// unpublished input before repairing an incomplete last frame, preserving
    /// the complete valid prefix and never advancing to ACCEPT in this method.
    pub(in crate::engine) fn repair_selected_move_prepare_tail(&mut self) -> Result<()> {
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: selected move")
            .clone();
        let original = self
            .inspect_file_history_move_recovery()?
            .ok_or_else(|| invalid("torn selected move has no original recovery envelope"))?;
        if !choice.matches_recovery(&original) {
            return Err(invalid("torn move envelope changed"));
        }
        let cancellation = HistoryWindowCancellation::default();
        let projection = self.read_original_move_projection(
            original.operation_id(),
            original.digest(),
            choice.principal(),
            &cancellation,
        )?;
        let binding = self.ready_move_prepare(&projection, original.digest(), &cancellation)?;
        self.next_slot = binding.sequence.checked_add(1).ok_or_else(limit)?;
        self.history_move_prepare = Some(binding.clone());
        for writer in self.journals.values_mut() {
            writer.verify_retained_name()?;
            writer.repair_torn_tail()?;
            writer.verify_retained_name()?;
        }
        self.observe_move_prepares(binding, &cancellation)?;
        Ok(())
    }

    /// Run after workspace loading and before disposable index rebuilding. Cold
    /// admission cannot repair envelopes, bodies, pending pairs or journal votes.
    pub(in crate::engine) fn verify_selected_move_prepare(&self) -> Result<()> {
        if self.blocked
            || self
                .history_move_prepare
                .as_ref()
                .is_none_or(PrepareBinding::is_decided)
        {
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
