//! Engine errors, request fields, and public response projections.
use crate::ApiError;
use json_codec::Value;
use storage_engine::Engine;

pub(crate) fn error(error: storage_engine::Error) -> ApiError {
    error_response(&error).with_source(error)
}

/// Borrowed HTTP projection for retained reports; diagnostic sources stay local.
pub(crate) fn error_response(error: &storage_engine::Error) -> ApiError {
    if let Some(published) = error.published_result() {
        let mut mapped = ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::RecoveryRequired,
            format!(
                "operation {} is already published; local reconciliation is required and writes remain blocked; retain the original operation instead of submitting a new one",
                published.operation_id
            ),
        );
        // This is not an invitation to repeat the already published mutation.
        mapped.retryable = false;
        return mapped;
    }
    let code = super::error_registry::public_code(error.code());
    let status = super::error_registry::status(code);
    if status == ::http::StatusCode::INTERNAL_SERVER_ERROR {
        ApiError::new(
            status,
            code,
            "storage operation could not be completed; administrator inspection is required",
        )
    } else {
        let mut mapped = ApiError::new(status, code, error.message());
        mapped.retryable |= error.is_retryable();
        mapped
    }
}

/// Sparse disposition absence means available, exactly as core reads; it does
/// not represent a missing version or failed authorization.
pub(super) fn stored_version_content_state(
    state: Option<&storage_contract::ContentState>,
) -> storage_contract::ContentState {
    state
        .copied()
        .unwrap_or(storage_contract::ContentState::Available)
}
enum CurrentContext<'a> {
    Lookup,
    // A missing context has already been resolved; do not search again.
    Resolved(Option<&'a storage_contract::FileContext>),
}

#[derive(Clone, Copy)]
pub(super) enum PendingScope<'a> {
    Current(storage_engine::NamespaceTree<'a>),
    Historical,
}

/// Borrowed, request-local source resolution. Only `pending_versions` can use
/// the direct-entry path, because it iterates this resolver's own snapshot.
/// External entries always retain the checked first-match lookup semantics.
pub(super) struct PendingSources<'a> {
    engine: &'a Engine,
    branch: &'a str,
    commit: &'a storage_contract::Commit,
    entries: &'a [storage_contract::Entry],
    scope: PendingScope<'a>,
    current: Option<&'a storage_contract::Branch>,
    main_repository: Option<&'a str>,
    #[cfg(test)]
    matching_visits: std::cell::Cell<usize>,
}

impl<'a> PendingSources<'a> {
    pub(super) fn new(
        engine: &'a Engine,
        branch: &'a str,
        commit: &'a storage_contract::Commit,
        scope: PendingScope<'a>,
    ) -> Self {
        let state = engine.state();
        let current = state.branches.get(branch);
        let main_repository = current
            .filter(|_| matches!(scope, PendingScope::Current(_)))
            .and_then(|_| {
                state
                    .repositories
                    .values()
                    .find(|repo| repo.main_branch_id == branch)
            })
            .map(|repo| repo.repository_id.as_str());
        Self {
            engine,
            branch,
            commit,
            entries: match scope {
                PendingScope::Current(namespace) => namespace.entries(),
                PendingScope::Historical => &commit.entries,
            },
            scope,
            current,
            main_repository,
            #[cfg(test)]
            matching_visits: std::cell::Cell::new(0),
        }
    }

