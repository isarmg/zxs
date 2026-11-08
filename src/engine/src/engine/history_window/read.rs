//! Capture and recheck one current-root read around lock-free page preparation.
use super::*;

pub struct HistoryWindowRead {
    preparation: std::sync::Mutex<PagePreparation>,
    history_root: HistoryRoot,
    request: HistoryWindowRequest,
    context: HistoryResolvedContext,
    cancellation: HistoryWindowCancellation,
    max_entries: usize,
    // Produced with this exact frozen view by one immutable Engine borrow.
    // Both healthy-index and authoritative-scan capture paths verify the member.
    owner_version: String,
    scope: Option<crate::engine::index::OwnedScopeRead>,
}

// Release frozen business tables before waiting to reacquire the Engine lock.
// Direct API callers may retain this handle longer; it then owns only the page.
// Only successful projection changes state.
enum PagePreparation {
    Pending {
        view: MetadataView,
        indexed: Option<query::IndexedSelection>,
    },
    Prepared(Box<HistoryWindowPage>),
}

impl std::fmt::Debug for HistoryWindowRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HistoryWindowRead")
            .field("file_id", &self.request.file_id)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl HistoryWindowRead {
    /// Prepare one bounded page outside the Engine lock. The caller retains its
    /// backend admission slot through encoding and sending and rechecks current
    /// identity and `Engine::recheck_history_window` before releasing the page.
    pub fn page(&self) -> Result<HistoryWindowPage> {
        self.cancellation.checkpoint()?;
        let mut preparation = self
            .preparation
            .lock()
            .expect("invariant: history preparation lock poisoned");
        self.cancellation.checkpoint()?;
        let (view, indexed) = match &mut *preparation {
            PagePreparation::Pending { view, indexed } => (view, indexed),
            PagePreparation::Prepared(page) => return Ok((**page).clone()),
        };
        if indexed.is_none()
            && let Some(scope) = &self.scope
        {
            *indexed = query::prepare_indexed_selection(
                scope,
                view,
                &self.request,
                &self.context,
                &self.cancellation,
            )?;
        }
        let page = view.history_window_observed(
            &self.request,
            &self.context,
            &self.cancellation,
            indexed.as_ref(),
            Some(&self.owner_version),
        )?;
        self.cancellation.checkpoint()?;
        *preparation = PagePreparation::Prepared(Box::new(page.clone()));
        Ok(page)
    }

    pub fn cancellation(&self) -> HistoryWindowCancellation {
        self.cancellation.clone()
    }

    pub fn context(&self) -> &HistoryResolvedContext {
        &self.context
    }

    /// Current core action admission limit, captured without imposing a smaller
    /// history or UI selection capacity.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}

impl Engine {
    /// Capture current object ownership, exact logical path and manifest root.
    /// The server authenticates its current caller before this trusted local API.
    /// Create the cancellation signal before the job enters the backend queue.
    pub fn capture_history_window(
        &self,
        request: HistoryWindowRequest,
        cancellation: HistoryWindowCancellation,
    ) -> Result<HistoryWindowRead> {
        let (context, owner_version) = self.history_window_context(&request, &cancellation)?;
        let view = self.history_window_view();
        let history_root = query::Query::new_with_owner(
            &view,
            &request,
            &context,
            &cancellation,
            Some(&owner_version),
        )?
        .identity
        .history_root;
        let scope = if matches!(
            request.collection,
            HistoryCollection::Nodes
                | HistoryCollection::Versions
                | HistoryCollection::Branches
                | HistoryCollection::Merges
                | HistoryCollection::NodeParents { .. }
        ) || matches!(&request.collection,
            HistoryCollection::NodeField { node_id, field }
                if *field != HistoryField::CreatedAt && self.state.versions.contains_key(node_id.as_str()))
        {
            self.capture_history_scope(request.file_id.as_str(), context.history_tree_id.as_str())?
        } else {
            None
        };
        Ok(HistoryWindowRead {
            preparation: std::sync::Mutex::new(PagePreparation::Pending {
                view,
                indexed: None,
            }),
            history_root,
            request,
            context,
            cancellation,
            max_entries: self.config.max_entries,
            owner_version,
            scope,
        })
    }

    /// Recheck the current object/path scope and exact target root. Unrelated
    /// histories may publish while this read prepares. A retained view conveys
    /// neither current authentication nor access to an old-root service.
    pub fn recheck_history_window(&self, read: &HistoryWindowRead) -> Result<()> {
        if let Some(scope) = &read.scope {
            self.record_scope_failure(scope);
        }
        let (context, _) = self.history_window_context(&read.request, &read.cancellation)?;
        if context != read.context || self.config.max_entries != read.max_entries {
            return Err(query::changed());
        }
        let view = self.history_window_view();
        if HistoryRoot::parse(view.history_root(context.history_tree_id.as_str())?)?
            != read.history_root
        {
            return Err(query::changed());
        }
        Ok(())
    }

    fn history_window_context(
        &self,
        request: &HistoryWindowRequest,
        cancel: &HistoryWindowCancellation,
    ) -> Result<(HistoryResolvedContext, String)> {
        cancel.checkpoint()?;
        let file = request.file_id.as_str();
        let version = match self.indexed_history_owner(file, cancel)? {
            Some(version) => version,
            None => nodes::owner_version(self.state.versions.values(), file, cancel)?.ok_or_else(
                || Error::new(storage_contract::ErrorCode::FileNotFound, "file not found"),
            )?,
        };
        let repository = self
            .planning_context()
            .repository_for_history(&version.history_tree_id)?;
        let tree = self.namespace_tree(&repository.main_branch_id)?;
        let current = if let Some(location) = &request.location {
            if location.repository_id.as_str() != repository.repository_id {
                return Err(Error::new(
                    storage_contract::ErrorCode::FileNotFound,
                    "file is not present in this repository context",
                ));
            }
            Some(
                tree.get_key(location.path.key())
                    .filter(|entry| {
                        entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file
                    })
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::FileNotFound,
                            "file is not present at this logical path",
                        )
                    })?,
            )
        } else {
            let physical = self
                .state
                .file_locations
                .get(file)
                .and_then(|path| tree.get(path))
                .filter(|entry| {
                    entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file
                });
            match physical {
                Some(entry) => Some(entry),
                None => {
                    let mut chosen = None;
                    for entry in tree.entries() {
                        cancel.checkpoint()?;
                        if entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file {
                            chosen = Some(entry);
                            break;
                        }
                    }
                    chosen
                }
            }
        };
        let context = current.and_then(|entry| {
            self.file_context_for_current_entry(&repository.repository_id, entry)
        });
        let current_node = context
            .as_ref()
            .and_then(|context| context.node_id.as_deref())
            .or_else(|| current.map(|entry| entry.version_id.as_str()));
        let selected = context
            .as_ref()
            .map(|context| context.branch_id.as_str())
            .or_else(|| self.state.file_selections.get(file).map(String::as_str));
        Ok((
            HistoryResolvedContext {
                repository_id: HistoryRepositoryId::parse(&repository.repository_id)?,
                history_tree_id: HistoryTreeId::parse(&repository.history_tree_id)?,
                path: current.map(|entry| entry.path.clone()),
                current_node_id: current_node.map(HistoryNodeId::parse).transpose()?,
                selected_branch_id: selected.map(HistoryBranchId::parse).transpose()?,
            },
            version.version_id.clone(),
        ))
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/history_window_read_tests.rs"]
mod tests;

