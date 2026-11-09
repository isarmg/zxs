//! Explicit preparation/resume only. No path reaches the publication journal.
use super::*;

pub(super) fn metadata_members(engine: &Engine) -> Result<Vec<DiskRegistrationIdentity>> {
    let registered: BTreeMap<_, _> = engine
        .registered_disks()
        .map(|disk| (disk.slot_id, disk))
        .collect();
    let mut members = Vec::new();
    for disk in engine
        .config
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
    {
        let current = registered.get(disk.slot_id.as_str()).ok_or_else(|| {
            stale("move recovery requires every original metadata member to remain registered")
        })?;
        if current.registration_mode != RegistrationMode::Enabled
            || !current.metadata_member
            || current.pool_id != engine.config.pool_id
            || current.disk_id != disk.disk_id
            || current.binding_generation != disk.binding_generation
        {
            return Err(stale("move recovery metadata membership changed"));
        }
        members.push(current.identity());
    }
    members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
    if members.is_empty() || members.len() > MAX_ITEMS {
        return Err(stale(
            "move recovery requires a bounded nonempty metadata membership",
        ));
    }
    Ok(members)
}

fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "move recovery preparation cancelled",
        ));
    }
    Ok(())
}

fn retained_failure(error: Error) -> Error {
    error.context(
        ErrorCode::RecoveryRequired,
        "original move preparation may be retained; inspect or resume the same operation, no move decision has been made",
    )
}

/// Missing slots may be created under this original record. A partial directory,
/// different record, changed binding or unreadable replica is never replaced.
fn replicate(
    engine: &Engine,
    record: &RecoveryRecord,
    cancellation: &HistoryWindowCancellation,
) -> Result<()> {
    let prepare = || -> Result<()> {
        for member in &record.metadata_members {
            checkpoint(cancellation)?;
            persist::Store::member(engine, member)?
                .save(record, engine.config.metadata_reserve_bytes)?;
        }
        // Establish a final read barrier over all expected exact replicas.
        for member in &record.metadata_members {
            checkpoint(cancellation)?;
            let store = persist::Store::member(engine, member)?;
            let stored = store.read()?.ok_or_else(|| {
                stale("move recovery metadata replica disappeared after synchronization")
            })?;
            if stored.record != *record {
                return Err(corrupt("move recovery metadata replicas disagree"));
            }
            stored.check_names(&store)?;
        }
        Ok(())
    };
    prepare().map_err(|error| {
        error.context(
            ErrorCode::RecoveryRequired,
            "move recovery preparation may be retained on only part of the original membership; source remains unchanged, resume the same operation",
        )
    })
}

impl Engine {
    /// Read-only original-envelope barrier for target staging. Rechecking the
    /// proposed State alone cannot substitute for every retained member copy.
    pub(crate) fn verify_file_history_move_recovery(
        &self,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        checkpoint(cancellation)?;
        self.require_unfenced_membership()?;
        self.recheck_file_history_move_projection(projection, cancellation)?;
        let expected = RecoveryRecord::capture(self, projection)?;
        if &expected.digest()? != reviewed {
            return Err(stale(
                "target preparation differs from the reviewed joint envelope",
            ));
        }
        let system = persist::Store::system(self)?;
        let original = system
            .read()?
            .ok_or_else(|| stale("target preparation has no original system recovery envelope"))?;
        if original.record != expected {
            return Err(stale(
                "target preparation differs from the original system envelope",
            ));
        }
        for member in &expected.metadata_members {
            checkpoint(cancellation)?;
            let store = persist::Store::member(self, member)?;
            let replica = store.read()?.ok_or_else(|| {
                stale("target preparation requires every original member envelope")
            })?;
            if replica.record != expected {
                return Err(corrupt("target preparation member envelopes disagree"));
            }
            replica.check_names(&store)?;
        }
        original.check_names(&system)?;
        Ok(())
    }

