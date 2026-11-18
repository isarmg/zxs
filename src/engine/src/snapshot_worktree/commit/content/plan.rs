//! All rows use the real FILE version/branch/root producer in the same order
//! on one prospective State. Reservation fixes the exact allocation trace;
//! replaying that trace is not permission to decide or install its output.
use super::{OriginalRow, Originals};
use crate::business::file_draft::worktree_content::{Trace, advance};
use crate::engine::{PlanningContext, Prepared};
use crate::persist::snapshot_worktree::content_batch::{Input, Output, Reservation};
use crate::snapshot_worktree::{RegistrationError, ReservedFileVersion, Result};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotEntryContent, SnapshotFile, SnapshotFileSource};
use crate::State;
use std::sync::Arc;
use storage_contract::{OperationId, Principal, Sequence};
use storage_contract::working_tree::SnapshotNodeId;

pub(in crate::snapshot_worktree) struct Projection {
    pub(in crate::snapshot_worktree) state: State,
    pub(in crate::snapshot_worktree) versions: Vec<Prepared>,
    pub(in crate::snapshot_worktree) rows: Vec<Output>,
    pub(in crate::snapshot_worktree) manifest: AdmittedSnapshotManifest,
}

impl OriginalRow {
    fn input(&self, result: &Output) -> Input {
        Input {
            edit_operation: self.edit_operation, edit_completion: self.edit_completion,
            edit: self.edit.clone(), original: self.original, base: self.base,
            owner: self.owner, namespace: self.namespace.clone(), owner_root: self.owner_root,
            owner_entity: self.owner_entity, history: self.history, branch_base: self.branch_base,
            clean: self.clean.clone(), commit: result.commit, tree: result.tree,
            allocations: result.content.allocations.clone(),
        }
    }
}

impl Originals {
    pub(super) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.state, context.state) || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || context.current_namespaces.is_some() || context.config.generation().is_some()
        {
            return Err(RegistrationError::PublicationChanged);
        }
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn plan_reserve(
        &self, context: &PlanningContext<'_>, principal: &Principal,
    ) -> Result<Reservation> {
        let destination_node = SnapshotNodeId::generate().map_err(RegistrationError::Allocation)?;
        let publish_operation = OperationId::generate().map_err(RegistrationError::Allocation)?;
        if context.state.operations.contains_key(&publish_operation)
            || publish_operation == self.request.operation_id || destination_node == self.source.node_id
        {
            return Err(RegistrationError::Identity);
        }
        let prepared = self.project(context, principal, destination_node, publish_operation, None)?;
        self.repository.require_held_source(&self.source_node).map_err(RegistrationError::NodeReadback)?;
        self.repository.require_free_content_node(destination_node).map_err(RegistrationError::NodeReadback)?;
        for (row, result) in self.rows.iter().zip(&prepared.rows) {
            let path = row.container.body_path(&result.content.version).map_err(RegistrationError::Publication)?;
            if row.container.container().map_err(RegistrationError::Publication)?
                .optional_metadata(&path).map_err(RegistrationError::Publication)?.is_some()
            { return Err(RegistrationError::Identity); }
        }
        let rows = self.rows.iter().zip(&prepared.rows).map(|(row, result)| row.input(result)).collect();
        let frozen = Reservation {
            request: self.request.clone(), source: self.source, source_binding: self.source_node.binding(),
            host: self.host.clone(), root: self.root, ordinary_head: self.ordinary_head,
            ordinary_generation: self.ordinary_generation, instance: self.sparse.binding(), ledger: self.ledger,
            before: self.before, destination_node, publish_operation, rows,
        };
        let ledger = crate::directory_worktree::ledger::HeldWorktreeLedger::reopen(&self.source_node,
            self.ledger, self.before, |_, _| Err(crate::directory_worktree::ledger::LedgerError::PublicationPending))
            .map_err(RegistrationError::Ledger)?;
        if ledger.instance() != self.sparse.binding() { return Err(RegistrationError::CommitScope); }
        ledger.preview_content(&frozen.events(&prepared.rows)).map_err(RegistrationError::Ledger)?;
        Ok(frozen)
    }

    pub(in crate::snapshot_worktree) fn project_reserved(
        &self, context: &PlanningContext<'_>, principal: &Principal, frozen: &Reservation,
    ) -> Result<Projection> {
        if self.request != frozen.request || self.source != frozen.source || self.host != frozen.host
            || self.root != frozen.root || self.source_node.binding() != frozen.source_binding
            || self.ordinary_head != frozen.ordinary_head || self.ordinary_generation != frozen.ordinary_generation
            || self.sparse.binding() != frozen.instance || self.ledger != frozen.ledger || self.before != frozen.before
            || self.rows.len() != frozen.rows.len()
        {
            return Err(RegistrationError::CommitScope);
        }
        self.project(context, principal, frozen.destination_node, frozen.publish_operation, Some(&frozen.rows))
    }

    fn project(
        &self, context: &PlanningContext<'_>, principal: &Principal,
        destination: SnapshotNodeId, operation: OperationId, frozen: Option<&[Input]>,
    ) -> Result<Projection> {
        self.require_context(context)?;
        let sequence = Sequence::new(context.next_slot);
        if sequence.get() <= context.state.sequence { return Err(RegistrationError::PublicationChanged); }
        let mut state = crate::business::file_draft::unborn::begin(context.state, sequence)
            .map_err(RegistrationError::Publication)?;
        let mut versions = Vec::with_capacity(self.rows.len());
        let mut results = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let body = self.body(i).map_err(RegistrationError::Publication)?;
            let mut issuer = ReservedFileVersion::for_original(&self.state, row);
            let trace = frozen.map_or(Trace::Allocate, |rows| Trace::Exact(&rows[i].allocations));
            let result = advance(context, &mut state, &mut versions, row, &body, &mut issuer,
                operation, principal, &self.request.message, trace).map_err(RegistrationError::Publication)?;
            if frozen.is_some_and(|rows| rows[i] != row.input(&result)) {
                return Err(RegistrationError::CommitScope);
            }
            results.push(result);
        }
        let manifest = replace_occurrences(self.source_node.manifest(), destination, operation, self.rows.iter().zip(&results)
            .map(|(row, output)| (&row.edit.path, row.original,
                SnapshotFileSource::Container { branch: row.base.branch_id(), node: row.base.node_id() }, &output.file)))?;
        // The authentic FILE results have supplied their actual node names.
        // This shared projection runs both before the permanent reserve and on
        // exact re-preparation. Never install a conflicted worktree namespace.
        crate::directory_worktree::io::require_configured_manifest(&state,&manifest,Some(&self.mount_name))
            .map_err(RegistrationError::ReadView)?;
        Ok(Projection { state, versions, rows: results, manifest })
    }
}