    fn entry_at(&self, index: usize) -> Option<&'a storage_contract::Entry> {
        match self.scope {
            PendingScope::Current(namespace) => namespace.visible_entry(index),
            PendingScope::Historical => self.entries.get(index),
        }
    }

    fn visible(&self, entry: &storage_contract::Entry) -> Option<&'a storage_contract::Entry> {
        (0..self.entries.len()).find_map(|index| {
            #[cfg(test)]
            self.matching_visits.set(self.matching_visits.get() + 1);
            self.entry_at(index).filter(|visible| visible.lookup_key() == entry.lookup_key()
                && visible.file_id == entry.file_id)
        })
    }

    pub(super) fn checked(&self, entry: &storage_contract::Entry) -> Option<&'a str> {
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return None;
        }
        let visible = self.visible(entry)?;
        if visible.entry_id != entry.entry_id || visible.version_id != entry.version_id {
            return None;
        }
        self.resolve(visible, CurrentContext::Lookup)
    }

    pub(super) fn pending_versions(&self) -> std::collections::BTreeMap<String, String> {
        (0..self.entries.len())
            .filter_map(|index| self.entry_at(index))
            .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
            .filter_map(|entry| {
                let context = self.main_repository.and_then(|repository| {
                    self.engine
                        .file_context_for_current_entry(repository, entry)
                });
                self.resolve(entry, CurrentContext::Resolved(context.as_deref()))
                .map(|source| (entry.file_id.clone(), source.to_owned()))
            })
            .collect()
    }

    fn resolve(
        &self,
        entry: &storage_contract::Entry,
        context: CurrentContext<'_>,
    ) -> Option<&'a str> {
        let state = self.engine.state();
        let branch = self.current?;
        let frozen = || self.commit.pending_file_versions.get(&entry.file_id)
            .filter(|version| **version == entry.version_id).map(String::as_str);
        if matches!(self.scope, PendingScope::Historical) {
            // The original commit is the sole issuer, even when it still
            // happens to be the current HEAD. No current selection fallback.
            return frozen();
        }
        if let Some(repository) = self.main_repository {
            let owned;
            let context = match context {
                CurrentContext::Resolved(context) => context,
                CurrentContext::Lookup => {
                    owned = self.engine.file_context(repository, entry.path.as_str());
                    owned.as_ref()
                }
            };
            if let Some(context) = context {
                let selected = state.branches.get(&context.branch_id)?;
                if selected.history_tree_id != branch.history_tree_id
                    || (selected.branch_id != branch.branch_id
                        && selected.file_id.as_deref() != Some(entry.file_id.as_str())) {
                    return None;
                }
                return selected.pending_file_versions.get(&entry.file_id)
                    .filter(|version| **version == entry.version_id).map(String::as_str);
            }
        }
        // A direct current FILE branch can carry its own pending source.
        frozen().or_else(|| self.engine.pending_file_source(self.branch, &entry.file_id)
            .filter(|version| *version == entry.version_id))
    }

    #[cfg(test)]
    pub(super) fn matching_visits(&self) -> usize {
        self.matching_visits.get()
    }
}

pub(super) fn current_entry_source<'a>(
    engine: &'a Engine,
    branch: &'a str,
    entry: &storage_contract::Entry,
    namespace: storage_engine::NamespaceTree<'a>,
) -> Option<&'a str> {
    let current = engine.state().branches.get(branch)?;
    let commit = engine.state().commits.get(&current.head_commit_id)?;
    PendingSources::new(engine, branch, commit, PendingScope::Current(namespace)).checked(entry)
}

/// The index and context come from this exact admitted current namespace, not
/// the immutable commit whose unfiltered offsets can differ after deregistration.
pub(super) fn current_entry_source_at_index<'a>(
    engine: &'a Engine,
    branch: &'a str,
    index: usize,
    context: Option<&storage_contract::FileContext>,
    namespace: storage_engine::NamespaceTree<'a>,
) -> Option<&'a str> {
    let current = engine.state().branches.get(branch)?;
    let commit = engine.state().commits.get(&current.head_commit_id)?;
    let entry = namespace.visible_entry(index)?;
    if entry.kind != storage_contract::NamespaceEntryKind::File {
        return None;
    }
    PendingSources::new(engine, branch, commit, PendingScope::Current(namespace)).resolve(
        entry,
        CurrentContext::Resolved(context),
    )
}

pub(super) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::invalid(format!("missing string field {key}")))
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/wire_tests.rs"]
mod tests;
