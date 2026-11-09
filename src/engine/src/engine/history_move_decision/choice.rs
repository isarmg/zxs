//! One immutable input/slot selection. This is a deny fence, never an ACCEPT,
//! quorum, operation receipt, namespace publication or permission to erase data.
use super::*;
use data_serialization::Deserialize;

mod discovery;

pub(super) const NAME: &str = "joint-choice.json";
const FORMAT: &str = "file-history-move-joint-choice";
const MAX_BYTES: usize = 64 * 1024;
const MAX_PARTICIPANTS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: String,
    operation_id: OperationId,
    principal: String,
    pool_id: String,
    configuration_hash: String,
    source_snapshot: String,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    decision_slot: u64,
    recovery_digest: String,
    decision_inputs_digest: String,
    native_digest: String,
    scoped_digest: String,
    metadata_members: Vec<DiskRegistrationIdentity>,
    participants: Vec<DiskRegistrationIdentity>,
}

/// Validated persisted boundary, distinct from its on-disk record. Bytes stay
/// exact across retries; there is no mutable phase, vote or completion flag.
#[derive(Clone, Eq, PartialEq)]
pub(in crate::engine) struct Choice {
    record: Record,
    bytes: Vec<u8>,
}

/// Diagnostic selection identity. Even a complete replication does not make
/// either end of the move visible or discharge any joint-publication obligation.
#[derive(Debug)]
pub struct FileHistoryMoveJointChoice {
    operation_id: OperationId,
    digest: String,
    source_sequence: u64,
    decision_slot: u64,
    participant_disks: usize,
}
impl FileHistoryMoveJointChoice {
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn source_sequence(&self) -> u64 {
        self.source_sequence
    }
    pub fn decision_slot(&self) -> u64 {
        self.decision_slot
    }
    pub fn participant_disks(&self) -> usize {
        self.participant_disks
    }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] {
        &OUTSTANDING
    }
}

// Genesis retains its exact original bytes. Current choices additionally bind
// the authenticated authority and registry, which Config serialization omits.
fn configuration_hash(config: &ValidatedConfig) -> Result<String> {
    Ok(match config.generation() {
        None => object_hash("file-history-move-config", config)?,
        Some(generation) => object_hash(
            "file-history-move-current-config",
            &json_codec::json!({
                "configuration": config,
                "authority": generation.hash,
                "registry": generation.registry(),
            }),
        )?,
    })
}

impl Choice {
    fn capture(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        scopes: &scopes::Scopes,
    ) -> Result<Self> {
        engine.require_native_move_authority()?;
        let scoped = scopes.report();
        let record = Record {
            format: FORMAT.into(),
            operation_id: projection.operation_id(),
            principal: projection.principal().into(),
            pool_id: engine.config.pool_id.clone(),
            configuration_hash: configuration_hash(&engine.config)?,
            source_snapshot: engine
                .snapshot
                .clone()
                .ok_or_else(|| invalid("move choice needs a published source"))?,
            source_sequence: engine.state.sequence,
            decision_slot: projection.proposed_sequence(),
            recovery_digest: recovery.to_string(),
            decision_inputs_digest: materials.digest.to_string(),
            native_digest: scoped.native_digest().into(),
            scoped_digest: scoped.scoped_digest().into(),
            metadata_members: materials
                .record
                .disks
                .iter()
                .filter(|disk| disk.carries_state())
                .map(|disk| disk.identity.clone())
                .collect(),
            participants: materials
                .record
                .disks
                .iter()
                .map(|disk| disk.identity.clone())
                .collect(),
        };
        record.require_config(&engine.config)?;
        let bytes = encode_metadata(&record, MAX_BYTES)?;
        let choice = Self { record, bytes };
        choice.require_baseline(engine)?;
        Ok(choice)
    }

    fn decode(config: &ValidatedConfig, bytes: &[u8]) -> Result<Self> {
        let record: Record = storage_contract::decode_strict(bytes)?;
        record.require_config(config)?;
        if encode_metadata(&record, MAX_BYTES)? != bytes {
            return Err(invalid(
                "move choice encoding is not the exact canonical record",
            ));
        }
        Ok(Self {
            record,
            bytes: bytes.to_vec(),
        })
    }

