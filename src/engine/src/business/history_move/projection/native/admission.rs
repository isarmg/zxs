//! One native move owns both its source D and target arrival. This admits a
//! retained receipt's shape, not its journal decision or physical readiness.
use super::*;
use std::collections::BTreeSet;

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct ReadRequest {
    namespace: FileHistoryCopyRequest,
    source_file_id: String,
}

/// Owned disk boundary for the borrowed writer's exact ResultBody schema.
/// Unknown or missing fields are rejected, not interpreted as an older format.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct ReadBody {
    move_request: ReadRequest,
    move_route: CopyRoute,
    source_file_id: String,
    file_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    source_records_hash: String,
    deletion_node: FileDeletionNode,
    retained_lifecycle: CopiedLifecycle,
    version_ids: Vec<String>,
    version_mapping: BTreeMap<String, String>,
    branch_mapping: BTreeMap<String, String>,
    branch_id: String,
    history_tree_id: String,
    commit_id: String,
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

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "native move source deletion differs from its one original receipt and target arrival",
    )
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    Ok(())
}

fn mapping(operation: OperationId, values: &BTreeMap<String, String>, prefix: &str) -> Result<()> {
    if values.is_empty() {
        return Err(invalid());
    }
    for (source, target) in values {
        storage_contract::validate_id(source, prefix)?;
        if target == source || *target != history_copy::identity(operation, source)? {
            return Err(invalid());
        }
    }
    Ok(())
}

