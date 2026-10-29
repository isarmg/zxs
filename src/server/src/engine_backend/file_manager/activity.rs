//! Read-only, bounded activity projection across committed history sources.
use super::file_repo;
use crate::{
    ActivityHistorySnapshot, ApiError,
    backend_activity::{ActivityDetailSnapshot, ActivityEventSnapshot, HISTORY_LIMIT},
    engine_backend::{branch::capture_content, repo_for_history},
};
use json_codec::Value;
use std::{cmp::Reverse, collections::BTreeMap};
use storage_engine::Engine;

// Operation receipts retain an open JSON payload on disk. Parse only fields
// consumed here; malformed retained rows are external state errors, not bugs.
fn optional_field<'a, T: data_serialization::Deserialize<'a>>(
    value: Option<&'a Value>,
) -> Result<Option<T>, ApiError> {
    value
        .filter(|value| !value.is_null())
        .map(|value| {
            T::deserialize(value).map_err(|source| {
                ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::StateInvalid,
                    "history receipt field is invalid",
                )
                .with_source(source)
            })
        })
        .transpose()
}

fn receipt_timestamp(value: Option<&Value>) -> Result<Option<u64>, ApiError> {
    optional_field::<&str>(value)?
        .map(|timestamp| {
            storage_contract::parse_u64_decimal(timestamp).map_err(|source| {
                ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::StateInvalid,
                    "history timestamp is invalid",
                )
                .with_source(source)
            })
        })
        .transpose()
}

enum Timestamp {
    Seconds(u64),
    Resolved(Option<u64>),
}
impl Timestamp {
    fn parse(self) -> Result<Option<u64>, ApiError> {
        match self {
            Self::Seconds(seconds) => Ok(Some(seconds)),
            Self::Resolved(timestamp) => Ok(timestamp),
        }
    }
}

enum PendingDetail<'a> {
    Captured(ActivityDetailSnapshot),
    FileOperation {
        version_id: Option<&'a str>,
        receipt: &'a Value,
    },
    DirectoryOperation {
        branch_id: &'a str,
        head: Option<&'a Value>,
    },
    Backup {
        copies: Option<&'a Value>,
    },
}
impl PendingDetail<'_> {
    fn parse(self) -> Result<ActivityDetailSnapshot, ApiError> {
        Ok(match self {
            Self::Captured(detail) => detail,
            Self::FileOperation {
                version_id,
                receipt,
            } => ActivityDetailSnapshot::FileOperation {
                version_id: match version_id {
                    Some(version) => Some(version.into()),
                    None => optional_field(receipt.get("version_id"))?,
                },
                node_id: optional_field(receipt.get("node_id"))?,
                branch_id: optional_field(receipt.get("branch_id"))?,
            },
            Self::DirectoryOperation { branch_id, head } => {
                ActivityDetailSnapshot::DirectoryOperation {
                    version_id: None,
                    directory_branch_id: branch_id.into(),
                    head_tree_id: optional_field(head)?,
                }
            }
            Self::Backup { copies } => ActivityDetailSnapshot::Backup {
                version_id: None,
                target_copies: optional_field(copies)?,
            },
        })
    }
}

struct PendingEvent<'a> {
    timestamp: Timestamp,
    slot_id: String,
    path: String,
    action: String,
    principal: Option<String>,
    file_id: String,
    message: String,
    detail: PendingDetail<'a>,
}

impl PendingEvent<'_> {
    fn finish(self, sequence: u64, event_id: String) -> Result<ActivityEventSnapshot, ApiError> {
        let Self {
            timestamp,
            slot_id,
            path,
            action,
            principal,
            file_id,
            message,
            detail,
        } = self;
        Ok(ActivityEventSnapshot {
            sequence,
            event_id,
            timestamp: timestamp.parse()?,
            slot_id,
            path,
            action,
            principal,
            file_id,
            message,
            detail: detail.parse()?,
        })
    }
}

type ActivityRows<'a> = BTreeMap<(Reverse<u64>, String), PendingEvent<'a>>;

