//! Engine errors, request fields, and public response projections.
use crate::ApiError;
use data_serialization::Serialize;
use json_codec::Value;
use storage_engine::Engine;

pub(crate) fn error(error: storage_engine::Error) -> ApiError {
    let code = super::error_registry::public_code(error.code());
    let status = super::error_registry::status(code);
    if status == ::http::StatusCode::INTERNAL_SERVER_ERROR {
        ApiError::new(
            status,
            code,
            "storage operation could not be completed; administrator inspection is required",
        )
        .with_source(error)
    } else {
        let mut mapped = ApiError::new(status, code, error.message());
        mapped.retryable |= error.is_retryable();
        mapped.with_source(error)
    }
}

pub(super) fn encode(value: impl Serialize) -> Result<Value, ApiError> {
    Ok(json_codec::to_value(value).expect("invariant: internal response serialization succeeds"))
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
/// Response ownership is independent from the version currently visible in a
/// branch. An unwritten branch reads its parent's immutable version.
pub(super) fn content_wire(engine: &Engine, visible: Option<&str>, source: Option<&str>) -> Value {
    json_codec::to_value(crate::backend_branch::ContentDispositionResponse::from(
        super::branch::capture_content(engine.state(), visible, source),
    ))
    .expect("invariant: internal content response serialization succeeds")
}

pub(super) fn branch_wire(
    engine: &Engine,
    branch: &storage_contract::Branch,
) -> Result<Value, ApiError> {
    encode(crate::backend_branch::BranchResponse::from(
        super::branch::capture(engine.state(), branch),
    ))
}

enum CurrentContext<'a> {
    Lookup,
    // A missing context has already been resolved; do not search again.
    Resolved(Option<&'a storage_contract::FileContext>),
}

/// Borrowed, request-local source resolution. Only `pending_versions` can use
/// the direct-entry path, because it iterates this resolver's own snapshot.
/// External entries always retain the checked first-match lookup semantics.
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

    fn visible(&self, entry: &storage_contract::Entry) -> Option<&'a storage_contract::Entry> {
        self.commit.entries.iter().find(|visible| {
            #[cfg(test)]
            self.matching_visits.set(self.matching_visits.get() + 1);
            visible.lookup_key() == entry.lookup_key() && visible.file_id == entry.file_id
        })
    }

    pub(super) fn checked(&self, entry: &storage_contract::Entry) -> Option<&'a str> {
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return None;
        }
        self.resolve(entry, self.visible(entry)?, CurrentContext::Lookup, None)
    }

    pub(super) fn pending_versions(&self) -> std::collections::BTreeMap<String, String> {
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
        self.commit
            .entries
            .iter()
            .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
            .filter_map(|entry| {
                let visible = entry;
                let context = self.main_repository.and_then(|repository| {
                    self.engine
                        .file_context_for_current_entry(repository, entry)
                });
                self.resolve(
                    entry,
                    visible,
                    CurrentContext::Resolved(context.as_deref()),
                    historical.as_ref(),
                )
                .map(|source| (entry.file_id.clone(), source.to_owned()))
            })
            .collect()
    }

    fn resolve(
        &self,
        entry: &storage_contract::Entry,
        visible: &'a storage_contract::Entry,
        context: CurrentContext<'_>,
        historical: Option<&std::collections::BTreeMap<&str, &'a storage_contract::FileContext>>,
    ) -> Option<&'a str> {
        let state = self.engine.state();
        let current = self.current?;
        if current.head_commit_id == self.commit.commit_id {
            if let Some(repository) = self.main_repository {
                let owned;
                let context = match context {
                    CurrentContext::Resolved(context) => context,
                    CurrentContext::Lookup => {
                        owned = self.engine.file_context(repository, &entry.path);
                        owned.as_ref()
                    }
                };
                if let Some(context) = context {
                    return if context.inherited_from.is_some() {
                        Some(visible.version_id.as_str())
                    } else {
                        state
                            .branches
                            .get(&context.branch_id)?
                            .pending_file_versions
                            .get(&entry.file_id)
                            .map(String::as_str)
                    };
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
                return context
                    .inherited_from
                    .as_ref()
                    .map(|_| visible.version_id.as_str());
            }
        }
        self.commit
            .pending_file_versions
            .get(&entry.file_id)
            .filter(|version| **version == entry.version_id)
            .map(String::as_str)
            .or_else(|| {
                (current.head_commit_id == self.commit.commit_id)
                    .then(|| self.engine.pending_file_source(self.branch, &entry.file_id))
                    .flatten()
            })
    }

    #[cfg(test)]
    pub(super) fn matching_visits(&self) -> usize {
        self.matching_visits.get()
    }
}

pub(super) fn pending_entry_source<'a>(
    engine: &'a Engine,
    branch: &'a str,
    commit: &'a storage_contract::Commit,
    entry: &storage_contract::Entry,
) -> Option<&'a str> {
    PendingSources::new(engine, branch, commit).checked(entry)
}

pub(super) fn current_entry_source<'a>(
    engine: &'a Engine,
    branch: &'a str,
    entry: &storage_contract::Entry,
) -> Option<&'a str> {
    let current = engine.state().branches.get(branch)?;
    let commit = engine.state().commits.get(&current.head_commit_id)?;
    pending_entry_source(engine, branch, commit, entry)
}

/// The index resolves against this resolver's own current commit. Unlike the
/// checked external-entry API, callers cannot substitute another snapshot's
/// entry and no repeated path scan is necessary.
pub(super) fn current_entry_source_at_index<'a>(
    engine: &'a Engine,
    branch: &'a str,
    index: usize,
    context: Option<&storage_contract::FileContext>,
) -> Option<&'a str> {
    let current = engine.state().branches.get(branch)?;
    let commit = engine.state().commits.get(&current.head_commit_id)?;
    let entry = commit.entries.get(index)?;
    if entry.kind != storage_contract::NamespaceEntryKind::File {
        return None;
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
pub(super) fn operation(input: &Value) -> Result<storage_contract::OperationId, ApiError> {
    field(input, "operation_id")?
        .parse::<storage_contract::OperationId>()
        .map_err(|source| error(source.into()))
}
pub(super) fn keys(input: &Value, allowed: &[&str]) -> Result<(), ApiError> {
    let object = input
        .as_object()
        .ok_or_else(|| ApiError::invalid("expected object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    Ok(())
}
pub(super) fn sanitized<T: Serialize>(value: T) -> Result<Value, ApiError> {
    let mut value = encode(value)?;
    redact(&mut value);
    Ok(value)
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("source_path");
            for value in object.values_mut() {
                redact(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
pub(super) use crate::operation_wire::operation_wire;

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/wire_tests.rs"]
mod tests;