/// An on-demand ancestry proof. Does not project or alter ordinary history pages.
pub struct FileHistorySourceRead {
    read: HistoryWindowRead,
    request: storage_contract::file_history_merge::FileHistorySourceRequest,
}
impl FileHistorySourceRead {
    pub fn source(&self) -> Result<storage_contract::file_history_merge::FileHistorySource> {
        self.read.cancellation.checkpoint()?;
        let preparation = self
            .read
            .preparation
            .lock()
            .expect("invariant: history preparation lock poisoned");
        let PagePreparation::Pending { view, .. } = &*preparation else {
            unreachable!("invariant: source-only read never prepares an ordinary history page");
        };
        let query = query::Query::new_with_owner(
            view,
            &self.read.request,
            &self.read.context,
            &self.read.cancellation,
            Some(&self.read.owner_version),
        )?;
        let branch = super::merge_source::resolve(&query, self.request.node_id.as_str())?;
        let result = storage_contract::file_history_merge::FileHistorySource {
            file_id: self.request.file_id.clone(),
            source_node_id: self.request.node_id.clone(),
            history_root: self.read.history_root,
            branch,
        };
        result.validate(&self.request)?;
        Ok(result)
    }
}
impl Engine {
    pub fn capture_file_history_source(
        &self,
        request: storage_contract::file_history_merge::FileHistorySourceRequest,
        cancellation: HistoryWindowCancellation,
    ) -> Result<FileHistorySourceRead> {
        let read = self.capture_history_window(request.history_request(), cancellation)?;
        Ok(FileHistorySourceRead { read, request })
    }
    pub fn recheck_file_history_source(&self, read: &FileHistorySourceRead) -> Result<()> {
        self.recheck_history_window(&read.read)
    }
}

