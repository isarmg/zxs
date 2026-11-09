//! One unpublished native operation, never a fabricated copy + delete pair.
//! This completes receipt-shaped logical inputs, not publication authority.
use super::*;
use data_serialization::{Deserialize, Serialize};
use storage_contract::{CommitState, CurrentHealth, Event, OperationResult, ProtectionAtPublish};

pub(crate) const KIND: &str = "file.history.move";

mod admission;
pub(crate) use admission::{
    admit_deletions, admit_displaced, original_move_intent, replay_selected_move,
    retained_deletion, verify_current_move_transition,
};
#[cfg(test)]
use admission::admit_source_deletion;

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Request<'a> {
    namespace: &'a FileHistoryCopyRequest,
    source_file_id: &'a str,
    #[serialization(skip_serializing_if = "Option::is_none")]
    replacement: Option<&'a FileHistoryMoveReplacement>,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
enum SlotState {
    CompleteVerified,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct Slot {
    slot_id: String,
    copy_level: u32,
    disk_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
    state: SlotState,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct Version {
    version_id: String,
    primary_slot_id: String,
    target_copies: u32,
    required_slots: Vec<String>,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
    slots: Vec<Slot>,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct ResultBody<'a> {
    move_request: Request<'a>,
    move_route: &'a CopyRoute,
    source_file_id: &'a str,
    file_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    source_records_hash: String,
    deletion_node: &'a FileDeletionNode,
    #[serialization(skip_serializing_if = "Option::is_none")]
    replacement: Option<&'a replacement::Evidence>,
    retained_lifecycle: &'a CopiedLifecycle,
    version_ids: Vec<String>,
    version_mapping: &'a BTreeMap<String, String>,
    branch_mapping: &'a BTreeMap<String, String>,
    branch_id: &'a str,
    history_tree_id: &'a str,
    commit_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    branch_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    timestamp: u64,
    #[serialization(with = "storage_contract::u64_string")]
    membership_epoch: u64,
    recovery_digest: String,
    decision_inputs_digest: String,
    version_protection: Vec<Version>,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Envelope<'a> {
    format: &'static str,
    /// Prospective protocol data, not an emitted or accepted OperationResult.
    proposed_operation: &'a OperationResult,
    source_file_database_records_hash: &'a str,
    target_file_database_records_hash: &'a str,
    #[serialization(skip_serializing_if = "Option::is_none")]
    replacement_file_database_records_hash: Option<&'a str>,
}

/// Opaque internal inputs. No caller receives the proposed OperationResult or
/// State, and ordinary publication explicitly rejects this unclosed operation.
pub(crate) struct NativeMoveDraft {
    operation: OperationId,
    envelope: Vec<u8>,
    state: Vec<u8>,
    proposed_state: State,
    source_records_hash: String,
    target_records_hash: String,
    replacement_records_hash: Option<String>,
    request_digest: String,
    digest: String,
}
impl NativeMoveDraft {
    pub(crate) fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub(crate) fn envelope(&self) -> &[u8] {
        &self.envelope
    }
    pub(crate) fn state(&self) -> &[u8] {
        &self.state
    }
    /// This remains an unaccepted draft. It cannot construct AdmittedState or
    /// authorize publication, including when used by a disposable SQL encoder.
    pub(crate) fn proposed_state(&self) -> &State {
        &self.proposed_state
    }
    pub(crate) fn source_records_hash(&self) -> &str {
        &self.source_records_hash
    }
    pub(crate) fn target_records_hash(&self) -> &str {
        &self.target_records_hash
    }
    pub(crate) fn replacement_records_hash(&self) -> Option<&str> {
        self.replacement_records_hash.as_deref()
    }
    pub(crate) fn request_digest(&self) -> &str {
        &self.request_digest
    }
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
}

/// Prospective successful snapshot for one frozen candidate. This is not an
/// admitted State or a returned receipt; ordinary publication still rejects it.
pub(crate) struct NativeMoveCandidate {
    state: State,
    operation: OperationId,
}
impl NativeMoveCandidate {
    pub(crate) fn into_state(self) -> State {
        self.state
    }
    pub(crate) fn state(&self) -> &State {
        &self.state
    }
    pub(crate) fn result(&self) -> &OperationResult {
        &self.state.operations[&self.operation]
    }
}
impl NativeMoveDraft {
    pub(crate) fn candidate_draft(&self) -> Result<NativeMoveCandidate> {
        let mut state = self.proposed_state.clone();
        let result = state
            .operations
            .get_mut(&self.operation)
            .expect("invariant: native move draft contains its one operation");
        // Ordinary publication also freezes its prospective result before the
        // first vote. The result remains private until a real joint publication.
        result.commit_state = CommitState::Published;
        result.protection_at_publish = ProtectionAtPublish::TargetMet;
        result.current_health = CurrentHealth::Healthy;
        history_copy::retained::bounded(&state)?;
        Ok(NativeMoveCandidate {
            state,
            operation: self.operation,
        })
    }
}

fn versions(engine: &Engine, projection: &FileHistoryMoveProjection) -> Result<Vec<Version>> {
    projection
        .version_mapping()
        .values()
        .map(|id| {
            let placement = &projection.writes.next.placements[id];
            let slots = placement
                .route_slot_ids
                .iter()
                .map(|slot| {
                    let disk = engine.disk(slot)?;
                    if placement.binding_generations.get(slot) != Some(&disk.binding_generation) {
                        return Err(changed());
                    }
                    Ok(Slot {
                        slot_id: slot.clone(),
                        copy_level: disk.level,
                        disk_id: disk.disk_id.clone(),
                        binding_generation: disk.binding_generation,
                        state: SlotState::CompleteVerified,
                    })
                })
                .collect::<Result<_>>()?;
            Ok(Version {
                version_id: id.clone(),
                primary_slot_id: placement.primary_slot_id.clone(),
                target_copies: placement.target_copies,
                required_slots: placement.route_slot_ids.clone(),
                topology_generation: placement.topology_generation,
                slots,
            })
        })
        .collect()
}

impl FileHistoryMoveProjection {
    /// The Engine caller has independently verified every original input and
    /// target FULL. That observation permits a template, not a success receipt.
    pub(crate) fn native_draft(
        &self,
        engine: &Engine,
        recovery: &FileHistoryMoveRecoveryDigest,
        inputs: &FileHistoryMoveDecisionDigest,
    ) -> Result<NativeMoveDraft> {
        let request = &self.capture.namespace;
        let target = &self.writes.next.repositories[&request.target_repository_id];
        let branch = &self.writes.next.branches[&target.main_branch_id];
        let input = Request {
            namespace: request,
            source_file_id: self.capture.source_file_id.as_str(),
            replacement: self.capture.replacement(),
        };
        let request_digest = Engine::native_request_digest(KIND, &self.principal, &json!(input))?;
        let result = ResultBody {
            move_request: input,
            move_route: &self.route,
            source_file_id: self.capture.source_file_id.as_str(),
            file_id: self.target_file_id().as_str(),
            source_sequence: self.capture.source_sequence,
            source_records_hash: self.capture.source_records_digest.to_string(),
            deletion_node: self.proposed_source_deletion(),
            replacement: self.writes.replacement.as_ref(),
            retained_lifecycle: &self.writes.lifecycle,
            version_ids: self.writes.version_mapping.values().cloned().collect(),
            version_mapping: &self.writes.version_mapping,
            branch_mapping: &self.writes.branch_mapping,
            branch_id: &target.main_branch_id,
            history_tree_id: &target.history_tree_id,
            commit_id: &branch.head_commit_id,
            branch_generation: branch.generation,
            timestamp: self.route.created_at,
            membership_epoch: engine.config.membership_epoch(),
            recovery_digest: recovery.to_string(),
            decision_inputs_digest: inputs.to_string(),
            version_protection: versions(engine, self)?,
        };
        history_copy::retained::bounded(&result)?;
        let receipt = OperationResult {
            operation_id: self.operation,
            kind: KIND.into(),
            commit_id: Some(branch.head_commit_id.clone()),
            history_tree_id: Some(target.history_tree_id.clone()),
            branch_id: Some(target.main_branch_id.clone()),
            // A prepared template is not a historical success. Final publication
            // must supply its own proven outcome; this draft claims none.
            commit_state: CommitState::Staging,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Unknown,
            request_digest: request_digest.clone(),
            sequence: self.proposed_sequence(),
            result: json_codec::to_value(result)?,
        };
        let mut next = self.writes.next.clone();
        next.operations.insert(self.operation, receipt.clone());
        next.operation_principals
            .insert(self.operation, self.principal.clone());
        next.events.push(Event {
            sequence: self.proposed_sequence(),
            operation_id: self.operation,
            kind: KIND.into(),
            object_id: branch.head_commit_id.clone(),
        });
        // Use the ordinary receipt-owner projection for BOTH files. The draft's
        // one operation must own the source D and target imported audit without
        // the special pre-receipt FILE encoder or separate copy/delete receipts.
        let source = crate::container_storage::file_database::capture_copy(
            &next,
            self.capture.source_file_id.as_str(),
            &self.proposed_source_deletion().history_tree_id,
        )?;
        let target_database = crate::container_storage::file_database::capture_copy(
            &next,
            self.target_file_id().as_str(),
            &target.history_tree_id,
        )?;
        let replacement_records_hash = self
            .proposed_replacement_deletion()
            .map(|node| {
                crate::container_storage::file_database::capture_copy(
                    &next,
                    &node.file_id,
                    &node.history_tree_id,
                )
                .map(|database| database.records_hash)
            })
            .transpose()?;
        history_copy::retained::bounded(&next)?;
        let state = crate::disk::encode_metadata(&next, 12 * 1024 * 1024)?;
        let envelope = crate::disk::encode_metadata(
            &Envelope {
                format: "file-history-move-unaccepted-native-operation",
                proposed_operation: &receipt,
                source_file_database_records_hash: &source.records_hash,
                target_file_database_records_hash: &target_database.records_hash,
                replacement_file_database_records_hash: replacement_records_hash.as_deref(),
            },
            12 * 1024 * 1024,
        )?;
        let mut digest = blake3::Hasher::new();
        digest.update(b"file-history-move-native-draft\0");
        digest.update(blake3::hash(&envelope).as_bytes());
        digest.update(blake3::hash(&state).as_bytes());
        Ok(NativeMoveDraft {
            operation: self.operation,
            envelope,
            state,
            proposed_state: next,
            source_records_hash: source.records_hash,
            target_records_hash: target_database.records_hash,
            replacement_records_hash,
            request_digest,
            digest: digest.finalize().to_hex().to_string(),
        })
    }
}

/// An unaccepted draft is not a general metadata operation permission. Keep
/// this barrier ahead of any payload/name I/O until database and recovery
/// closure supply their own private, validated publication capability.
pub(crate) fn reject_unclosed_publication(
    previous: &State,
    next: &State,
    result: &OperationResult,
) -> Result<()> {
    let reject = || {
        Error::new(
            ErrorCode::UnsupportedOperation,
            "native move requires its dedicated joint execution and recovery path; unaccepted inputs cannot use generic publication",
        )
    };
    if result.kind == KIND {
        return Err(reject());
    }
    let inspect = |id: &OperationId| -> Result<()> {
        let before = previous.operations.get(id);
        let after = next.operations.get(id);
        if before != after
            && before
                .into_iter()
                .chain(after)
                .any(|receipt| receipt.kind == KIND)
        {
            return Err(reject());
        }
        Ok(())
    };
    match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys) => keys.iter().try_for_each(inspect),
        storage_contract::MapChanges::All => {
            for id in previous.operations.keys().chain(next.operations.keys()) {
                inspect(id)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/business/history_move_native_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/business/history_move_native_lifecycle_tests.rs"]
mod lifecycle_tests;