/// Source maps use opaque IDs, not sequence order. Only a strictly older
/// sequence cannot displace a full page; ties still compete by event ID.
fn history_sequence_may_fit(events: &ActivityRows<'_>, sequence: u64) -> bool {
    events.len() < HISTORY_LIMIT
        || events
            .last_key_value()
            .is_some_and(|((Reverse(oldest), _), _)| sequence >= *oldest)
}

/// Keep the newest 200 audit rows while scanning, rather than allocating the
/// complete history and discarding all but one page afterward.
fn record_event<'a>(
    events: &mut ActivityRows<'a>,
    sequence: u64,
    id: String,
    event: PendingEvent<'a>,
) {
    events.insert((Reverse(sequence), id), event);
    if events.len() > HISTORY_LIMIT {
        events.pop_last();
    }
}

fn commit_events<'a>(engine: &'a Engine, events: &mut ActivityRows<'a>) -> Result<(), ApiError> {
    for commit in engine.state().commits.values() {
        if !history_sequence_may_fit(events, commit.sequence) {
            continue;
        }
        let repo = repo_for_history(engine, &commit.history_tree_id)?;
        // Deletions are committed namespace facts, independent of content
        // identity and of whether another alias still names the same file.
        // The ordinal preserves repeated removal of one path in a transaction.
        if let Some(deletions) = &commit.namespace_deletions {
            for (ordinal, deleted) in deletions.paths.iter().enumerate() {
                let display = if repo.file_manager_root {
                    deleted.path.clone()
                } else {
                    format!("{}/{}", repo.name, deleted.path)
                };
                record_event(
                    events,
                    commit.sequence,
                    format!("{}:delete:{}", commit.commit_id, ordinal),
                    PendingEvent {
                        timestamp: Timestamp::Seconds(deletions.timestamp),
                        slot_id: repo.primary_slot_id.clone(),
                        path: display,
                        action: "delete".into(),
                        principal: Some(commit.author.clone()),
                        file_id: String::new(),
                        message: commit.message.clone(),
                        detail: PendingDetail::Captured(ActivityDetailSnapshot::Plain {
                            version_id: None,
                        }),
                    },
                );
            }
        }
        if matches!(
            commit.kind.as_str(),
            "root"
                | "file_branch_root"
                | "file_content"
                | "file_metadata"
                | "file_node_publication"
                | "directory_namespace"
                | "namespace_batch"
        ) {
            continue;
        }
        let receipt = engine
            .published_receipt(commit.operation_id)
            .map_err(crate::engine_backend::error)?;
        if commit.kind == "file_selection"
            && receipt
                .as_ref()
                .is_some_and(|op| op.kind == "workspace.commit")
        {
            continue;
        }
        let parent = commit
            .parent_commit_ids
            .first()
            .and_then(|id| engine.state().commits.get(id));
        let parent_tree = commit
            .parent_commit_ids
            .first()
            .and_then(|id| engine.commit_namespace(id));
        // Branch registration is an audit event, but it does not
        // claim the source version as newly written branch content.
        let pending_creation = matches!(
            commit.kind.as_str(),
            "file_branch_pending" | "branch_pending"
        ) || commit.kind == "file_branch"
            && receipt
                .as_ref()
                .is_some_and(|operation| operation.result.get("placeholder_files").is_some());
        let timestamp =
            receipt_timestamp(receipt.as_ref().and_then(|op| op.result.get("timestamp")))?;
        let action = if commit.kind == "namespace" {
            receipt
                .as_ref()
                .and_then(|op| op.result.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("namespace")
                .to_owned()
        } else {
            commit.kind.clone()
        };
        let commit_entries = engine
            .commit_namespace(&commit.commit_id)
            .expect("invariant: iterated commit exists")
            .entries()
            .map_err(crate::engine_backend::error)?;
        for entry in &commit_entries {
            let pending_source = commit
                .pending_file_versions
                .get(&entry.file_id)
                .or_else(|| pending_creation.then_some(&entry.version_id));
            if pending_creation && entry.kind != storage_contract::NamespaceEntryKind::File {
                continue;
            }
            // An unchanged entry is one the parent view resolves identically
            // at the same key: a point read, never a parent-list scan.
            let unchanged = !(pending_creation
                && entry.kind == storage_contract::NamespaceEntryKind::File)
                && match (&parent, &parent_tree) {
                    (Some(p), Some(tree)) => tree
                        .get_key(entry.lookup_key())
                        .map_err(crate::engine_backend::error)?
                        .is_some_and(|old| {
                            old.entry_id == entry.entry_id
                                && old.path == entry.path
                                && old.version_id == entry.version_id
                                && p.pending_file_versions.contains_key(&old.file_id)
                                    == pending_source.is_some()
                        }),
                    _ => false,
                };
            if unchanged {
                continue;
            }
            let display = if repo.file_manager_root {
                entry.path().to_owned()
            } else {
                format!("{}/{}", repo.name, entry.path)
            };
            let detail = if entry.kind == storage_contract::NamespaceEntryKind::File {
                ActivityDetailSnapshot::Content(capture_content(
                    engine.state(),
                    Some(&entry.version_id),
                    pending_source.map(String::as_str),
                ))
            } else {
                ActivityDetailSnapshot::Plain {
                    version_id: Some(entry.version_id.clone()),
                }
            };
            let event = PendingEvent {
                timestamp: Timestamp::Resolved(timestamp),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: action.clone(),
                principal: Some(commit.author.clone()),
                file_id: entry.file_id.clone(),
                message: commit.message.clone(),
                detail: PendingDetail::Captured(detail),
            };
            record_event(
                events,
                commit.sequence,
                format!("{}:{}:{}", commit.commit_id, entry.entry_id, entry.path),
                event,
            );
        }
    }
    Ok(())
}

