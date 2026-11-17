use crate::disk::DiskResources;
use crate::engine::PlanningContext;
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::drafts::{
    DirectoryDraftError as Failure, DirectoryDraftIntent, Result,
};
use crate::working_tree::snapshot::{
    HeldSnapshotNode, MAX_ENTRIES, SnapshotEntry, SnapshotEntryContent, SnapshotNodeBinding,
};
use crate::{Engine, State, ValidatedConfig};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_contract::file_container::NamespaceDirectoryParent;
use storage_contract::file_draft::{DraftLocation, WorkingSelection};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, SnapshotHandle};
use storage_contract::{DiskId, OperationId, Principal};

/// Same-source directory input, not a FILE material or draft-clear capability.
/// No directory, ledger, version identity or permanent operation is created here.
pub(crate) struct DirectoryDraftScope {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    snapshot: Option<String>,
    slot: u64,
    source: SnapshotHandle,
    source_binding: SnapshotNodeBinding,
    host: HostEvidence,
    original: Vec<SnapshotEntry>,
    selected: Vec<WorkingSelection>,
    projection: Vec<DirectoryOccurrence>,
    operation: OperationId,
    principal: Principal,
    message: String,
}

/// Indexes are created only by the bounded, sorted merge below. This mapping
/// refers to the exact original manifest and picker request order, not paths
/// reopened from today's namespace or caller-provided historical version IDs.
#[derive(Clone, Copy)]
pub(super) enum DirectoryOccurrence {
    Original(usize),
    Selected(usize),
}

impl DirectoryDraftScope {
    /// The native planner must perform exact Existing handling before calling
    /// this. Selecting a node is not a durable reservation of its branch.
    pub(super) fn capture(engine: &Engine, intent: DirectoryDraftIntent<'_>) -> Result<Self> {
        // Permanent exact retry is checked before current write/CAS gates.
        // This low-level fresh planner still cannot return an Existing result.
        if super::retry(engine, &intent)?.is_some() {
            return Err(Failure::Existing(intent.operation));
        }
        engine.require_write().map_err(Failure::Engine)?;
        if engine.admitted_current_namespaces().map_err(Failure::Engine)?.is_some()
            || engine.config.generation().is_some()
        {
            return Err(Failure::Scope);
        }
        validate_selection(intent.selected, engine.config.max_entries.min(MAX_ENTRIES))?;
        let index = engine.snapshot_receipts().map_err(Failure::Registration)?;
        let node = index.draft_head(intent.source).map_err(Failure::Registration)?;
        let host = node.host();
        if node.initialization().source.root.is_some()
            || host.namespace.directory
                != NamespaceDirectoryId::Repository(host.namespace.repository_id)
        {
            return Err(Failure::Scope);
        }
        let root = engine.namespace_host_root(
            host.namespace.repository_id,
            host.namespace.branch_id,
        ).map_err(Failure::Engine)?;
        let owner = root.binding().registration.disk_id.parse::<DiskId>()
            .map_err(Failure::Identity)?;
        // Root evidence is from the published directory, not merely a same-disk
        // Config or a user path. Full immutable-node readback is delayed until
        // the FILE picker has checked the entire requested save-generation set.
        if host.registration != root.binding().registration
            || host.root_entity != root.binding().root_entity
            || host.common_data != root.binding().roots.common_data
        {
            return Err(Failure::SourceChanged);
        }
        for selection in intent.selected {
            require_owner(engine.state(), host, owner, &selection.target)?;
        }
        require_original(node.manifest().entries(), intent.selected)?;
        let projection = merge_selection(node.manifest().entries(), intent.selected)?;
        root.common().map_err(Failure::Engine)?;
        Ok(Self {
            state: Arc::clone(&engine.state),
            config: engine.config.clone(),
            resources: Arc::clone(&engine.disk_resources),
            snapshot: engine.snapshot.clone(),
            slot: engine.next_slot,
            source: intent.source,
            source_binding: node.binding(),
            host: host.clone(),
            original: node.manifest().entries().to_vec(),
            selected: intent.selected.to_vec(),
            projection,
            operation: intent.operation,
            principal: intent.principal.clone(),
            message: intent.message.into(),
        })
    }

    /// Used immediately before binding the unique batch picker/construction
    /// result. Equal State values or a fresh admission cannot reissue this scope.
    pub(crate) fn require_engine(&self, engine: &Engine) -> Result<()> {
        self.require_source(&engine.planning_context())?;
        engine.require_write().map_err(Failure::Engine)
    }

