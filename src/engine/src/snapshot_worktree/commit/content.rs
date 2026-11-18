//! Proof for multiple existing originals in one Initial worktree. Capture is
//! immutable: it verifies real native owners, FILEs, sparse bodies and source
//! node but neither allocates nor publishes a history node or container.
pub(in crate::snapshot_worktree) mod plan;
pub(crate) mod material;
pub(crate) mod recovery;
pub(crate) mod scope;
use crate::container_storage::ObservedContainerReplica;
use crate::directory_worktree::ledger::{Binding, Checkpoint, EditRecord};
use crate::directory_worktree::lifecycle::Phase;
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::{ReceiptIndex, RegistrationError, Result};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotFile, SnapshotFileSource};
use crate::{Engine, State, ValidatedConfig};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_contract::{CommitId, DiskId, Generation, HistoryTreeId, OperationId};
use storage_contract::file_container::{ContainerEntity, ContainerNamespace};
use storage_contract::file_draft::{DraftBase, DraftLocation, FileWorkingState};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, SnapshotHost};
use storage_contract::working_tree::RootId;

pub(crate) struct OriginalRow {
    pub(crate) edit_operation: OperationId,
    pub(crate) edit_completion: OperationId,
    pub(crate) edit: EditRecord,
    pub(crate) original: SnapshotFile,
    pub(crate) target: DraftLocation,
    pub(crate) base: DraftBase,
    pub(crate) owner: DiskId,
    pub(crate) namespace: ContainerNamespace,
    pub(crate) owner_root: RootId,
    pub(crate) owner_entity: ContainerEntity,
    pub(crate) history: HistoryTreeId,
    pub(crate) branch_base: CommitId,
    pub(crate) clean: FileWorkingState,
    pub(crate) container: Arc<ObservedContainerReplica>,
}

pub(crate) struct Originals {
    mount_name: name_policy::Name,
    pub(crate) state: Arc<State>,
    pub(crate) config: ValidatedConfig,
    pub(crate) resources: Arc<crate::disk::DiskResources>,
    pub(crate) request: CommitWorktree,
    pub(crate) source: SnapshotHandle,
    pub(crate) host: HostEvidence,
    pub(crate) root: SnapshotHost,
    pub(crate) ordinary_head: CommitId,
    pub(crate) ordinary_generation: Generation,
    pub(crate) source_node: HeldSnapshotNode,
    pub(crate) repository: crate::working_tree::snapshot::PinnedSnapshotRepository,
    pub(crate) sparse: Arc<HeldSparseWorktree>,
    pub(crate) ledger: Binding,
    pub(crate) before: Checkpoint,
    pub(crate) rows: Vec<OriginalRow>,
    // Node/files/worktrees, held host/repository, sparse directory and its
    // ledger parent proof. Each ordinary FILE separately bills its own chain.
    _descriptors: crate::engine::PayloadProofReservation,
}

/// Only the captured set creates this source. The path carried into ordinary
/// FILE preparation is descriptive: each reopen verifies the sparse inode and
/// the complete sealed contents through its original managed-disk parent.
pub(crate) struct OriginalBody {
    sparse: Arc<HeldSparseWorktree>,
    edit: EditRecord,
    path: PathBuf,
}

impl OriginalBody {
    pub(crate) fn path(&self) -> &Path { &self.path }
    pub(crate) fn is_exact(&self, sparse: &Arc<HeldSparseWorktree>, edit: &EditRecord) -> bool {
        Arc::ptr_eq(&self.sparse, sparse) && &self.edit == edit
    }

    pub(crate) fn open(&self, path: &Path) -> crate::Result<crate::ReadFile> {
        if path != self.path {
            return Err(crate::business::file_draft::invalid());
        }
        self.sparse.open_material(&self.edit.file_edit().material).map_err(|error| {
            crate::Error::SnapshotSource(Box::new(crate::snapshot_worktree::SnapshotSourceError::OriginalBody(
                Box::new(RegistrationError::EditInstance(error).into()),
            )))
        })
    }
}

impl Originals {
    pub(super) fn body(&self, i: usize) -> crate::Result<crate::engine::ReadSource> {
        let row = self.rows.get(i).ok_or_else(crate::business::file_draft::invalid)?;
        let path = self.sparse.directory().path().join(row.edit.leaf.entry().as_path());
        Ok(crate::engine::ReadSource::original_body(Arc::new(OriginalBody {
            sparse: Arc::clone(&self.sparse), edit: row.edit.clone(), path,
        })))
    }

    pub(in crate::snapshot_worktree) fn capture(
        engine: &Engine, index: &ReceiptIndex<'_>, request: &CommitWorktree,
    ) -> Result<Self> {
        Self::capture_inner(engine, index, request, None)
    }