/// One bounded, same-history observation of equal nonempty content metadata.
/// This holds no content lease and cannot authorize association, merge or purge.
pub struct ContentMatchRead {
    read: HistoryWindowRead,
    request: storage_contract::content_matches::ContentMatchRequest,
}
impl ContentMatchRead {
    /// Prepare outside the Engine lock, then recheck before exposing the page.
    /// O(retained versions) scan time, O(page limit) additional result storage.
    /// Fails for cancellation, a node outside the admitted file/history, or a
    /// continuation that is not a matching version of another file.
    pub fn page(&self) -> Result<storage_contract::content_matches::ContentMatchPage> {
        self.read.cancellation.checkpoint()?;
        let preparation = self
            .read
            .preparation
            .lock()
            .expect("invariant: history preparation lock poisoned");
        let PagePreparation::Pending { view, .. } = &*preparation else {
            unreachable!("invariant: match-only read never prepares an ordinary history page");
        };
        let query = query::Query::new_with_owner(
            view,
            &self.read.request,
            &self.read.context,
            &self.read.cancellation,
            Some(&self.read.owner_version),
        )?;
        super::content_matches::project(&query, &self.request)
    }
}
impl Engine {
    /// Capture a currently admitted file/history root. The service supplies current
    /// authentication; no body reads or persistent mutations occur here. Missing
    /// ownership, stale roots and cancellation retain the history-read errors.
    pub fn capture_content_matches(
        &self,
        request: storage_contract::content_matches::ContentMatchRequest,
        cancellation: HistoryWindowCancellation,
    ) -> Result<ContentMatchRead> {
        let read = self.capture_history_window(request.history_request(), cancellation)?;
        Ok(ContentMatchRead { read, request })
    }
    /// Reject cancellation or a changed current ownership/context/history root.
    /// Authentication is rechecked separately by the service under this lock.
    pub fn recheck_content_matches(&self, read: &ContentMatchRead) -> Result<()> {
        self.recheck_history_window(&read.read)
    }
}

/// A complete bounded automatic component, retaining separate file/event IDs.
/// Equality edges do not confer mutation authority or a body-read lease.
pub struct SharedHistoryRead {
    read: HistoryWindowRead,
    request: storage_contract::shared_history::SharedHistoryRequest,
}
impl SharedHistoryRead {
    /// Prepare outside the Engine lock, then recheck before exposing the page.
    /// Index and scan budgets bound CPU and additional storage. An oversized
    /// component fails rather than presenting a partial graph as complete.
    pub fn graph(&self) -> Result<storage_contract::shared_history::SharedHistoryGraph> {
        self.with_query(|query| super::shared_history::project(query, &self.request))
    }

    /// Analyze a hypothetical coupled body/event retirement, not authorize it.
    /// Requires the same final recheck as graph(). Current immutable references,
    /// leases, publication/recovery and physical cleanup are NOT discharged.
    pub fn permanent_removal_analysis(
        &self,
        selection: &super::PermanentNodeRemovalSelection,
    ) -> Result<super::PermanentNodeRemovalAnalysis> {
        self.with_query(|query| {
            let graph = super::shared_history::project(query, &self.request)?;
            super::permanent_removal::analyze(query, &graph, selection)
        })
    }

    fn with_query<T>(&self, project: impl FnOnce(&query::Query<'_>) -> Result<T>) -> Result<T> {
        self.read.cancellation.checkpoint()?;
        let preparation = self
            .read
            .preparation
            .lock()
            .expect("invariant: history preparation lock poisoned");
        let PagePreparation::Pending { view, .. } = &*preparation else {
            unreachable!("invariant: shared-history read never prepares an ordinary history page");
        };
        let query = query::Query::new_with_owner(
            view,
            &self.read.request,
            &self.read.context,
            &self.read.cancellation,
            Some(&self.read.owner_version),
        )?;
        project(&query)
    }
}
impl Engine {
    /// Capture a currently admitted file/history root. The service supplies current
    /// authentication; no body reads or persistent mutations occur here. Missing
    /// ownership, stale roots and cancellation retain the history-read errors.
    pub fn capture_shared_history(
        &self,
        request: storage_contract::shared_history::SharedHistoryRequest,
        cancellation: HistoryWindowCancellation,
    ) -> Result<SharedHistoryRead> {
        let read = self.capture_history_window(request.history_request(), cancellation)?;
        Ok(SharedHistoryRead { read, request })
    }
    /// Reject cancellation or a changed current ownership/context/history root.
    /// Authentication is rechecked separately by the service under this lock.
    pub fn recheck_shared_history(&self, read: &SharedHistoryRead) -> Result<()> {
        self.recheck_history_window(&read.read)
    }