    pub(super) fn storage_bytes(&self) -> u64 {
        self.bytes.len() as u64 + 4096
    }

    pub(in crate::engine) fn source_sequence(&self) -> u64 {
        self.record.source_sequence
    }

    fn require_source(&self, engine: &Engine) -> Result<()> {
        if engine.state.sequence != self.record.source_sequence
            || engine.snapshot.as_ref() != Some(&self.record.source_snapshot)
            || engine
                .state
                .operations
                .contains_key(&self.record.operation_id)
        {
            return Err(invalid(
                "retained move choice lost its original published source",
            ));
        }
        Ok(())
    }

    fn require_baseline(&self, engine: &Engine) -> Result<()> {
        self.record.require_config(&engine.config)?;
        self.require_source(engine)?;
        let next_slot = if let Some(prepared) = &engine.history_move_prepare {
            if !prepared.matches(self.record.operation_id, self.record.decision_slot) {
                return Err(invalid(
                    "move PREPARE does not belong to the original choice",
                ));
            }
            self.record.decision_slot.checked_add(1).ok_or_else(limit)?
        } else {
            self.record.decision_slot
        };
        if engine.next_slot != next_slot {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "retained move choice differs from the exact published source/next slot; keep all material",
            ));
        }
        Ok(())
    }

    pub(in crate::engine) fn decision_slot(&self) -> u64 {
        self.record.decision_slot
    }

    pub(super) fn principal(&self) -> &str {
        &self.record.principal
    }

    pub(super) fn matches_recovery(&self, info: &FileHistoryMoveRecoveryInfo) -> bool {
        info.operation_id() == self.record.operation_id
            && info.digest().to_string() == self.record.recovery_digest
            && info.source_sequence() == self.record.source_sequence
            && info.decision_slot() == self.record.decision_slot
    }

    pub(super) fn require_prepare_candidate(
        &self,
        config: &ValidatedConfig,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        self.record.require_config(config)?;
        config.verify_candidate_membership(candidate)?;
        let transaction = object_hash(
            "file-history-move-candidate-transaction",
            &self.report().digest(),
        )?;
        if candidate.pool_id != self.record.pool_id
            || candidate.sequence != self.record.decision_slot
            || candidate.operation_id != self.record.operation_id
            || candidate.previous_snapshot.as_deref() != Some(self.record.source_snapshot.as_str())
            || candidate.transaction_id != format!("tx_{}", &transaction[..32])
            || candidate.generation() != config.generation_hash()
            || candidate.registry()
                != config
                    .generation()
                    .and_then(|generation| generation.registry())
        {
            return Err(invalid(
                "journal PREPARE is not the original selected native move candidate",
            ));
        }
        Ok(())
    }

    pub(super) fn require_native(&self, native: &crate::business::NativeMoveDraft) -> Result<()> {
        if native.operation_id() != self.record.operation_id
            || native.digest() != self.record.native_digest
        {
            return Err(invalid(
                "move recovery does not reproduce the original selected native write set",
            ));
        }
        Ok(())
    }

    pub(super) fn require_archive_choice(&self, directory: &DiskRoot) -> Result<()> {
        let held = HeldRegular::capture(directory, NAME, MAX_BYTES)?
            .ok_or_else(|| invalid("move completion directory lost its original choice"))?;
        if held.bytes() != self.bytes {
            return Err(invalid("move completion choice changed"));
        }
        held.check_running()
    }

    pub(super) fn participants(&self) -> &[DiskRegistrationIdentity] {
        &self.record.participants
    }

    pub(super) fn report(&self) -> FileHistoryMoveJointChoice {
        FileHistoryMoveJointChoice {
            operation_id: self.record.operation_id,
            digest: hash(b"file-history-move-joint-choice", &self.bytes)
                .to_hex()
                .to_string(),
            source_sequence: self.record.source_sequence,
            decision_slot: self.record.decision_slot,
            participant_disks: self.record.participants.len(),
        }
    }
}

