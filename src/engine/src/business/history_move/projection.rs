//! One private metadata write set, never a journal candidate or an I/O permit.
//! The target graph and source D share an operation, slot, actor and clock.
use super::*;
use history_copy::{CopyRoute, retained::CopiedLifecycle};
use json_codec::json;

mod native;
pub(crate) use native::{
    KIND as NATIVE_KIND, NativeMoveCandidate, NativeMoveDraft, admit_deletions,
    admit_displaced, original_move_intent, reject_unclosed_publication, replay_selected_move,
    retained_deletion, verify_current_move_transition,
};

struct ProjectionSource<'a> {
    namespace: &'a FileHistoryCopyRequest,
    file: &'a HistoryFileId,
    branch_heads: &'a BTreeMap<String, String>,
    parents: &'a [String],
    versions: &'a [String],
    replacement: Option<&'a replacement::Capture>,
}
impl<'a> From<&'a FileHistoryMoveCapture> for ProjectionSource<'a> {
    fn from(capture: &'a FileHistoryMoveCapture) -> Self {
        Self {
            namespace: &capture.namespace,
            file: &capture.source_file_id,
            branch_heads: &capture.branch_heads,
            parents: &capture.parent_node_ids,
            versions: &capture.version_ids,
            replacement: capture.replacement.as_ref(),
        }
    }
}

struct ProjectionInput<'a> {
    source: ProjectionSource<'a>,
    route: &'a CopyRoute,
    principal: &'a str,
    operation: OperationId,
    sequence: u64,
}

struct WriteSet {
    next: State,
    target_file: HistoryFileId,
    version_mapping: BTreeMap<String, String>,
    branch_mapping: BTreeMap<String, String>,
    lifecycle: CopiedLifecycle,
    replacement: Option<replacement::Evidence>,
}

/// Frozen inputs recovered only after checking the original source envelope.
/// Construction alone grants no authority; replay checks current admission.
pub(crate) struct FileHistoryMoveReplay {
    pub(crate) principal: String,
    pub(crate) operation: OperationId,
    pub(crate) sequence: u64,
    pub(crate) route: CopyRoute,
}

#[derive(data_serialization::Serialize)]
pub(crate) struct FileHistoryMoveTargetMetadata<'a> {
    pub(crate) target: crate::HistoryManifest,
    pub(crate) imported_lifecycle: &'a CopiedLifecycle,
    pub(crate) source_deletion: &'a FileDeletionNode,
    #[serialization(skip_serializing_if = "Option::is_none")]
    replacement: Option<&'a replacement::Evidence>,
}

/// Proposed target graph and source deletion, held together and never exposed
/// as a writable State. This is deliberately not serializable, not a permanent
/// operation receipt and not accepted by any publication or SMB entry point.
/// The proposed slot is observed, not reserved. Drop discards only memory.
pub struct FileHistoryMoveProjection {
    capture: FileHistoryMoveCapture,
    principal: String,
    operation: OperationId,
    route: CopyRoute,
    writes: WriteSet,
}

impl FileHistoryMoveProjection {
    pub fn capture(&self) -> &FileHistoryMoveCapture {
        &self.capture
    }

    pub fn operation_id(&self) -> OperationId {
        self.operation
    }

    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// An unreserved proposed decision slot, not a published sequence.
    pub fn proposed_sequence(&self) -> u64 {
        self.writes.next.sequence
    }

    pub fn proposed_source_deletion(&self) -> &FileDeletionNode {
        &self.writes.next.file_deletions[self.capture.source_file_id.as_str()]
    }

    pub fn target_file_id(&self) -> &HistoryFileId {
        &self.writes.target_file
    }

    /// Complete source-to-target version identities, not verified body copies.
    pub fn version_mapping(&self) -> &BTreeMap<String, String> {
        &self.writes.version_mapping
    }

    /// Includes archived and alias branch identities, not only the write branch.
    pub fn branch_mapping(&self) -> &BTreeMap<String, String> {
        &self.writes.branch_mapping
    }

    /// Frozen logical route bindings. They do not prove physical disk health.
    pub fn target_binding_generations(&self) -> &BTreeMap<String, u64> {
        &self.route.bindings
    }

    /// Count of historical D/undo facts proposed for the target. The new source
    /// D belongs to the source and is not imported into the target as a live D.
    pub fn retained_lifecycle_events(&self) -> usize {
        self.writes.lifecycle.events.len()
    }

    pub(crate) fn recovery_route(&self) -> &CopyRoute {
        &self.route
    }

    /// Exact bounded logical target graph, imported audit and the same source
    /// D. This digest does not certify physical bodies or any SQLite database.
    pub(crate) fn recovery_records_hash(&self) -> Result<String> {
        let closure = self.target_metadata()?;
        Ok(object_hash("file-history-move-joint-projection", &closure)?)
    }

