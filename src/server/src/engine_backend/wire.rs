//! Engine errors, request fields, and public response projections.
use crate::ApiError;
use data_serialization::Serialize;
use json_codec::{Value, json};
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
/// A missing stored disposition has the same available-content semantics as
/// core reads. This is distinct from LIVE/PENDING branch presentation states.
pub(super) fn stored_version_content_state(
    state: Option<&storage_contract::ContentState>,
) -> &'static str {
    state
        .copied()
        .unwrap_or(storage_contract::ContentState::Available)
        .as_str()
}
/// Response ownership is independent from the version currently visible in a
/// branch. An unwritten branch reads its parent's immutable version.
pub(super) fn content_wire(engine: &Engine, visible: Option<&str>, source: Option<&str>) -> Value {
    let visible = source.or(visible).filter(|version| !version.is_empty());
    let inherited = source.is_some();
    let state = visible
        .and_then(|version| engine.state().content_states.get(version))
        .map(storage_contract::ContentState::as_str)
        .unwrap_or("LIVE");
    json!({"version_id":visible,"materialized":!inherited && visible.is_some(),
        "inherited":inherited,"own_version_id":(!inherited).then_some(visible).flatten(),
        "source_version_id":source,"content_state":if inherited { "PENDING" } else { state },
        "visible_content_state":state})
}
pub(super) fn branch_wire(
    engine: &Engine,
    branch: &storage_contract::Branch,
) -> Result<Value, ApiError> {
    let mut result = encode(branch)?;
    let snapshot = engine.state().commits.get(&branch.head_commit_id);
    let mut pending = snapshot
        .map(|commit| commit.pending_file_versions.clone())
        .unwrap_or_default();
    pending.extend(branch.pending_file_versions.clone());
    result["pending_file_versions"] = encode(&pending)?;
    result["materialized"] = pending.is_empty().into();
    result["inherited"] = (!pending.is_empty()).into();
    result["content_state"] = if pending.is_empty() {
        "LIVE"
    } else {
        "PENDING"
    }
    .into();
    if let Some(file) = branch.file_id.as_deref() {
        result["read_only"] = (branch.state != "active"
            || engine
                .state()
                .file_branch_aliases
                .contains_key(&branch.branch_id))
        .into();
        result["alias_of"] = engine
            .state()
            .file_branch_aliases
            .get(&branch.branch_id)
            .cloned()
            .into();
        let visible = snapshot
            .and_then(|commit| commit.entries.iter().find(|entry| entry.file_id == file))
            .map(|entry| entry.version_id.as_str());
        result["node_id"] = branch
            .file_node_id
            .clone()
            .or_else(|| visible.map(str::to_owned))
            .into();
        result.as_object_mut().unwrap().extend(
            content_wire(engine, visible, pending.get(file).map(String::as_str))
                .as_object()
                .unwrap()
                .clone(),
        );
    }
    Ok(result)
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
            visible.lookup_key == entry.lookup_key && visible.file_id == entry.file_id
        })
    }

    pub(super) fn checked(&self, entry: &storage_contract::Entry) -> Option<&'a str> {
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return None;
        }
        self.resolve(entry, self.visible(entry)?, CurrentContext::Lookup, None)
    }

    pub(super) fn pending_versions(&self) -> std::collections::BTreeMap<String, String> {
        // Published trees are sorted with unique normalized keys. If a caller
        // supplies a noncanonical snapshot, preserve the old first-match rule.
        let canonical = self
            .commit
            .entries
            .windows(2)
            .all(|pair| pair[0].lookup_key < pair[1].lookup_key)
            && self
                .commit
                .entries
                .iter()
                .all(|entry| entry.lookup_key == name_policy::path_key(&entry.path));
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
                let visible = if canonical {
                    entry
                } else {
                    self.visible(entry)?
                };
                let context = self.main_repository.and_then(|repository| {
                    if canonical {
                        self.engine
                            .file_context_for_current_entry(repository, entry)
                    } else {
                        self.engine
                            .file_context(repository, &entry.path)
                            .map(std::borrow::Cow::Owned)
                    }
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
                Some(contexts) => contexts.get(entry.lookup_key.as_str()).copied(),
                None => state
                    .directory_trees
                    .values()
                    .filter(|tree| tree.commit_id == self.commit.commit_id)
                    .find_map(|tree| tree.file_contexts.get(&entry.lookup_key)),
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
/// Only an absent identity selects the caller's default. Explicit null or a
/// malformed value must not silently change a historical request to HEAD.
pub(super) fn optional_identity<'a>(
    value: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, ApiError> {
    value
        .get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ApiError::invalid(format!("{key} must be a nonempty string when supplied"))
                })
        })
        .transpose()
}
pub(super) fn operation(input: &Value) -> Result<&str, ApiError> {
    field(input, "operation_id")
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
        _ => {}
    }
}
pub(super) fn operation_wire(op: storage_engine::OperationResult) -> Result<Value, ApiError> {
    let health = match op.current_health.as_str() {
        "HEALTHY" | "DEGRADED" | "UNAVAILABLE" | "UNKNOWN" => op.current_health.as_str(),
        _ => "UNKNOWN",
    };
    let current_health = json!({"state":health,"checked_at":date_time::OffsetDateTime::now_utc().format(&date_time::format_description::well_known::Rfc3339).map_err(ApiError::invalid_source)?});
    let content_save = op.kind == "workspace.commit"
        && op.protection_at_publish != storage_contract::ProtectionAtPublish::NotApplicable;
    if content_save {
        let complete = matches!(
            op.commit_state,
            storage_contract::CommitState::Published
                | storage_contract::CommitState::Acked
                | storage_contract::CommitState::PublishedDegraded
        );
        let state = if complete {
            "completed"
        } else if op.commit_state == storage_contract::CommitState::Aborted {
            "failed"
        } else if matches!(
            op.commit_state,
            storage_contract::CommitState::InDoubt
                | storage_contract::CommitState::RecoveryRequired
        ) {
            "in_doubt"
        } else {
            "processing"
        };
        let result = if complete {
            let versions = op
                .result
                .get("version_protection")
                .filter(|versions| versions.is_array())
                .ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::StateInvalid,
                        "frozen publication protection evidence is missing",
                    )
                })?;
            json!({"outcome":if op.protection_at_publish==storage_contract::ProtectionAtPublish::TargetMet{"COMMITTED_AND_PROTECTED"}else{"COMMITTED_DEGRADED"},"commit_id":op.commit_id,"branch_id":op.branch_id,"branch_generation":field(&op.result,"branch_generation")?,"publication_sequence":op.sequence.to_string(),"membership_epoch":field(&op.result,"membership_epoch")?,"protection_at_publish":op.protection_at_publish,"versions":versions})
        } else {
            Value::Null
        };
        let error = if op.commit_state == storage_contract::CommitState::Aborted {
            encode(ApiError::conflict(
                storage_contract::ErrorCode::OperationAborted,
                "the operation was aborted before publication",
            ))?
        } else {
            Value::Null
        };
        Ok(
            json!({"operation_id":op.operation_id,"operation_kind":"content_save","request_digest":op.request_digest,"state":state,"commit_state":op.commit_state,"result":result,"error":error,"current_health":current_health}),
        )
    } else {
        Ok(
            json!({"operation_id":op.operation_id,"operation_kind":op.kind,"request_digest":op.request_digest,"state":if matches!(op.commit_state,storage_contract::CommitState::Published|storage_contract::CommitState::Acked){"completed"}else{"in_doubt"},"commit_state":op.commit_state,"protection_at_publish":op.protection_at_publish,"publication_sequence":op.sequence.to_string(),"result":op.result,"error":null,"current_health":current_health}),
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/wire_tests.rs"]
mod tests;