impl Record {
    fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "unsupported joint move choice; retain its original bytes",
            ));
        }
        if config.generation().is_some_and(|generation| {
            generation.portable.is_backup()
                || generation.portable.is_foreign()
                || generation.registry().is_none()
        }) || self.pool_id != config.pool_id
            || self.configuration_hash != configuration_hash(config)?
        {
            return Err(invalid(
                "move choice requires its exact original configuration, authority and registry",
            ));
        }
        Engine::require_actor(&self.principal)?;
        for digest in [
            &self.source_snapshot,
            &self.recovery_digest,
            &self.decision_inputs_digest,
            &self.native_digest,
            &self.scoped_digest,
        ] {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid(
                    "move choice digest is not canonical lowercase hexadecimal",
                ));
            }
        }
        if self.source_sequence == 0
            || self.decision_slot <= self.source_sequence
            || self.participants.is_empty()
            || self.participants.len() > MAX_PARTICIPANTS
            || self
                .participants
                .windows(2)
                .any(|p| p[0].disk_id >= p[1].disk_id)
        {
            return Err(invalid(
                "move choice needs a later original decision slot and bounded sorted exact participants",
            ));
        }
        for participant in &self.participants {
            if participant.pool_id != config.pool_id
                || !config.io_disks().any(|disk| {
                    disk.slot_id == participant.slot_id
                        && disk.disk_id == participant.disk_id
                        && disk.binding_generation == participant.binding_generation
                })
            {
                return Err(invalid(
                    "move choice participant is not an original I/O binding",
                ));
            }
        }
        let metadata: Vec<_> = self
            .participants
            .iter()
            .filter(|identity| {
                config
                    .disks
                    .iter()
                    .any(|disk| disk.disk_id == identity.disk_id && disk.metadata_member)
            })
            .cloned()
            .collect();
        if metadata != self.metadata_members
            || metadata.is_empty()
            || metadata.len()
                != config
                    .disks
                    .iter()
                    .filter(|disk| disk.metadata_member)
                    .count()
        {
            return Err(invalid(
                "move choice does not retain the exact full metadata membership",
            ));
        }
        Ok(())
    }
}

impl Engine {
    /// Original move replay keeps its chosen slot after PREPARE consumes the
    /// ordinary next-slot counter. This does not reopen ordinary write access.
    pub(crate) fn move_source_slot(&self) -> u64 {
        self.history_move_choice
            .as_ref()
            .map_or(self.next_slot, Choice::decision_slot)
    }