    pub(crate) fn target_metadata(&self) -> Result<FileHistoryMoveTargetMetadata<'_>> {
        let history = &self.writes.next.repositories[&self.capture.namespace.target_repository_id]
            .history_tree_id;
        let closure = FileHistoryMoveTargetMetadata {
            target: crate::history_package::capture_history_copy(&self.writes.next, history)?,
            imported_lifecycle: &self.writes.lifecycle,
            source_deletion: self.proposed_source_deletion(),
            replacement: self.writes.replacement.as_ref(),
        };
        // Count before canonical Value/byte allocation. The entire joint
        // logical envelope, not each fragment separately, is limited to 12 MiB.
        history_copy::retained::bounded(&closure)?;
        Ok(closure)
    }

    pub(crate) fn target_file_database(
        &self,
    ) -> Result<crate::container_storage::file_database::PreparedMoveFile> {
        crate::container_storage::file_database::prepare_move(
            &self.writes.next,
            &self.writes.lifecycle,
        )
    }

    pub(crate) fn source_file_database(
        &self,
        previous: &State,
    ) -> Result<crate::container_storage::file_database::PreparedMoveFile> {
        crate::container_storage::file_database::prepare_move_source(
            previous,
            &self.writes.next,
            self.proposed_source_deletion(),
        )
    }

    pub(crate) fn proposed_replacement_deletion(&self) -> Option<&FileDeletionNode> {
        self.writes
            .replacement
            .as_ref()
            .map(|evidence| &evidence.deletion_node)
    }

    pub(crate) fn replacement_file_database(
        &self,
        previous: &State,
    ) -> Result<Option<crate::container_storage::file_database::PreparedMoveFile>> {
        self.proposed_replacement_deletion()
            .map(|node| {
                crate::container_storage::file_database::prepare_move_source(
                    previous,
                    &self.writes.next,
                    node,
                )
            })
            .transpose()
    }

    /// Exact complete proposal, intentionally still without a published move
    /// receipt/event. Its bytes are only copied to existing metadata owners.
    pub(crate) fn unaccepted_state_bytes(&self) -> Result<Vec<u8>> {
        history_copy::retained::bounded(&self.writes.next)?;
        crate::disk::encode_metadata(&self.writes.next, 12 * 1024 * 1024)
    }
}

/// A same-owner, same-slot metadata observation. It grants no body, database,
/// journal, recovery or publication authority.
pub struct CurrentFileHistoryMoveProjection<'a> {
    projection: &'a FileHistoryMoveProjection,
    _engine: &'a Engine,
}

impl CurrentFileHistoryMoveProjection<'_> {
    pub fn projection(&self) -> &FileHistoryMoveProjection {
        self.projection
    }
}

fn require_unused(engine: &Engine, operation: OperationId) -> Result<()> {
    engine.require_selected_move_operation(operation)?;
    if engine.state().operations.contains_key(&operation) {
        return Err(Error::new(
            ErrorCode::IdempotencyKeyReused,
            "move projection cannot reuse a published operation",
        ));
    }
    Ok(())
}

fn map_ids<'a>(
    operation: OperationId,
    ids: impl Iterator<Item = &'a String>,
) -> Result<BTreeMap<String, String>> {
    ids.map(|id| Ok((id.clone(), history_copy::identity(operation, id)?)))
        .collect()
}

fn deletion(previous: &State, input: &ProjectionInput<'_>) -> Result<FileDeletionNode> {
    let source_input = &input.source;
    let request = source_input.namespace;
    let source = &previous.repositories[&request.source_repository_id];
    let entry = namespace::find_unordered(
        &previous.commits[&request.expected_source_head].entries,
        &request.source_path,
    )
    .expect("invariant: a current move capture has the exact source occurrence");
    let hash = object_hash(
        "file-history-move-deletion",
        &json!([input.operation, source_input.file.as_str()]),
    )?;
    Ok(FileDeletionNode {
        node_id: format!("fdel_{}", &hash[..32]),
        repository_id: source.repository_id.clone(),
        history_tree_id: source.history_tree_id.clone(),
        file_id: source_input.file.as_str().into(),
        path: entry.path.clone(),
        branch_heads: source_input.branch_heads.clone(),
        parent_node_ids: source_input.parents.to_vec(),
        operation_id: input.operation,
        author: input.principal.into(),
        sequence: input.sequence,
        created_at: input.route.created_at,
    })
}

fn name_arrival(next: &mut State, request: &FileHistoryCopyRequest, file: &str) {
    let main = &next.repositories[&request.target_repository_id].main_branch_id;
    let selected = &next.file_selections[file];
    let commits = [
        next.branches[main].head_commit_id.clone(),
        next.branches[selected].head_commit_id.clone(),
    ];
    for id in commits {
        let commit = next
            .commits
            .get_mut(&id)
            .expect("invariant: target projection just created both arrival commits");
        commit.kind = "file_history_move".into();
        commit.message = "Move complete file history; soft delete source".into();
    }
}

