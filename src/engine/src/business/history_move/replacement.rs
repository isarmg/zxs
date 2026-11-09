//! A replacement binds the displaced file before the one move decision.
//! Its bytes and graph stay owned by that file; they are never an import source.
use super::*;
use data_serialization::{Deserialize, Serialize};

/// Exact target observed by the authenticated caller. Parsing grants no write
/// authority: capture verifies this identity in the current target namespace.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct FileHistoryMoveReplacement {
    entry_id: String,
    file_id: String,
}
impl FileHistoryMoveReplacement {
    pub fn parse(entry_id: &str, file_id: &str) -> Result<Self> {
        storage_contract::validate_id(entry_id, "entry")?;
        HistoryFileId::parse(file_id)?;
        Ok(Self {
            entry_id: entry_id.into(),
            file_id: file_id.into(),
        })
    }

    pub(in crate::business) fn matches(&self, entry: &Entry) -> bool {
        entry.kind == NamespaceEntryKind::File
            && entry.entry_id == self.entry_id
            && entry.file_id == self.file_id
    }
}

pub(super) struct Capture {
    pub(super) identity: FileHistoryMoveReplacement,
    pub(super) entry: Entry,
    pub(super) context: FileContext,
    pub(super) heads: BTreeMap<String, String>,
    pub(super) parents: Vec<String>,
    pub(super) versions: Vec<String>,
    pub(super) records_hash: String,
}

fn changed_target() -> Error {
    Error::new(
        ErrorCode::WorkspaceChanged,
        "move replacement target differs from its exact original occurrence",
    )
}

pub(super) fn capture(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
    identity: &FileHistoryMoveReplacement,
    maximum: usize,
    cancellation: &HistoryWindowCancellation,
) -> Result<Capture> {
    checkpoint(cancellation)?;
    let repository = state
        .repositories
        .get(&request.target_repository_id)
        .ok_or_else(changed_target)?;
    let main = &state.branches[&repository.main_branch_id];
    if main.head_commit_id != request.expected_target_head {
        return Err(changed_target());
    }
    let retained = &state.commits[&main.head_commit_id].entries;
    let entries = match view {
        Some(view) => {
            view.require_branch_write(state, &main.branch_id)?;
            view.branch_entries(&main.branch_id, &main.head_commit_id)?
        }
        None => retained,
    };
    if entries != retained.as_slice() {
        return Err(changed_target());
    }
    let path = name_policy::LookupPath::parse(&request.target_name)?;
    require_ordinary_move_source(state, repository, entries, &path)?;
    let entry = namespace::find_unordered_key(entries, path.key())
        .filter(|entry| identity.matches(entry))
        .ok_or_else(changed_target)?;
    let file = HistoryFileId::parse(&entry.file_id)?;
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id)?;
    let deleted = crate::file_deletion::preflight::capture(
        state,
        repository,
        entries,
        FileDeletionTarget {
            repository_id: &repository_id,
            file_id: &file,
            path: &path,
        },
        maximum.min(storage_contract::client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES),
        cancellation,
    )?;
    if deleted.occurrences().len() != 1
        || state.file_deletions.contains_key(file.as_str())
        || state.file_backup_policies.contains_key(file.as_str())
        || state.file_backup_paths.contains_key(file.as_str())
        || main.pending_file_versions.contains_key(file.as_str())
    {
        return Err(Error::new(
            ErrorCode::FileBranchScope,
            "move replacement requires one ordinary saved target occurrence",
        ));
    }
    let context = state
        .file_contexts
        .get(&directory::context_key_from_key(
            &repository.repository_id,
            entry.lookup_key(),
        ))
        .ok_or_else(changed_target)?;
    if context.inherited_from.is_some()
        || state.file_selections.get(file.as_str()) != Some(&context.branch_id)
        || state.file_branch_aliases.contains_key(&context.branch_id)
        || !state.branches[&context.branch_id]
            .pending_file_versions
            .is_empty()
        || file_nodes::branch_head(state, &context.branch_id)?
            != *context.node_id.as_ref().unwrap_or(&entry.version_id)
    {
        return Err(changed_target());
    }
    let placement = &state.placements[&entry.version_id];
    let mut versions = Vec::new();
    for version in state
        .versions
        .values()
        .filter(|version| version.file_id == entry.file_id)
    {
        checkpoint(cancellation)?;
        if versions.len() == maximum.min(65_536) {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "move replacement target history exceeds its version budget",
            ));
        }
        let retained = &state.placements[&version.version_id];
        if version.history_tree_id != repository.history_tree_id
            || retained.primary_slot_id != repository.primary_slot_id
            || retained.primary_slot_id != placement.primary_slot_id
            || retained.target_copies != placement.target_copies
            || retained.route_slot_ids != placement.route_slot_ids
            || retained.binding_generations != placement.binding_generations
            || state
                .content_states
                .get(&version.version_id)
                .is_some_and(|state| *state != ContentState::Available)
        {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "move replacement requires the target's complete original FULL route",
            ));
        }
        if let Some(view) = view {
            view.require_backup_configuration_source(state, version)?;
        }
        versions.push(version.version_id.clone());
    }
    let database = crate::container_storage::file_database::capture_copy(
        state,
        file.as_str(),
        &repository.history_tree_id,
    )?;
    Ok(Capture {
        identity: identity.clone(),
        entry: entry.clone(),
        context: context.clone(),
        heads: deleted
            .branches()
            .iter()
            .map(|head| (head.branch().branch_id.clone(), head.node_id().into()))
            .collect(),
        parents: deleted.parent_node_ids().to_vec(),
        versions,
        records_hash: database.records_hash,
    })
}