    pub(crate) fn require_selected_move_operation(&self, operation: OperationId) -> Result<()> {
        if self
            .history_move_intent
            .is_some_and(|original| original != operation)
        {
            return Err(invalid("only the original retained move intent may resume"));
        }
        if let Some(choice) = &self.history_move_choice {
            choice.require_baseline(self)?;
            if choice.record.operation_id != operation {
                return Err(invalid(
                    "only the original selected move operation may resume",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::engine) fn require_move_choice_baseline(&self) -> Result<()> {
        if let Some(choice) = &self.history_move_choice {
            choice.require_baseline(self)?;
        }
        Ok(())
    }

    /// Called after the exact source prefix is recovered. A choice can retain
    /// an unused slot beyond the last physical frame; restore that same slot,
    /// never a fresh decision or an observed-counter rewind.
    pub(in crate::engine) fn restore_selected_move_slot(&mut self) -> Result<()> {
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: selected move retains its original choice")
            .clone();
        if self.history_move_prepare.is_none() {
            choice.require_source(self)?;
            if self.next_slot > choice.decision_slot() {
                return Err(invalid(
                    "journal advanced beyond the unused original move slot",
                ));
            }
            self.next_slot = choice.decision_slot();
        }
        choice.require_baseline(self)
    }

    /// Inspect the in-process deny fence discovered at startup or installed
    /// before writing a choice. Not a claim that all replicas/materials exist.
    pub fn inspect_file_history_move_joint_choice(&self) -> Option<FileHistoryMoveJointChoice> {
        self.history_move_choice.as_ref().map(Choice::report)
    }

    /// Select exactly these independently revalidated FULL/FILE/native/scoped
    /// inputs and next slot on every original participant. Errors after fencing
    /// retain the fence and any bytes; there is no cancel/clear/replan fallback.
    /// This does not write ordinary Candidate/ACCEPT or return move success.
    pub fn select_file_history_move_joint_choice(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveJointChoice> {
        checkpoint(cancellation)?;
        self.require_move_source_read()?;
        self.require_unfenced_membership()?;
        let (materials, native, scopes) =
            self.move_scoped_inputs(projection, recovery, cancellation)?;
        self.verify_move_scopes(
            projection,
            recovery,
            &materials,
            &native,
            &scopes,
            cancellation,
        )?;
        let choice = Choice::capture(self, projection, recovery, &materials, &scopes)?;
        if self
            .history_move_choice
            .as_ref()
            .is_some_and(|original| original != &choice)
        {
            return Err(invalid(
                "move choice cannot replace the original operation or materials",
            ));
        }
        for participant in &materials.record.disks {
            store::Store::require_choice_capacity(
                self,
                &materials,
                participant,
                &native,
                &scopes,
                choice.bytes.len(),
            )?;
        }
        checkpoint(cancellation)?;
        // Fence first, even if the first fsync fails. A partially durable choice
        // can only tighten denial on restart; it is never a voting authority.
        self.history_move_choice = Some(choice.clone());
        let finish = || -> Result<()> {
            store::Store::system(self, &materials, store::Mode::Read)?
                .install_choice(&choice.bytes, cancellation)?;
            for participant in &materials.record.disks {
                checkpoint(cancellation)?;
                store::Store::disk(self, &materials, participant, store::Mode::Read)?
                    .install_choice(&choice.bytes, cancellation)?;
            }
            self.verify_joint_choice(&materials, &choice, cancellation)?;
            self.verify_move_scopes(
                projection,
                recovery,
                &materials,
                &native,
                &scopes,
                cancellation,
            )
        };
        finish().map_err(|error| error.context(ErrorCode::RecoveryRequired,
            "joint move choice may be partially durable; ordinary writes stay fenced, resume only the original materials"))?;
        Ok(choice.report())
    }

    /// Read every exact choice and independently reconstruct/revalidate every
    /// original material. Missing choice replicas are not repaired here.
    pub fn read_file_history_move_joint_choice(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveJointChoice> {
        let (_, _, _, report) = self.selected_move_inputs(projection, recovery, cancellation)?;
        Ok(report)
    }

    pub(super) fn selected_move_inputs(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<(
        Materials,
        crate::business::NativeMoveDraft,
        scopes::Scopes,
        FileHistoryMoveJointChoice,
    )> {
        let (materials, native, scopes) =
            self.move_scoped_inputs(projection, recovery, cancellation)?;
        let expected = Choice::capture(self, projection, recovery, &materials, &scopes)?;
        if self.history_move_choice.as_ref() != Some(&expected) {
            return Err(invalid(
                "the exact move choice has not been selected by this engine",
            ));
        }
        self.verify_joint_choice(&materials, &expected, cancellation)?;
        self.verify_move_scopes(
            projection,
            recovery,
            &materials,
            &native,
            &scopes,
            cancellation,
        )?;
        Ok((materials, native, scopes, expected.report()))
    }

    fn verify_joint_choice(
        &self,
        materials: &Materials,
        choice: &Choice,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        store::Store::system(self, materials, store::Mode::Read)?
            .verify_choice(&choice.bytes, cancellation)?;
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            store::Store::disk(self, materials, participant, store::Mode::Read)?
                .verify_choice(&choice.bytes, cancellation)?;
        }
        choice.require_baseline(self)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/history_move_choice_tests.rs"]
mod tests;