    pub(in crate::snapshot_worktree) fn retained(
        engine: &Engine, index: &ReceiptIndex<'_>, grant: &crate::snapshot_worktree::registry::FrozenContent<'_>,
    ) -> Result<Self> {
        Self::capture_inner(engine, index, &grant.plan().request, Some(grant))
    }

    fn capture_inner(
        engine: &Engine, index: &ReceiptIndex<'_>, request: &CommitWorktree,
        owned: Option<&crate::snapshot_worktree::registry::FrozenContent<'_>>,
    ) -> Result<Self> {
        index.require_source(engine.state())?;
        if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
            || engine.config.generation().is_some()
        {
            return Err(RegistrationError::InitialOnly);
        }
        let registered = match owned {
            Some(grant) => index.registered_for_content(grant)?,
            None => index.worktree(request.target)?,
        };
        let source = registered.source().handle();
        let host = registered.source().host().clone();
        let root = registered.source().initialization().host;
        if (owned.is_none() && index.commit_head(source)? != source) || registered.revision() != request.expected
            || registered.published_node().is_some() || !registered.added_files().is_empty()
            || registered.reservation().request.parent_directory_id != root.directory_id
            || !host.directories.is_empty()
        {
            return Err(RegistrationError::CommitScope);
        }
        crate::snapshot_worktree::edit::require_mount(engine, registered.reservation())?;
        registered.checkpoint().require_publication_frames().map_err(RegistrationError::Ledger)?;
        let descriptors = crate::engine::payload_proof_budget().reserve(7)
            .ok_or_else(|| RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into()))?;
        let source_node = index.reopen_node(engine, source)?;
        let ordinary = engine.namespace_host_root(host.namespace.repository_id, host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        let local = ordinary.binding();
        if local.registration != host.registration || local.root_entity != host.root_entity
            || local.roots.common_data != host.common_data || local.root_id != root.root_id
            || local.generation != root.expected_root_generation || local.roots.common_data.directory_id != root.directory_id
        { return Err(RegistrationError::Source); }
        let repository = source_node.pin_repository(ordinary.common().map_err(RegistrationError::Publication)?)
            .map_err(RegistrationError::NodeReadback)?;
        let ledger = registered.reopen(&source_node)?.into_ledger();
        let active = index.active_edits(request.target);
        if active.is_empty() || active.len() > crate::working_tree::snapshot::MAX_VERSIONS
            || ledger.edits() != active || !ledger.file_results().is_empty()
            || !ledger.added_files().is_empty() || ledger.worktree().phase() != (Phase::Active { saved: None })
        {
            return Err(RegistrationError::CommitScope);
        }
        ledger.require_existing_content_scope().map_err(RegistrationError::Ledger)?;
        let sparse = Arc::new(HeldSparseWorktree::reopen(&source_node, ledger.instance())
            .map_err(RegistrationError::EditInstance)?);
        let before = ledger.checkpoint();
        let ledger_binding = ledger.binding();
        drop(ledger);
        let owner_id: DiskId = host.registration.disk_id.parse().map_err(RegistrationError::Allocation)?;
        let disk = engine.config().disks.iter().find(|disk| disk.disk_id == host.registration.disk_id)
            .ok_or(RegistrationError::Source)?;
        let branch = engine.state().branches.get(&host.namespace.branch_id.to_string())
            .ok_or(RegistrationError::Source)?;
        let ordinary_head = branch.head_commit_id.parse().map_err(RegistrationError::Allocation)?;
        let ordinary_generation = Generation::new(branch.generation);
        let namespace = engine.namespace_snapshot(&branch.branch_id).map_err(RegistrationError::Publication)?;
        let mut files = BTreeSet::new();
        let mut entries = BTreeSet::new();
        let mut entities = BTreeSet::new();
        let mut rows = Vec::with_capacity(active.len());
        for edit in active {
            let (plan, completed) = index.original_edit(request.target, edit)?;
            let original = plan.original;
            let SnapshotFileSource::Container { branch, node } = plan.original_source else {
                return Err(RegistrationError::CommitScope);
            };
            if !files.insert(original.file_id) { return Err(RegistrationError::CommitScope); }
            let record = engine.state().file_containers.get(&original.file_id)
                .ok_or(RegistrationError::CommitScope)?;
            let replica = record.replicas.get(&owner_id).ok_or(RegistrationError::CommitScope)?;
            let target = DraftLocation { repository: host.namespace.repository_id,
                path: edit.path.clone(), entry: record.namespace.entry, file: original.file_id };
            if record.owner_disk != owner_id || record.namespace.parent.ordinary().is_none()
                || record.namespace.repository != target.repository || record.namespace.branch != host.namespace.branch_id
                || replica.root_id != root.root_id || replica.entity.inode() == 0
                || !entries.insert(target.entry) || !entities.insert((replica.entity.device(), replica.entity.inode()))
                || !matches!(record.working, FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. })
            {
                return Err(RegistrationError::CommitScope);
            }
            let chosen = crate::business::file_draft::from_worktree_namespace(engine.state(), &target, namespace.tree())
                .map_err(RegistrationError::Publication)?;
            if chosen.base.branch_id() != branch || chosen.base.node_id() != node
                || chosen.base.version_id() != original.version_id || chosen.history != record.history_tree_id
                || chosen.placement.route_slot_ids.as_slice() != [disk.slot_id.clone()]
                || chosen.placement.binding_generations.get(&disk.slot_id) != Some(&disk.binding_generation)
                || disk.level != 1 || disk.upstream_slot_id.is_some()
            {
                return Err(RegistrationError::CommitScope);
            }
            let branch_base = engine.state().branches.get(&branch.to_string())
                .ok_or(RegistrationError::CommitScope)?.head_commit_id.parse().map_err(RegistrationError::Allocation)?;
            let version = engine.state().versions.get(&original.version_id.to_string()).ok_or(RegistrationError::CommitScope)?;
            if version.file_id != original.file_id.to_string() || version.logical_size != original.size
                || version.content_hash != original.content_hash
            {
                return Err(RegistrationError::CommitScope);
            }
            let container = Arc::new(crate::engine::fresh_layout::containers::observe_replica(
                engine.config(), &engine.disk_resources, engine.state(), original.file_id, disk,
            ).map_err(RegistrationError::Publication)?);
            container.verify_version_body(version).map_err(RegistrationError::Publication)?;
            sparse.open_material(&edit.file_edit().material).map_err(RegistrationError::EditInstance)?;
            rows.push(OriginalRow {
                edit_operation: completed.reserve, edit_completion: plan.completion, edit: edit.clone(),
                original, target, base: chosen.base, owner: owner_id, namespace: record.namespace.clone(),
                owner_root: replica.root_id, owner_entity: replica.entity, history: chosen.history,
                branch_base, clean: record.working.clone(), container,
            });
        }
        let mount_name=name_policy::Name::parse(registered.mount.mount.name.as_str())
            .map_err(|e| RegistrationError::WorktreeName(crate::directory_worktree::names::NameError::Invalid(e)))?;
        Ok(Self {
            mount_name,
            state: Arc::clone(&engine.state), config: engine.config.clone(), resources: Arc::clone(&engine.disk_resources),
            request: request.clone(), source, host, root, ordinary_head, ordinary_generation, source_node, repository,
            sparse, ledger: ledger_binding, before, rows, _descriptors: descriptors,
        })
    }
}