impl ReadBody {
    fn require_versions(&self, operation: OperationId) -> Result<()> {
        mapping(operation, &self.version_mapping, "ver")?;
        mapping(operation, &self.branch_mapping, "br")?;
        let route = &self.move_route;
        if self.version_ids.iter().ne(self.version_mapping.values())
            || self
                .branch_mapping
                .keys()
                .ne(self.deletion_node.branch_heads.keys())
            || self.version_protection.len() != self.version_ids.len()
            || route.slots.is_empty()
            || route.slots.len() > 32
            || route.slots.iter().collect::<BTreeSet<_>>().len() != route.slots.len()
            || route.bindings.len() != route.slots.len()
            || route.slots.iter().any(|slot| {
                slot.is_empty()
                    || !route
                        .bindings
                        .get(slot)
                        .is_some_and(|generation| *generation > 0)
            })
        {
            return Err(invalid());
        }
        // Exact nonempty vectors above establish a first record for the common
        // historical route. No current Config/placement is substituted for it.
        let first = &self.version_protection[0];
        for (version, id) in self.version_protection.iter().zip(&self.version_ids) {
            if version.version_id != *id
                || version.primary_slot_id != route.slots[0]
                || version.target_copies as usize != route.slots.len()
                || version.required_slots != route.slots
                || version.topology_generation != first.topology_generation
                || version.slots != first.slots
                || version.slots.len() != route.slots.len()
            {
                return Err(invalid());
            }
        }
        let mut disks = BTreeSet::new();
        for (index, (slot, expected)) in first.slots.iter().zip(&route.slots).enumerate() {
            storage_contract::validate_id(&slot.disk_id, "disk")?;
            if slot.slot_id != *expected
                || slot.copy_level as usize != index + 1
                || Some(&slot.binding_generation) != route.bindings.get(expected)
                || !disks.insert(&slot.disk_id)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn require_arrival(
        &self,
        state: &State,
        receipt: &OperationResult,
        principal: &str,
    ) -> Result<()> {
        let request = &self.move_request.namespace;
        let source = state
            .repositories
            .get(&request.source_repository_id)
            .ok_or_else(invalid)?;
        let target = state
            .repositories
            .get(&request.target_repository_id)
            .ok_or_else(invalid)?;
        let arrival = state.commits.get(&self.commit_id).ok_or_else(invalid)?;
        let previous = state
            .commits
            .get(&request.expected_target_head)
            .ok_or_else(invalid)?;
        if request.source_repository_id == request.target_repository_id
            || !source.file_manager_root
            || !target.file_manager_root
            || previous.history_tree_id != target.history_tree_id
            || previous.sequence >= receipt.sequence
            || source.history_tree_id == target.history_tree_id
            || source.history_tree_id != self.deletion_node.history_tree_id
            || target.history_tree_id != self.history_tree_id
            || target.main_branch_id != self.branch_id
            || arrival.commit_id != self.commit_id
            || arrival.history_tree_id != self.history_tree_id
            || arrival.operation_id != receipt.operation_id
            || arrival.sequence != receipt.sequence
            || arrival.author != principal
            || arrival.kind != "file_history_move"
            || arrival.parent_commit_ids != [request.expected_target_head.clone()]
        {
            return Err(invalid());
        }
        let target_path = name_policy::PortablePath::parse(&request.target_name)?;
        let _ = super::super::super::target_path::ancestors(&previous.entries, &target_path)
            .map_err(|_| invalid())?;
        let path = name_policy::LookupPath::from(&target_path);
        let entry =
            namespace::find_unordered_key(&arrival.entries, path.key()).ok_or_else(invalid)?;
        if arrival.entries.len() != previous.entries.len() + 1
            || arrival
                .entries
                .iter()
                .filter(|entry| entry.lookup_key() != path.key())
                .ne(previous.entries.iter())
            || namespace::find_unordered_key(&previous.entries, path.key()).is_some()
            || entry.kind != NamespaceEntryKind::File
            || entry.file_id != self.file_id
            || entry.path() != request.target_name
            || !self
                .version_mapping
                .values()
                .any(|version| version == &entry.version_id)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(crate) fn admit_source_deletion(
    state: &State,
    receipt: &OperationResult,
) -> Result<FileDeletionNode> {
    if receipt.kind != KIND
        || receipt.sequence == 0
        || receipt.sequence > state.sequence
        || receipt.commit_state != CommitState::Published
        || receipt.protection_at_publish != ProtectionAtPublish::TargetMet
        || receipt.current_health != CurrentHealth::Healthy
    {
        return Err(invalid());
    }
    let principal = state
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    Engine::require_actor(principal)?;
    history_copy::retained::require_event_budget(
        receipt
            .result
            .get("retained_lifecycle")
            .ok_or_else(invalid)?,
    )?;
    history_copy::retained::bounded(&receipt.result)?;
    let body: ReadBody = json_codec::from_value(receipt.result.clone())?;
    let request = &body.move_request;
    let node = &body.deletion_node;
    crate::file_deletion::publication::admit_node_shape(node)?;
    storage_contract::validate_id(&request.namespace.source_repository_id, "repo")?;
    storage_contract::validate_id(&request.namespace.target_repository_id, "repo")?;
    storage_contract::validate_id(&request.namespace.expected_source_head, "commit")?;
    storage_contract::validate_id(&request.namespace.expected_target_head, "commit")?;
    storage_contract::validate_id(&body.branch_id, "br")?;
    storage_contract::validate_id(&body.history_tree_id, "history")?;
    name_policy::PortablePath::parse(&request.namespace.target_name)?;
    let source_path = name_policy::LookupPath::parse(&request.namespace.source_path)?;
    for hash in [
        &body.source_records_hash,
        &body.recovery_digest,
        &body.decision_inputs_digest,
    ] {
        digest(hash)?;
    }
    let deletion = object_hash(
        "file-history-move-deletion",
        &json!([receipt.operation_id, body.source_file_id]),
    )?;
    if request.source_file_id != body.source_file_id
        || node.file_id != body.source_file_id
        || body.file_id == body.source_file_id
        || body.file_id != history_copy::identity(receipt.operation_id, &body.source_file_id)?
        || node.node_id != format!("fdel_{}", &deletion[..32])
        || node.repository_id != request.namespace.source_repository_id
        || node.path.key() != source_path.key()
        || node.operation_id != receipt.operation_id
        || node.author != *principal
        || node.sequence != receipt.sequence
        || node.created_at != body.timestamp
        || body.move_route.created_at != body.timestamp
        || body.source_sequence == 0
        || body.source_sequence >= receipt.sequence
        || body.branch_generation == 0
        || body.membership_epoch != 1
        || receipt.request_digest
            != Engine::native_request_digest(KIND, principal, &json!(request))?
        || receipt.commit_id.as_deref() != Some(body.commit_id.as_str())
        || receipt.history_tree_id.as_deref() != Some(body.history_tree_id.as_str())
        || receipt.branch_id.as_deref() != Some(body.branch_id.as_str())
        || body.commit_id != history_copy::identity(receipt.operation_id, "commit_arrival")?
        || body.retained_lifecycle.source_sequence != body.source_sequence
        || body.retained_lifecycle.source_history_tree_id != node.history_tree_id
        || body.retained_lifecycle.source_records_hash != body.source_records_hash
    {
        return Err(invalid());
    }
    history_copy::retained::validate(receipt, &body.file_id, &body.retained_lifecycle)?;
    body.require_versions(receipt.operation_id)?;
    body.require_arrival(state, receipt, principal)?;
    Ok(body.deletion_node)
}

/// Read only original operation intent. A later rename, D undo or HEAD change
/// must not turn an exact retry into a new capture of today's source path.
pub(crate) fn original_move_namespace(receipt: &OperationResult) -> Result<FileHistoryCopyRequest> {
    if receipt.kind != KIND {
        return Err(invalid());
    }
    history_copy::retained::bounded(&receipt.result)?;
    let body: ReadBody = json_codec::from_value(receipt.result.clone())?;
    Ok(body.move_request.namespace)
}

/// Recompute the complete selected transition from the old published State,
/// using the one native receipt's original request, route, clock and operation.
/// Neither a stored draft nor an arbitrary snapshot may substitute its writes.
pub(crate) fn replay_selected_move(
    engine: &Engine,
    next: &State,
    operation: OperationId,
) -> Result<(FileHistoryMoveProjection, NativeMoveDraft)> {
    let receipt = next.operations.get(&operation).ok_or_else(invalid)?;
    admit_source_deletion(next, receipt)?;
    let body: ReadBody = json_codec::from_value(receipt.result.clone())?;
    let principal = next
        .operation_principals
        .get(&operation)
        .ok_or_else(invalid)?;
    let stop = HistoryWindowCancellation::default();
    let capture = engine.capture_retained_file_history_move(
        FileHistoryMoveRequest {
            namespace: &body.move_request.namespace,
            source_file_id: &HistoryFileId::parse(&body.move_request.source_file_id)?,
        },
        &stop,
    )?;
    let projection = engine.replay_file_history_move_projection(
        capture,
        FileHistoryMoveReplay {
            principal: principal.clone(),
            operation,
            sequence: receipt.sequence,
            route: body.move_route,
        },
        &stop,
    )?;
    let native = projection.native_draft(
        engine,
        &FileHistoryMoveRecoveryDigest::from_native_receipt(&body.recovery_digest)?,
        &FileHistoryMoveDecisionDigest::from_native_receipt(&body.decision_inputs_digest)?,
    )?;
    let rebuilt = native.candidate_draft()?;
    if crate::disk::encode_metadata(rebuilt.state(), 12 * 1024 * 1024)?
        != crate::disk::encode_metadata(next, 12 * 1024 * 1024)?
    {
        return Err(invalid());
    }
    Ok((projection, native))
}
