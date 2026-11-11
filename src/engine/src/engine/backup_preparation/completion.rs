//! Finish the original in-process handoff after Current has been installed.
//! Later decisions keep their own stops; a retry never clears them by receipt.
use super::*;

impl Engine {
    pub(in crate::engine) fn complete_backup_serving_open(&mut self) -> Result<OperationResult> {
        self.restore_backup_current()?;
        let admitted = self.backup_serving.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup serving open lacks its native completion",
            )
        })?;
        if self.state.operations.get(&admitted.barrier.operation) != Some(&admitted.result) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup replay did not retain its exact original operation result",
            ));
        }
        let barrier = admitted.barrier.clone();
        let result = admitted.result.clone();
        self.reopen_backup_system_index(&barrier)?;
        // SYSTEM retains the Genesis discovery anchor. The authenticated
        // bootstrap extension selects the native Backup fanout on restart.
        let discovery = self
            .config
            .generation()
            .expect("invariant: completed Backup serving has native generation authority")
            .original();
        write_json(&self.root.join("config.json"), discovery, true)?;
        if self.retire_completed_backup_source_stop(&barrier, &result)? {
            self.restore_published_disk_histories();
        }
        Ok(result)
    }

    fn retire_completed_backup_source_stop(
        &mut self,
        barrier: &ServingBarrier,
        result: &OperationResult,
    ) -> Result<bool> {
        let Some(fence) = self.backup_cutover_fence_status() else {
            return Ok(false);
        };
        if fence.operation_id != result.operation_id
            || fence.transition_hash != result.request_digest
        {
            return Ok(false);
        }
        // The native first result binds the exact original BackupCutover
        // intent, including its actor. No successor publication can run while
        // that stop remains owned by this unfinished first handoff.
        barrier.require_current(self)?;
        self.require_no_commit_preparation()?;
        if !self.blocked
            || !fence.all_members_fenced()
            || self.state.sequence != barrier.first_sequence
            || self.next_slot
                != barrier.first_sequence.checked_add(1).ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "Backup first slot is exhausted",
                    )
                })?
            || self.pending_membership.is_some()
            || self.pending_disk_exit.is_some()
            || self.history_move_choice.is_some()
            || self.history_move_prepare.is_some()
            || self.journals.len() != self.config.metadata_members().len()
            || self
                .journals
                .values()
                .any(crate::journal::JournalWriter::needs_recovery)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Backup source stop cannot retire across another pending decision or changed first boundary",
            ));
        }
        let _activity = self._locks.activity()?;
        for disk in &self.config.disks {
            let handle = self.disk_resources.get(disk)?;
            crate::engine::provenance::validate_owned_seal(
                &self.disk_resources,
                disk,
                &self.metadata_integrity,
            )?;
            self.config.verify_root_membership(&handle.root, disk)?;
            self._locks.verify_disk(disk, &handle.root)?;
        }
        for journal in self.journals.values() {
            journal.verify_retained_name()?;
        }
        // Persistent source-fence evidence remains untouched. Only this exact
        // old in-memory stop is retired after the final fallible handoff steps.
        self.membership_fence = None;
        self.blocked = false;
        Ok(true)
    }
}