fn file_operation_events<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
) -> Result<(), ApiError> {
    // Newest-first: once the page is full and a receipt's slot is older than
    // the retained page, every later receipt is older still, so the pull
    // stops without reading further sealed segments.
    for entry in engine.receipts_descending() {
        let entry = entry.map_err(crate::engine_backend::error)?;
        if !history_sequence_may_fit(events, entry.result.sequence) {
            break;
        }
        let operation = &entry.result;
        if !(matches!(
            operation.kind.as_str(),
            "file.metadata"
                | "version.metadata"
                | "file.branch.rename"
                | "file.path.metadata"
                | "file.branches.merge"
                | "file.tags.update"
        ) && operation.result.get("no_change").and_then(Value::as_bool) != Some(true))
        {
            continue;
        }
        let Some(file) = operation.result.get("file_id").and_then(Value::as_str) else {
            continue;
        };
        let repo = match file_repo(engine, file) {
            Ok(repo) => repo,
            Err(error) if error.code == storage_contract::ErrorCode::FileNotFound => continue,
            Err(error) => return Err(error),
        };
        let path_node = operation
            .result
            .get("node_id")
            .and_then(Value::as_str)
            .and_then(|id| engine.state().file_path_nodes.get(id));
        let metadata_node = operation
            .result
            .get("node_id")
            .and_then(Value::as_str)
            .and_then(|id| engine.state().file_metadata_nodes.get(id));
        let current = engine
            .list_tree(&repo.main_branch_id)
            .map_err(crate::engine_backend::error)?
            .into_iter()
            .find(|entry| entry.file_id == file);
        let relative = path_node
            .map(|node| node.path.clone())
            .or_else(|| current.map(|entry| entry.path.into_string()))
            .or_else(|| {
                engine
                    .state()
                    .versions
                    .values()
                    .find(|v| v.file_id == file)
                    .map(|v| v.creation_name.clone())
            })
            .unwrap_or_default();
        let display = if repo.file_manager_root {
            relative
        } else {
            format!("{}/{}", repo.name, relative)
        };
        record_event(
            events,
            operation.sequence,
            operation.operation_id.to_string(),
            PendingEvent {
                timestamp: Timestamp::Resolved(receipt_timestamp(
                    operation.result.get("timestamp"),
                )?),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: operation.kind.clone(),
                principal: Some(entry.principal.clone()),
                file_id: file.into(),
                message: operation
                    .result
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                detail: PendingDetail::Captured(
                    PendingDetail::FileOperation {
                        version_id: path_node
                            .map(|node| node.version_id.as_str())
                            .or_else(|| metadata_node.map(|node| node.version_id.as_str())),
                        receipt: &operation.result,
                    }
                    .parse()?,
                ),
            },
        );
    }
    Ok(())
}