    /// Inspect authenticated body cleanup on the exact root's current routes.
    /// This bounded synchronous read performs disk I/O, not cleanup or retirement.
    /// The caller supplies current authorization and keeps the Engine owner alive.
    /// Offline, malformed, missing-evidence and residual-body copies never certify
    /// absence. An observation does not cover retired disks or external copies.
    pub fn inspect_permanent_removal_cleanup(
        &self,
        read: &SharedHistoryRead,
        selection: &super::PermanentNodeRemovalSelection,
    ) -> Result<super::PermanentRemovalCleanupObservation> {
        self.recheck_shared_history(read)?;
        let analysis = read.permanent_removal_analysis(selection)?;
        let result =
            super::permanent_removal_cleanup::inspect(self, &analysis, &read.read.cancellation)?;
        self.recheck_shared_history(read)?;
        Ok(result)
    }
}

/// Publication and body cleanup have separate outcomes. A published retirement
/// is permanent even when an offline current replica needs later cleanup.
#[derive(Debug)]
pub struct PermanentNodeRetirementOutcome {
    publication: crate::OperationResult,
    separation: Result<crate::PermanentNodeSeparation>,
    cleanup: Result<super::PermanentRemovalCleanupObservation>,
}
impl PermanentNodeRetirementOutcome {
    pub fn publication(&self) -> &crate::OperationResult { &self.publication }
    pub fn separation(&self) -> std::result::Result<&crate::PermanentNodeSeparation, &Error> {
        self.separation.as_ref()
    }
    pub fn cleanup(&self) -> std::result::Result<&super::PermanentRemovalCleanupObservation, &Error> {
        self.cleanup.as_ref()
    }
}

impl Engine {
    /// Permanently retire an explicit selection from one complete, current
    /// shared component. Old immutable descriptions remain authentication
    /// evidence, not live nodes. Trusted callers supply current authorization.
    /// HTTP/UI adapters keep their confirmation and current-session admission.
    pub fn retire_file_nodes(
        &mut self,
        read: &SharedHistoryRead,
        selection: &super::PermanentNodeRemovalSelection,
        principal: &str,
        operation_id: crate::OperationId,
    ) -> Result<PermanentNodeRetirementOutcome> {
        let request = crate::node_retirement::Request {
            history_tree_id: read.read.context.history_tree_id.as_str().into(),
            history_root: read.request.history_root.to_string(),
            origin_file_id: read.request.file_id.as_str().into(),
            version_ids: selection.versions().iter().map(|id| id.as_str().to_owned()).collect(),
        };
        let publication = self.mutate(
            crate::node_retirement::KIND, principal, operation_id, json_codec::to_value(&request)?,
            |engine, next, _| {
                engine.recheck_shared_history(read)?;
                let analysis = read.permanent_removal_analysis(selection)?;
                if !analysis.affected_branch_heads().is_empty() || !analysis.frozen_files().is_empty() {
                    return Err(Error::new(storage_contract::ErrorCode::ContentPinned, "retirement selection has a retained HEAD or frozen file"));
                }
                engine.repository_for_history(&request.history_tree_id)?;
                engine.check_purge(&request.history_tree_id, &request.version_ids)?;
                // Open workspaces may hold source selections not yet present in
                // their base commit. Conservatively close this entire history.
                if engine.workspaces.values().any(|workspace| {
                    matches!(workspace.state, crate::WorkspaceState::Open | crate::WorkspaceState::Sealed)
                        && engine.state.branches.get(&workspace.branch_id)
                            .is_some_and(|branch| branch.history_tree_id == request.history_tree_id)
                }) {
                    return Err(Error::new(storage_contract::ErrorCode::WorkspaceBusy, "retirement history has an open workspace"));
                }
                let decision = crate::node_retirement::decide(&engine.state, request.clone())?;
                for id in &request.version_ids { next.content_states.insert(id.clone(), crate::ContentState::Retired); }
                Ok(json_codec::json!({"retirement": decision}))
            },
        )?;
        Ok(self.finish_node_retirement(publication, &read.read.cancellation))
    }