    pub(super) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.require_source(context)?;
        context.require_write().map_err(Failure::Engine)
    }

    fn require_source(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.state, context.state)
            || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || self.snapshot != *context.snapshot
            || self.slot != context.next_slot
            || context.current_namespaces.is_some()
        {
            return Err(Failure::SourceChanged);
        }
        Ok(())
    }

    /// This is the real permanently registered source, not a snapshot rebuilt
    /// from today's entire namespace. A late unselected draft is never scanned.
    pub(crate) fn open_original(&self, engine: &Engine) -> Result<HeldSnapshotNode> {
        self.require_engine(engine)?;
        let index = engine.snapshot_receipts().map_err(Failure::Registration)?;
        let node = index.reopen_node(engine, self.source).map_err(Failure::Registration)?;
        if node.binding() != self.source_binding {
            return Err(Failure::SourceChanged);
        }
        Ok(node)
    }

    pub(crate) fn selections(&self) -> &[WorkingSelection] {
        &self.selected
    }

    pub(crate) fn original_entries(&self) -> &[SnapshotEntry] {
        &self.original
    }

    pub(super) fn projection(&self) -> &[DirectoryOccurrence] {
        &self.projection
    }

    pub(crate) fn source(&self) -> SnapshotHandle {
        self.source
    }

    pub(crate) fn host(&self) -> &HostEvidence {
        &self.host
    }

    pub(crate) fn operation(&self) -> OperationId {
        self.operation
    }

    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(super) fn validate_selection(targets: &[WorkingSelection], maximum: usize) -> Result<()> {
    if targets.is_empty() || targets.len() > maximum {
        return Err(Failure::Selection);
    }
    let mut files = BTreeSet::new();
    let mut entries = BTreeSet::new();
    for selection in targets {
        let target = &selection.target;
        if target.path.parent().is_some()
            || !files.insert(target.file) || !entries.insert(target.entry)
        {
            return Err(Failure::Selection);
        }
    }
    let mut ordered: Vec<_> = targets.iter().map(|selection| &selection.target).collect();
    ordered.sort_by(|left, right| left.path.key().cmp(right.path.key()));
    if ordered.windows(2).any(|pair| pair[0].path.key() == pair[1].path.key()) {
        return Err(Failure::Selection);
    }
    Ok(())
}

pub(super) fn require_owner(
    state: &State,
    host: &HostEvidence,
    owner: DiskId,
    target: &DraftLocation,
) -> Result<()> {
    let record = state.file_containers.get(&target.file)
        .ok_or(Failure::Occurrence(target.file))?;
    if target.repository != host.namespace.repository_id
        || record.namespace.repository != host.namespace.repository_id
        || record.namespace.branch != host.namespace.branch_id
        || record.namespace.entry != target.entry
        || record.namespace.parent != NamespaceDirectoryParent::Root
        || record.namespace.name.as_str() != target.path.file_name()
        || record.owner_disk != owner
    {
        return Err(Failure::Occurrence(target.file));
    }
    // Born/Unborn, exact current occurrence, working mode/revision, selected
    // base and saved manifest belong to the unique working-set picker. Passing
    // this owner relation alone must never authorize a FILE construction.
    Ok(())
}

pub(super) fn require_original(
    entries: &[SnapshotEntry], targets: &[WorkingSelection],
) -> Result<()> {
    let files: BTreeMap<_, _> = targets.iter()
        .map(|selection| (selection.target.file, &selection.target)).collect();
    let mut paths: Vec<_> = targets.iter().map(|selection| &selection.target).collect();
    paths.sort_by(|left, right| left.path.key().cmp(right.path.key()));
    for entry in entries {
        let index = paths.binary_search_by(|target| target.path.key().cmp(entry.path.key()));
        if let Ok(index) = index {
            let target = paths[index];
            match entry.content {
                SnapshotEntryContent::File { material, .. }
                    if entry.path == target.path && material.file_id == target.file => {}
                SnapshotEntryContent::File { .. } | SnapshotEntryContent::Directory {} => {
                    return Err(Failure::Original(target.file));
                }
            }
        } else if let SnapshotEntryContent::File { material, .. } = entry.content
            && files.contains_key(&material.file_id)
        {
            // A move/alias is a separate occurrence transition. It cannot be
            // smuggled into this root-only selected-body publication.
            return Err(Failure::Original(material.file_id));
        }
    }
    Ok(())
}

pub(super) fn merge_selection(
    original: &[SnapshotEntry],
    selected: &[WorkingSelection],
) -> Result<Vec<DirectoryOccurrence>> {
    let mut order: Vec<_> = (0..selected.len()).collect();
    order.sort_by(|left, right| {
        selected[*left].target.path.key().cmp(selected[*right].target.path.key())
    });
    let mut source = 0;
    let mut change = 0;
    let mut result = Vec::new();
    while source < original.len() || change < order.len() {
        if result.len() == MAX_ENTRIES {
            return Err(Failure::Selection);
        }
        match (original.get(source), order.get(change)) {
            (Some(entry), Some(index)) => {
                match entry.path.key().cmp(selected[*index].target.path.key()) {
                    std::cmp::Ordering::Less => {
                        result.push(DirectoryOccurrence::Original(source));
                        source += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        result.push(DirectoryOccurrence::Selected(*index));
                        source += 1;
                        change += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        result.push(DirectoryOccurrence::Selected(*index));
                        change += 1;
                    }
                }
            }
            (Some(_), None) => {
                result.push(DirectoryOccurrence::Original(source));
                source += 1;
            }
            (None, Some(index)) => {
                result.push(DirectoryOccurrence::Selected(*index));
                change += 1;
            }
            (None, None) => break,
        }
    }
    Ok(result)
}