/// Only the exact live capture can initiate occupancy. The local result is
/// never a grant: a caller must recover the original from the installed prefix.
pub(in crate::snapshot_worktree) fn reserve(
    engine: &mut Engine, original: Originals, principal: &storage_contract::Principal,
) -> Result<()> {
    let plan = original.plan_reserve(&engine.planning_context(), principal)?;
    let operation = plan.request.operation_id;
    let record = crate::persist::snapshot_worktree::Record::ReserveContentBatch(plan);
    // Keep the full physical observations through native metadata preparation.
    crate::snapshot_worktree::publication::publish_new(engine, operation, principal, &record)?;
    drop(original);
    Ok(())
}

pub(super) fn finish(engine: &mut Engine, index: &ReceiptIndex<'_>,
    retry: crate::snapshot_worktree::registry::ContentRetry<'_>, request: &CommitWorktree,
    principal: &storage_contract::Principal) -> Result<SnapshotHandle> {
    use crate::snapshot_worktree::registry::ContentRetry;
    let handle = match retry {
        ContentRetry::Complete(handle) => handle,
        ContentRetry::Reserved(frozen) => {
            engine.require_unstarted_content_slot(request, principal, Some(frozen.plan().publish_operation)).map_err(RegistrationError::Publication)?;
            let input = material::OriginalContentInput::capture(engine, index, &frozen, principal)?;
            engine.publish_sparse_content(input).map_err(RegistrationError::Publication)?;
            #[cfg(feature = "fault-injection")]
            engine.failpoint("after_worktree_content_response").map_err(RegistrationError::Publication)?;
            let receipts = engine.snapshot_receipts()?;
            let Some(ContentRetry::Complete(handle)) = receipts.retry_content(request, principal)?
                else { return Err(RegistrationError::Pending(request.operation_id)); };
            handle
        }
    };
    let completed = engine.snapshot_receipts()?;
    completed.reopen_node(engine, handle)?;
    crate::snapshot_worktree::with_worktree(engine, request.target, |_| Ok(()))?;
    Ok(handle)
}
