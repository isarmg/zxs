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
/// Source selection over one immutable State; callers supply their own checked
/// entry/context and fallback lookup, without granting authority or doing I/O.
pub(super) struct PendingResolution<'a> {
    state: &'a storage_contract::State,
    commit: &'a storage_contract::Commit,
    current: Option<&'a storage_contract::Branch>,
    pub(super) main_repository: Option<&'a str>,
}

pub(super) struct PendingContexts<'a> {
    pub(super) current: Option<&'a storage_contract::FileContext>,
    pub(super) historical: Option<&'a storage_contract::FileContext>,
}

impl<'a> PendingResolution<'a> {
    pub(super) fn new(
        state: &'a storage_contract::State,
        branch: &str,
        commit: &'a storage_contract::Commit,
    ) -> Self {
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
            state,
            commit,
            current,
            main_repository,
        }
    }

    pub(super) fn historical(&self) -> bool {
        self.current
            .is_some_and(|current| current.head_commit_id != self.commit.commit_id)
    }

    pub(super) fn resolve<'e>(
        &'e self,
        entry: &'e storage_contract::Entry,
        visible: &'e storage_contract::Entry,
        contexts: PendingContexts<'_>,
        fallback: impl FnOnce() -> Option<&'e str>,
    ) -> Option<&'e str> {
        let current = self.current?;
        if current.head_commit_id == self.commit.commit_id {
            if self.main_repository.is_some()
                && let Some(context) = contexts.current
            {
                return if context.inherited_from.is_some() {
                    Some(visible.version_id.as_str())
                } else {
                    self.state
                        .branches
                        .get(&context.branch_id)
                        .and_then(|branch| {
                            branch
                                .pending_file_versions
                                .get(&entry.file_id)
                                .map(String::as_str)
                        })
                };
            }
        } else if let Some(context) = contexts.historical {
            return context
                .inherited_from
                .as_ref()
                .map(|_| visible.version_id.as_str());
        }
        self.commit
            .pending_file_versions
            .get(&entry.file_id)
            .filter(|version| **version == entry.version_id)
            .map(String::as_str)
            .or_else(|| {
                (current.head_commit_id == self.commit.commit_id)
                    .then(fallback)
                    .flatten()
            })
    }
}

/// Borrowed, request-local source resolution for external entries, retaining
/// checked point-lookup semantics against the commit namespace.
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
        let source = PendingResolution::new(engine.state(), branch, commit);
        Self {
            engine,
            branch,
            commit,
            current: source.current,
            main_repository: source.main_repository,
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
        let state = self.engine.state();
        let source = PendingResolution {
            state,
            commit: self.commit,
            current: self.current,
            main_repository: self.main_repository,
        };
        let context = match self.main_repository {
            Some(repository) => self.engine.file_context(repository, &entry.path)?,
            None => None,
        };
        let historical = source
            .historical()
            .then(|| {
                state
                    .directory_trees
                    .values()
                    .filter(|tree| tree.commit_id == self.commit.commit_id)
                    .find_map(|tree| tree.file_contexts.get(entry.lookup_key()))
            })
            .flatten();
        Ok(source
            .resolve(
                entry,
                &visible,
                PendingContexts {
                    current: context.as_ref(),
                    historical,
                },
                || self.engine.pending_file_source(self.branch, &entry.file_id),
            )
            .map(str::to_owned))
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