pub(in crate::business) fn destination<'a>(
    request: &'a FileHistoryCopyRequest,
    replacement: Option<&'a FileHistoryMoveReplacement>,
) -> history_copy::Destination<'a> {
    match replacement {
        None => history_copy::Destination::MoveIntoDiskRoot(request),
        Some(identity) => history_copy::Destination::ReplaceIntoDiskRoot(request, identity),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Evidence {
    pub(super) deletion_node: FileDeletionNode,
    pub(super) occurrence: crate::file_deletion::occurrence::DisplacedOccurrence,
    pub(super) source_records_hash: String,
}

/// New live nodes must fit the existing exact recovery entrance. Leave room
/// for the ordinary undo receipt envelope rather than truncating a frontier.
pub(super) fn require_recoverable(node: &FileDeletionNode) -> Result<()> {
    use storage_contract::client_protocol::{
        MAX_FILE_DELETION_FRONTIER_ENTRIES, MAX_FILE_DELETION_RESPONSE_BYTES,
    };
    if node.branch_heads.len() > MAX_FILE_DELETION_FRONTIER_ENTRIES
        || node.parent_node_ids.len() > MAX_FILE_DELETION_FRONTIER_ENTRIES
    {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "complete move deletion frontier exceeds the recovery response budget",
        ));
    }
    crate::disk::encode_metadata(node, MAX_FILE_DELETION_RESPONSE_BYTES / 2)?;
    Ok(())
}

pub(super) fn project(
    state: &mut State,
    request: &FileHistoryCopyRequest,
    capture: &Capture,
    principal: &str,
    operation: OperationId,
    timestamp: u64,
) -> Result<Evidence> {
    let repository = &state.repositories[&request.target_repository_id];
    let hash = object_hash(
        "file-history-move-replaced-deletion",
        &json_codec::json!([operation, capture.entry.file_id]),
    )?;
    let node = FileDeletionNode {
        node_id: format!("fdel_{}", &hash[..32]),
        repository_id: repository.repository_id.clone(),
        history_tree_id: repository.history_tree_id.clone(),
        file_id: capture.entry.file_id.clone(),
        path: capture.entry.path.clone(),
        branch_heads: capture.heads.clone(),
        parent_node_ids: capture.parents.clone(),
        operation_id: operation,
        author: principal.into(),
        sequence: state.sequence,
        created_at: timestamp,
    };
    require_recoverable(&node)?;
    state
        .file_deletions
        .insert(node.file_id.clone(), node.clone());
    Ok(Evidence {
        deletion_node: node,
        occurrence: crate::file_deletion::occurrence::DisplacedOccurrence {
            namespace_head: request.expected_target_head.clone(),
            displaced: capture.entry.clone(),
            displaced_context: capture.context.clone(),
        },
        source_records_hash: capture.records_hash.clone(),
    })
}