    /// Retain the exact joint metadata preparation on the system root and all
    /// configured metadata members. This is not PREPARE/ACCEPT in the journal,
    /// does not reserve the operation/slot and cannot publish either namespace.
    pub fn save_file_history_move_recovery(
        &mut self,
        projection: &FileHistoryMoveProjection,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveRecoveryInfo> {
        checkpoint(cancellation)?;
        self.require_unfenced_membership()?;
        self.recheck_file_history_move_projection(projection, cancellation)?;
        let record = RecoveryRecord::capture(self, projection)?;
        record.validate()?;
        let system = persist::Store::system(self)?;
        system.save(&record, self.config.metadata_reserve_bytes)?;
        let finish = || -> Result<FileHistoryMoveRecoveryInfo> {
            replicate(self, &record, cancellation)?;
            let retained = system.read()?.ok_or_else(|| {
                stale("move recovery system envelope disappeared before the final barrier")
            })?;
            if retained.record != record {
                return Err(corrupt(
                    "move recovery system envelope changed during replication",
                ));
            }
            retained.check_names(&system)?;
            self.recheck_file_history_move_projection(projection, cancellation)?;
            record.info()
        };
        finish().map_err(retained_failure)
    }

    /// Inspect the bounded system envelope only. The expected member count is
    /// not a health report, quorum, target proof or operation-success receipt.
    pub fn inspect_file_history_move_recovery(
        &self,
    ) -> Result<Option<FileHistoryMoveRecoveryInfo>> {
        persist::Store::system(self)?
            .read()?
            .map(|observed| observed.record.info())
            .transpose()
    }

    /// Recreate the SAME joint projection after restart, using its original
    /// clock, slot, route and identities. Complete only missing member envelopes
    /// for that exact original request. Never replan, replace partial material,
    /// create a body/D or promote preparation into a successful move.
    pub fn resume_file_history_move_recovery(
        &mut self,
        operation: OperationId,
        reviewed: &FileHistoryMoveRecoveryDigest,
        principal: &str,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveProjection> {
        checkpoint(cancellation)?;
        Self::require_actor(principal)?;
        self.require_move_source_read()?;
        self.require_selected_move_operation(operation)?;
        self.require_unfenced_membership()?;
        let system = persist::Store::system(self)?;
        let observed = system.read()?.ok_or_else(|| {
            Error::new(
                ErrorCode::PlanNotFound,
                "no original joint move envelope is retained",
            )
        })?;
        let record = &observed.record;
        if record.operation_id != operation
            || record.principal != principal
            || &record.digest()? != reviewed
        {
            return Err(stale(
                "move recovery requires the original operation, actor and reviewed envelope",
            ));
        }
        record.require_current(self)?;
        let capture = self.capture_retained_file_history_move(
            FileHistoryMoveRequest {
                namespace: &record.namespace,
                source_file_id: &HistoryFileId::parse(&record.source_file_id)?,
            },
            cancellation,
        )?;
        let projection = self.replay_file_history_move_projection(
            capture,
            crate::business::FileHistoryMoveReplay {
                principal: principal.into(),
                operation,
                sequence: record.decision_slot,
                route: record.route.clone(),
            },
            cancellation,
        )?;
        if RecoveryRecord::capture(self, &projection)? != *record {
            return Err(stale(
                "the independently rebuilt complete move write set differs from its original envelope",
            ));
        }
        observed.check_names(&system)?;
        // Synchronize the same system bytes too: a prior failure may have
        // installed the name without completing all parent-directory barriers.
        let finish = || -> Result<()> {
            system.save(record, self.config.metadata_reserve_bytes)?;
            replicate(self, record, cancellation)?;
            observed.check_names(&system)?;
            self.recheck_file_history_move_projection(&projection, cancellation)?;
            Ok(())
        };
        finish().map_err(retained_failure)?;
        Ok(projection)
    }
}