    /// Resume the exact authenticated decision after interruption/restart. No
    /// fresh selection, stale root, new operation ID or duplicate receipt is used.
    pub fn resume_file_node_retirement(
        &mut self, operation_id: crate::OperationId, principal: &str,
    ) -> Result<PermanentNodeRetirementOutcome> {
        Self::require_actor(principal)?;
        self.require_write()?;
        let publication = self.state.operations.get(&operation_id)
            .filter(|receipt| receipt.kind == crate::node_retirement::KIND)
            .ok_or_else(|| Error::new(storage_contract::ErrorCode::InvalidRequest, "retirement operation not found"))?;
        if self.operation_owner(operation_id)?.as_deref() != Some(principal) {
            return Err(Error::new(storage_contract::ErrorCode::IdempotencyKeyReused, "retirement operation belongs to another actor"));
        }
        Ok(self.finish_node_retirement(publication.clone(), &HistoryWindowCancellation::default()))
    }

    fn finish_node_retirement(
        &mut self, publication: crate::OperationResult, cancel: &HistoryWindowCancellation,
    ) -> PermanentNodeRetirementOutcome {
        let separation = crate::node_retirement::decision(&publication)
            .and_then(|decision| decision.separation_observation(&self.state));
        let cleanup = (|| {
            let decision = crate::node_retirement::decision(&publication)?;
            self.cleanup_purged()?;
            let view = self.metadata_view();
            let root = HistoryRoot::parse(view.history_root(&decision.request.history_tree_id)?)?;
            let versions = decision.request.version_ids.iter()
                .map(|id| HistoryVersionId::parse(id))
                .collect::<storage_contract::Result<Vec<_>>>()?;
            super::permanent_removal_cleanup::inspect_required(self, root, &versions, &decision.cleanup_routes, cancel)
        })();
        PermanentNodeRetirementOutcome { publication, separation, cleanup }
    }
}

impl Engine {
    /// Transport retry of the original selection, before recapturing its now-old
    /// root. A same-key different file/root/version list must never resume cleanup.
    pub fn retry_file_node_retirement_at_root(
        &mut self, operation_id: crate::OperationId, principal: &str,
        file: &HistoryFileId, root: HistoryRoot, selection: &super::PermanentNodeRemovalSelection,
    ) -> Result<PermanentNodeRetirementOutcome> {
        Self::require_actor(principal)?;
        if self.operation_owner(operation_id)?.as_deref() != Some(principal) {
            return Err(Error::new(storage_contract::ErrorCode::IdempotencyKeyReused, "retirement operation belongs to another actor"));
        }
        let receipt = self.state.operations.get(&operation_id)
            .filter(|receipt| receipt.kind == crate::node_retirement::KIND)
            .ok_or_else(|| Error::new(storage_contract::ErrorCode::IdempotencyKeyReused, "operation is not this retirement request"))?;
        let decision = crate::node_retirement::decision(receipt)?;
        if decision.request.origin_file_id != file.as_str() || decision.request.history_root != root.to_string()
            || !decision.request.version_ids.iter().map(String::as_str).eq(selection.versions().iter().map(HistoryVersionId::as_str)) {
            return Err(Error::new(storage_contract::ErrorCode::IdempotencyKeyReused, "retirement operation is bound to another file, root or version selection"));
        }
        self.resume_file_node_retirement(operation_id, principal)
    }

    /// Resume only the original file's operation. A detail URL cannot silently
    /// redirect recovery to another file, even for the same administrator.
    pub fn resume_file_node_retirement_for_file(
        &mut self, file: &HistoryFileId, operation_id: crate::OperationId, principal: &str,
    ) -> Result<PermanentNodeRetirementOutcome> {
        Self::require_actor(principal)?;
        if self.operation_owner(operation_id)?.as_deref() != Some(principal) {
            return Err(Error::new(storage_contract::ErrorCode::IdempotencyKeyReused, "retirement operation belongs to another actor"));
        }
        let receipt = self.state.operations.get(&operation_id)
            .filter(|receipt| receipt.kind == crate::node_retirement::KIND)
            .ok_or_else(|| Error::new(storage_contract::ErrorCode::InvalidRequest, "retirement operation not found"))?;
        if crate::node_retirement::decision(receipt)?.request.origin_file_id != file.as_str() {
            return Err(Error::new(storage_contract::ErrorCode::InvalidRequest, "retirement operation belongs to another file"));
        }
        self.resume_file_node_retirement(operation_id, principal)
    }
}
