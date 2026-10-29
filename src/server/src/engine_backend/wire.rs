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

/// Borrowed, request-local source resolution. Only `pending_versions` and the
/// current-entry resolver use the direct-entry path, because their entries are
/// this resolver's own snapshot materializations. External entries always
/// retain the checked point-lookup semantics against the commit namespace.
pub(super) struct PendingSources<'a> {
    engine: &'a Engine,
    branch: &'a str,
    commit: &'a storage_contract::Commit,
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
    ) -> Self {
        let state = engine.state();
        let current = state.branches.get(branch);
        let main_repository = current
            .filter(|current| current.head_commit_id == commit.commit_id)
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
            current,
            main_repository,
            #[cfg(test)]
            matching_visits: std::cell::Cell::new(0),
        }
    }

    /// One checked point read of this resolver's commit namespace: the entry's
    /// normalized key must resolve to the same file identity. Never a scan.
    fn visible(
        &self,
        entry: &storage_contract::Entry,
    ) -> Result<Option<storage_contract::Entry>, storage_engine::Error> {
        #[cfg(test)]
        self.matching_visits.set(self.matching_visits.get() + 1);
        let Some(tree) = self.engine.commit_namespace(&self.commit.commit_id) else {
            return Ok(None);
        };
        Ok(tree
            .get_key(entry.lookup_key())?
            .filter(|visible| visible.file_id == entry.file_id))
    }

    pub(super) fn checked(
        &self,
        entry: &storage_contract::Entry,
    ) -> Result<Option<String>, storage_engine::Error> {
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return Ok(None);
        }
        let Some(visible) = self.visible(entry)? else {
            return Ok(None);
        };
        self.resolve(entry, &visible, CurrentContext::Lookup, None)
    }

    pub(super) fn pending_versions(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, storage_engine::Error> {
        let historical = self
            .current
            .filter(|current| current.head_commit_id != self.commit.commit_id)
            .map(|_| {
                let mut contexts = std::collections::BTreeMap::new();
                for tree in self
                    .engine
                    .state()
                    .directory_trees
                    .values()
                    .filter(|tree| tree.commit_id == self.commit.commit_id)
                {
                    for (key, context) in &tree.file_contexts {
                        // BTreeMap iteration preserves the original first tree.
                        contexts.entry(key.as_str()).or_insert(context);
                    }
                }
                contexts
            });
        let Some(tree) = self.engine.commit_namespace(&self.commit.commit_id) else {
            return Ok(std::collections::BTreeMap::new());
        };
        let mut versions = std::collections::BTreeMap::new();
        tree.walk(|entry| {
            if entry.kind != storage_contract::NamespaceEntryKind::File {
                return Ok(true);
            }
            let context = self.main_repository.and_then(|repository| {
                self.engine
                    .file_context_for_current_entry(repository, entry)
            });
            if let Some(source) = self.resolve(
                entry,
                entry,
                CurrentContext::Resolved(context.as_deref()),
                historical.as_ref(),
            )? {
                versions.insert(entry.file_id.clone(), source);
            }
            Ok(true)
        })?;
        Ok(versions)
    }

    fn resolve(
        &self,
        entry: &storage_contract::Entry,
        visible: &storage_contract::Entry,
        context: CurrentContext<'_>,
        historical: Option<&std::collections::BTreeMap<&str, &'a storage_contract::FileContext>>,
    ) -> Result<Option<String>, storage_engine::Error> {
        let state = self.engine.state();
        let Some(current) = self.current else {
            return Ok(None);
        };
        if current.head_commit_id == self.commit.commit_id {
            if let Some(repository) = self.main_repository {
                let owned;
                let context = match context {
                    CurrentContext::Resolved(context) => context,
                    CurrentContext::Lookup => {
                        owned = self.engine.file_context(repository, &entry.path)?;
                        owned.as_ref()
                    }
                };
                if let Some(context) = context {
                    return Ok(if context.inherited_from.is_some() {
                        Some(visible.version_id.clone())
                    } else {
                        state.branches.get(&context.branch_id).and_then(|branch| {
                            branch.pending_file_versions.get(&entry.file_id).cloned()
                        })
                    });
                }
            }
        } else {
            let context = match historical {
                Some(contexts) => contexts.get(entry.lookup_key()).copied(),
                None => state
                    .directory_trees
                    .values()
                    .filter(|tree| tree.commit_id == self.commit.commit_id)
                    .find_map(|tree| tree.file_contexts.get(entry.lookup_key())),
            };
            if let Some(context) = context {
                return Ok(context
                    .inherited_from
                    .as_ref()
                    .map(|_| visible.version_id.clone()));
            }
        }
        Ok(self
            .commit
            .pending_file_versions
            .get(&entry.file_id)
            .filter(|version| **version == entry.version_id)
            .cloned()
            .or_else(|| {
                (current.head_commit_id == self.commit.commit_id)
                    .then(|| {
                        self.engine
                            .pending_file_source(self.branch, &entry.file_id)
                            .map(str::to_owned)
                    })
                    .flatten()
            }))
    }

    #[cfg(test)]
    pub(super) fn matching_visits(&self) -> usize {
        self.matching_visits.get()
    }
}

pub(super) fn pending_entry_source(
    engine: &Engine,
    branch: &str,
    commit: &storage_contract::Commit,
    entry: &storage_contract::Entry,
) -> Result<Option<String>, storage_engine::Error> {
    PendingSources::new(engine, branch, commit).checked(entry)
}

/// The entry is this branch's own current-commit materialization, already in
/// hand at the caller. Unlike the checked external-entry API, callers cannot
/// substitute another snapshot's entry and no repeated key lookup is needed.
pub(super) fn current_entry_source_resolved(
    engine: &Engine,
    branch: &str,
    entry: &storage_contract::Entry,

    context: Option<&storage_contract::FileContext>,
) -> Result<Option<String>, storage_engine::Error> {
    let Some(current) = engine.state().branches.get(branch) else {
        return Ok(None);
    };
    let Some(commit) = engine.state().commits.get(&current.head_commit_id) else {
        return Ok(None);
    };
    if entry.kind != storage_contract::NamespaceEntryKind::File {
        return Ok(None);
    }
    PendingSources::new(engine, branch, commit).resolve(
        entry,
        entry,
        CurrentContext::Resolved(context),
        None,
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