/// Pure, deterministic reconstruction from one still-current capture and frozen
/// route/slot/actor/time. There is no operation receipt: exposing this partial
/// State to ordinary publication would be invalid and is intentionally absent.
fn compose(
    previous: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    input: &ProjectionInput<'_>,
) -> Result<WriteSet> {
    let mut next = previous.clone();
    next.begin_tracking();
    crate::container_storage::capture_names(&mut next);
    next.sequence = input.sequence;
    next.business_generation = previous
        .business_generation
        .checked_add(1)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "business generation exhausted"))?;
    let request = input.source.namespace;
    let copied = history_copy::retained::project(
        previous,
        &mut next,
        view,
        replacement::destination(
            request,
            input.source.replacement.map(|capture| &capture.identity),
        ),
        input.route,
        input.principal,
        input.operation,
    )?;
    let target_file = HistoryFileId::parse(&copied.lifecycle.file_id)?;
    name_arrival(&mut next, request, target_file.as_str());
    let node = deletion(previous, input)?;
    if input.source.replacement.is_some() {
        replacement::require_recoverable(&node)?;
    }
    next.file_deletions.insert(node.file_id.clone(), node);
    let replacement = input
        .source
        .replacement
        .map(|capture| {
            replacement::project(
                &mut next,
                request,
                capture,
                input.principal,
                input.operation,
                input.route.created_at,
            )
        })
        .transpose()?;
    crate::file_deletion::freeze::verify(previous, &next)?;
    crate::container_storage::capture_changed_names(previous, &mut next);
    // These are complete logical graph checks, not SQLite or payload proofs.
    for repository in [&request.source_repository_id, &request.target_repository_id] {
        crate::history_package::validate_history_copy(
            &next,
            &previous.repositories[repository].history_tree_id,
        )?;
    }
    Ok(WriteSet {
        next,
        target_file,
        version_mapping: map_ids(input.operation, input.source.versions.iter())?,
        branch_mapping: map_ids(input.operation, input.source.branch_heads.keys())?,
        lifecycle: copied.lifecycle,
        replacement,
    })
}

impl Engine {
    /// Compose the target retained graph and source full-frontier D in one
    /// private metadata write set. Does not copy bodies, install databases,
    /// reserve an operation or slot, write PREPARE/ACCEPT, publish a receipt,
    /// hide the source, or change the real target namespace.
    ///
    /// This consumes a current capture. The trusted caller authenticates the
    /// actor before invoking this API. Only the dedicated move_current_file
    /// executor can consume the complete three-scope and journal closure.
    pub fn project_file_history_move(
        &self,
        capture: FileHistoryMoveCapture,
        principal: &str,
        operation: OperationId,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveProjection> {
        checkpoint(cancellation)?;
        Self::require_actor(principal)?;
        require_unused(self, operation)?;
        self.recheck_file_history_move(&capture, cancellation)?;
        let route = CopyRoute {
            created_at: now()?,
            slots: capture.target_slots.clone(),
            bindings: capture
                .target_slots
                .iter()
                .map(|slot| Ok((slot.clone(), self.disk(slot)?.binding_generation)))
                .collect::<Result<_>>()?,
        };
        self.compose_file_history_move_projection(
            capture,
            FileHistoryMoveReplay {
                principal: principal.into(),
                operation,
                sequence: self.move_source_slot(),
                route,
            },
            cancellation,
        )
    }

    pub(crate) fn replay_file_history_move_projection(
        &self,
        capture: FileHistoryMoveCapture,
        replay: FileHistoryMoveReplay,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveProjection> {
        checkpoint(cancellation)?;
        Self::require_actor(&replay.principal)?;
        require_unused(self, replay.operation)?;
        self.recheck_file_history_move(&capture, cancellation)?;
        let bindings = capture
            .target_slots
            .iter()
            .map(|slot| Ok((slot.clone(), self.disk(slot)?.binding_generation)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if replay.sequence != self.move_source_slot()
            || replay.route.slots != capture.target_slots
            || replay.route.bindings != bindings
        {
            return Err(changed());
        }
        self.compose_file_history_move_projection(capture, replay, cancellation)
    }

    // Both callers above establish the same-owner source and exact route/slot.
    fn compose_file_history_move_projection(
        &self,
        capture: FileHistoryMoveCapture,
        replay: FileHistoryMoveReplay,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveProjection> {
        let writes = compose(
            self.state(),
            self.admitted_current_namespaces()?,
            &ProjectionInput {
                source: ProjectionSource::from(&capture),
                route: &replay.route,
                principal: &replay.principal,
                operation: replay.operation,
                sequence: replay.sequence,
            },
        )?;
        checkpoint(cancellation)?;
        Ok(FileHistoryMoveProjection {
            capture,
            principal: replay.principal,
            operation: replay.operation,
            route: replay.route,
            writes,
        })
    }

    /// Keep the original projection fixed. A changed source, target, config,
    /// decision slot or volatile write workspace is an error, not a replan.
    pub fn recheck_file_history_move_projection<'a>(
        &'a self,
        projection: &'a FileHistoryMoveProjection,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<CurrentFileHistoryMoveProjection<'a>> {
        self.recheck_file_history_move(&projection.capture, cancellation)?;
        require_unused(self, projection.operation)?;
        if self.move_source_slot() != projection.proposed_sequence() {
            return Err(changed());
        }
        Ok(CurrentFileHistoryMoveProjection {
            projection,
            _engine: self,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/history_move_projection_tests.rs"]
mod tests;