fn directory_operation_events<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
) -> Result<(), ApiError> {
    for entry in engine.receipts_descending() {
        let entry = entry.map_err(crate::engine_backend::error)?;
        if !history_sequence_may_fit(events, entry.result.sequence) {
            break;
        }
        let operation = &entry.result;
        if !(matches!(
            operation.kind.as_str(),
            "directory.branches.create"
                | "directory.branches.select"
                | "directory.default.select"
                | "directory.branch.rename"
                | "directory.scope.initialize"
        ) && operation.result.get("no_change").and_then(Value::as_bool) != Some(true))
        {
            continue;
        }
        let Some(id) = operation
            .result
            .get("directory_branch_id")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(branch) = engine.state().directory_branches.get(id) else {
            continue;
        };
        let Some(repo) = engine.state().repositories.get(&branch.repository_id) else {
            continue;
        };
        let relative = operation
            .result
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(&branch.path);
        let display = if repo.file_manager_root {
            relative.to_owned()
        } else if relative.is_empty() {
            repo.name.clone()
        } else {
            format!("{}/{}", repo.name, relative)
        };
        record_event(
            events,
            operation.sequence,
            operation.operation_id.to_string(),
            PendingEvent {
                timestamp: Timestamp::Resolved(receipt_timestamp(
                    operation.result.get("timestamp"),
                )?),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: operation.kind.clone(),
                principal: Some(entry.principal.clone()),
                file_id: String::new(),
                message: operation
                    .result
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                detail: PendingDetail::Captured(
                    PendingDetail::DirectoryOperation {
                        branch_id: id,
                        head: operation.result.get("head_tree_id"),
                    }
                    .parse()?,
                ),
            },
        );
    }
    Ok(())
}

fn backup_events<'a>(engine: &'a Engine, events: &mut ActivityRows<'a>) -> Result<(), ApiError> {
    for entry in engine.receipts_descending() {
        let entry = entry.map_err(crate::engine_backend::error)?;
        if !history_sequence_may_fit(events, entry.result.sequence) {
            break;
        }
        let operation = &entry.result;
        if operation.kind != "backup.configure" {
            continue;
        }
        let history =
            optional_field::<&str>(operation.result.get("history_tree_id"))?.ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::StateInvalid,
                    "backup configuration receipt lacks history identity",
                )
            })?;
        let repo = match repo_for_history(engine, history) {
            Ok(repo) => repo,
            Err(error) if error.status == ::http::StatusCode::NOT_FOUND => continue,
            Err(error) => return Err(error),
        };
        let path = operation.result["scope"]["path"].as_str().unwrap_or("");
        let display = if repo.file_manager_root {
            path.to_owned()
        } else if path.is_empty() {
            repo.name.clone()
        } else {
            format!("{}/{path}", repo.name)
        };
        record_event(
            events,
            operation.sequence,
            operation.operation_id.to_string(),
            PendingEvent {
                timestamp: Timestamp::Resolved(receipt_timestamp(
                    operation.result.get("timestamp"),
                )?),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: "backup.configure".into(),
                principal: Some(entry.principal.clone()),
                file_id: operation.result["policy"]["scope_file_id"]
                    .as_str()
                    .unwrap_or("")
                    .into(),
                message: String::new(),
                detail: PendingDetail::Captured(
                    PendingDetail::Backup {
                        copies: operation.result.get("target_copies"),
                    }
                    .parse()?,
                ),
            },
        );
    }
    Ok(())
}