/// One full-manifest constructor for planning and verified receipt replay.
/// A row addresses its complete original path + FILE selector, never its
/// content hash or all appearances of that FileId. Untouched aliases keep
/// their own spelling/selection/version even when their bytes match a save.
pub(in crate::snapshot_worktree) fn replace_occurrences<'a>(
    source: &AdmittedSnapshotManifest, destination: SnapshotNodeId, operation: OperationId,
    replacements: impl IntoIterator<Item=(&'a name_policy::PortablePath, SnapshotFile, SnapshotFileSource,
        &'a crate::directory_worktree::projection::FileResult)>,
) -> Result<AdmittedSnapshotManifest> {
    let mut entries = source.entries().to_vec();
    let mut last = None;
    let mut selected = std::collections::BTreeSet::new();
    for (path, original, selector, result) in replacements {
        if last.is_some_and(|key| key >= path.key()) || !selected.insert(original.file_id)
            || selected.len() > crate::working_tree::snapshot::MAX_VERSIONS
            || result.path != *path || result.material.file_id != original.file_id
            || result.material.version_id == original.version_id
        { return Err(RegistrationError::CommitScope); }
        let at = entries.binary_search_by(|e| e.path.key().cmp(path.key())).map_err(|_| RegistrationError::CommitScope)?;
        let entry = &mut entries[at];
        if entry.path != *path || entry.content != (SnapshotEntryContent::File { material: original, source: selector }) {
            return Err(RegistrationError::CommitScope);
        }
        entry.content = SnapshotEntryContent::File { material: result.material, source: result.source };
        last = Some(path.key());
    }
    if selected.is_empty() { return Err(RegistrationError::CommitScope); }
    AdmittedSnapshotManifest::prepare(source.repository_id(), destination, operation, entries).map_err(RegistrationError::Manifest)
}
