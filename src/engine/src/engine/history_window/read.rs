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