fn path_events<'a>(engine: &'a Engine, events: &mut ActivityRows<'a>) -> Result<(), ApiError> {
    for node in engine.state().file_path_nodes.values() {
        if !history_sequence_may_fit(events, node.sequence) {
            continue;
        }
        let repo = repo_for_history(engine, &node.history_tree_id)?;
        let display = if repo.file_manager_root {
            node.path.clone()
        } else {
            format!("{}/{}", repo.name, node.path)
        };
        record_event(
            events,
            node.sequence,
            node.node_id.clone(),
            PendingEvent {
                timestamp: Timestamp::Seconds(node.created_at),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: "file.path.rename".into(),
                principal: Some(node.author.clone()),
                file_id: node.file_id.clone(),
                message: String::new(),
                detail: PendingDetail::Captured(ActivityDetailSnapshot::Path {
                    version_id: node.version_id.clone(),
                    node_id: node.node_id.clone(),
                    previous_path: node.previous_path.clone(),
                    branch_id: node.branch_id.clone(),
                }),
            },
        );
    }
    Ok(())
}

fn tree_events<'a>(engine: &'a Engine, events: &mut ActivityRows<'a>) -> Result<(), ApiError> {
    for tree in engine.state().directory_trees.values() {
        if !history_sequence_may_fit(events, tree.sequence) {
            continue;
        }
        // One bounded lookup per retained tree: the receipt published at this
        // tree's exact decision slot.
        let Some(entry) = engine
            .receipt_by_sequence(tree.sequence)
            .map_err(crate::engine_backend::error)?
        else {
            continue;
        };
        let operation = &entry.result;
        // Registering a directory references the parent's existing
        // subtree. Its initialization is covered by the register event.
        if matches!(
            operation.kind.as_str(),
            "directory.branches.create" | "directory.scope.initialize"
        ) {
            continue;
        }
        let Some(branch) = engine
            .state()
            .directory_branches
            .get(&tree.directory_branch_id)
        else {
            continue;
        };
        let Some(repo) = engine.state().repositories.get(&branch.repository_id) else {
            continue;
        };
        let display = if repo.file_manager_root {
            tree.path.clone()
        } else if tree.path.is_empty() {
            repo.name.clone()
        } else {
            format!("{}/{}", repo.name, tree.path)
        };
        let action = if operation.kind == "directory.namespace.batch" {
            match operation.result.get("action").and_then(Value::as_str) {
                Some("move") => "directory.namespace.move",
                Some("copy") => "directory.namespace.copy",
                Some("delete") => "directory.namespace.delete",
                Some("rename") => "directory.namespace.rename",
                _ => "directory.tree.write",
            }
        } else {
            "directory.tree.write"
        };
        record_event(
            events,
            tree.sequence,
            tree.tree_id.clone(),
            PendingEvent {
                timestamp: Timestamp::Resolved(receipt_timestamp(
                    operation.result.get("timestamp"),
                )?),
                slot_id: repo.primary_slot_id.clone(),
                path: display,
                action: action.into(),
                principal: Some(entry.principal.clone()),
                file_id: String::new(),
                message: String::new(),
                detail: PendingDetail::Captured(ActivityDetailSnapshot::Tree {
                    version_id: None,
                    directory_branch_id: tree.directory_branch_id.clone(),
                    head_tree_id: tree.tree_id.clone(),
                    parent_tree_id: tree.parent_tree_id.clone(),
                }),
            },
        );
    }
    Ok(())
}

pub(crate) fn read(engine: &Engine) -> Result<ActivityHistorySnapshot, ApiError> {
    let mut events = BTreeMap::new();
    commit_events(engine, &mut events)?;
    file_operation_events(engine, &mut events)?;
    directory_operation_events(engine, &mut events)?;
    backup_events(engine, &mut events)?;
    path_events(engine, &mut events)?;
    tree_events(engine, &mut events)?;
    let events = events
        .into_iter()
        .map(|((Reverse(sequence), id), event)| event.finish(sequence, id))
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(ActivityHistorySnapshot { events })
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/activity_tests.rs"]
mod activity_tests;
