//! Ordinary PREPARE for the original selected candidate. No ACCEPT, decision,
//! active install or permission to advance either namespace is created here.
use super::*;
use crate::journal::{Record, RecordType};

mod cold;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::engine) struct PrepareBinding {
    operation: OperationId,
    sequence: u64,
    candidate_hash: String,
    snapshot_hash: String,
}

impl PrepareBinding {
    pub(in crate::engine) fn matches(&self, operation: OperationId, slot: u64) -> bool {
        self.operation == operation && self.sequence == slot
    }

    fn require_record(&self, record: &Record) -> Result<()> {
        if record.sequence != self.sequence
            || record.record_type != RecordType::Prepare
            || record.candidate_hash != self.candidate_hash
            || record.object_hash != self.snapshot_hash
        {
            return Err(invalid(
                "selected move journal must retain only the original unaccepted PREPARE",
            ));
        }
        Ok(())
    }
}

/// A count of real PREPARE frames, never votes or a successful move result.
#[derive(Debug)]
pub struct FileHistoryMoveJournalPreparation {
    binding: PrepareBinding,
    metadata_members: usize,
    prepared_members: usize,
}

impl FileHistoryMoveJournalPreparation {
    pub fn operation_id(&self) -> OperationId {
        self.binding.operation
    }
    pub fn sequence(&self) -> u64 {
        self.binding.sequence
    }
    pub fn candidate_hash(&self) -> &str {
        &self.binding.candidate_hash
    }
    pub fn snapshot_hash(&self) -> &str {
        &self.binding.snapshot_hash
    }
    pub fn metadata_members(&self) -> usize {
        self.metadata_members
    }
    pub fn prepared_members(&self) -> usize {
        self.prepared_members
    }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] {
        &OUTSTANDING
    }
}

impl Engine {
    fn ready_move_prepare(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<PrepareBinding> {
        // Both observers reconstruct from the original source. Counts or one
        // surviving candidate object cannot replace the full ordinary closure.
        let ordinary = self.read_file_history_move_ordinary(projection, recovery, cancellation)?;
        let candidate =
            self.read_file_history_move_candidate(projection, recovery, cancellation)?;
        if ordinary.candidate_hash() != candidate.candidate_hash() {
            return Err(invalid(
                "move preparation changed between complete observations",
            ));
        }
        let binding = PrepareBinding {
            operation: projection.operation_id(),
            sequence: candidate.sequence(),
            candidate_hash: candidate.candidate_hash().into(),
            snapshot_hash: candidate.snapshot_hash().into(),
        };
        if self
            .history_move_prepare
            .as_ref()
            .is_some_and(|original| original != &binding)
        {
            return Err(invalid(
                "move PREPARE cannot select a replacement candidate",
            ));
        }
        Ok(binding)
    }

    fn observe_move_prepares(
        &self,
        binding: PrepareBinding,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveJournalPreparation> {
        let members = self.members();
        let mut prepared_members = 0;
        for disk in &members {
            checkpoint(cancellation)?;
            let writer = self.journals.get(&disk.disk_id).ok_or_else(|| {
                invalid("move PREPARE needs every original held metadata journal")
            })?;
            if writer.needs_recovery() {
                return Err(invalid(
                    "move journal needs exact recovery before another PREPARE",
                ));
            }
            writer.verify_retained_name()?;
            let handle = self.disk_resources.get(disk)?;
            let lease = handle.root.admit_io()?;
            let seal = validate_seal_observed(&self.config, disk, &mut |_| lease.check_running())?;
            validate_stored_seal(&handle.seal, &seal)?;
            let scan = crate::journal::scan_configured_at(&handle.root, disk, &self.config)?;
            if scan.torn_tail {
                return Err(invalid(
                    "torn move journal is retained without automatic tail repair",
                ));
            }
            let start = scan.records.last().map_or(0, |(last, _)| {
                scan.records
                    .partition_point(|(record, _)| record.sequence < last.sequence)
            });
            if writer.records() != &scan.records[start..] {
                return Err(invalid(
                    "move journal bytes differ from the original held writer tail",
                ));
            }
            let mut prepared = false;
            for (record, _) in &scan.records {
                if record.sequence < binding.sequence {
                    continue;
                }
                binding.require_record(record)?;
                if std::mem::replace(&mut prepared, true) {
                    return Err(invalid(
                        "duplicate original move PREPARE on one metadata member",
                    ));
                }
            }
            prepared_members += usize::from(prepared);
            writer.verify_retained_name()?;
            lease.check_running()?;
        }
        self.require_move_choice_baseline()?;
        Ok(FileHistoryMoveJournalPreparation {
            binding,
            metadata_members: members.len(),
            prepared_members,
        })
    }

    /// Append only the selected ordinary candidate's PREPARE on every original
    /// metadata member after complete body/database/input verification. Partial
    /// writes retain the choice fence and can only resume this candidate.
    pub fn prepare_file_history_move_journal(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveJournalPreparation> {
        let binding = self.ready_move_prepare(projection, recovery, cancellation)?;
        self.observe_move_prepares(binding.clone(), cancellation)?;
        let next_slot = binding.sequence.checked_add(1).ok_or_else(limit)?;
        let members = self.members();
        let physical = self
            .config
            .metadata_reserve_bytes
            .checked_add(4096)
            .ok_or_else(limit)?;
        for disk in &members {
            checkpoint(cancellation)?;
            let handle = self.disk_resources.get(disk)?;
            let configured = self
                .used_bytes(&disk.slot_id)
                .checked_add(4096)
                .ok_or_else(limit)?;
            if handle.root.available()? < physical
                || disk
                    .capacity_bytes
                    .is_some_and(|capacity| configured > capacity)
            {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "move PREPARE would consume the metadata reserve",
                ));
            }
        }
        checkpoint(cancellation)?;
        // Consume the ordinary counter once. Original move reconstruction uses
        // its selected slot, never a new operation/clock or a rewind of this one.
        self.history_move_prepare = Some(binding.clone());
        self.next_slot = next_slot;
        let mut finish = || -> Result<FileHistoryMoveJournalPreparation> {
            for disk in &members {
                checkpoint(cancellation)?;
                self.append_journal(
                    disk,
                    RecordType::Prepare,
                    binding.sequence,
                    &binding.candidate_hash,
                    &binding.snapshot_hash,
                )?;
            }
            let report = self.read_file_history_move_journal(projection, recovery, cancellation)?;
            if report.prepared_members != report.metadata_members {
                return Err(invalid(
                    "move PREPARE did not reach every original metadata member",
                ));
            }
            Ok(report)
        };
        finish().map_err(|error| error.context(ErrorCode::RecoveryRequired,
            "move PREPARE may be partial; keep the original choice and all source/target data, no ACCEPT or move success exists"))
    }

    /// Read actual journal frames and all original preparation material. Missing
    /// PREPARE frames are counted, never filled or interpreted as accepted votes.
    pub fn read_file_history_move_journal(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveJournalPreparation> {
        let binding = self.ready_move_prepare(projection, recovery, cancellation)?;
        self.observe_move_prepares(binding, cancellation)
    }
}
