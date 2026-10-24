//! Read-only, bounded activity projection across committed history sources.
use super::{file_repo, utc_timestamp};
use crate::{
    ApiError,
    engine_backend::{repo_for_history, wire::content_wire},
};
use json_codec::{Value, json};
use std::{cmp::Reverse, collections::BTreeMap};
use storage_engine::Engine;

const HISTORY_LIMIT: usize = 200;

fn utc_event_timestamp(event: &mut Value) -> Result<(), ApiError> {
    if !event["timestamp"].is_null() {
        let seconds = event["timestamp"]
            .as_str()
            .and_then(|value| storage_contract::parse_u64_decimal(value).ok())
            .ok_or_else(|| ApiError::new(500, "STATE_INVALID", "history timestamp is invalid"))?;
        event["timestamp"] = utc_timestamp(seconds)?.into();
    }
    Ok(())
}

/// Source maps use opaque IDs, not sequence order. Only a strictly older
/// sequence cannot displace a full page; ties still compete by event ID.
fn history_sequence_may_fit(
    events: &BTreeMap<(Reverse<u64>, String), Value>,
    sequence: u64,
) -> bool {
    events.len() < HISTORY_LIMIT
        || events
            .last_key_value()
            .is_some_and(|((Reverse(oldest), _), _)| sequence >= *oldest)
}

/// Keep the newest 200 audit rows while scanning, rather than allocating the
/// complete history and discarding all but one page afterward.
fn record_event(events: &mut BTreeMap<(Reverse<u64>, String), Value>, event: Value) {
    let sequence = event["sequence"]
        .as_str()
        .unwrap_or("0")
        .parse::<u64>()
        .unwrap_or(0);
    let id = event["event_id"].as_str().unwrap_or("").to_string();
    events.insert((Reverse(sequence), id), event);
    if events.len() > HISTORY_LIMIT {
        events.pop_last();
    }
}

pub(super) fn read(engine: &Engine) -> Result<Value, ApiError> {
    let mut events = BTreeMap::new();
    for commit in engine.state().commits.values() {
        if !history_sequence_may_fit(&events, commit.sequence) {
            continue;
        }
        let Ok(repo) = repo_for_history(engine, &commit.history_tree_id) else {
            continue;
        };
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
        if commit.kind == "file_selection"
            && engine
                .state()
                .operations
                .get(&commit.operation_id)
                .is_some_and(|op| op.kind == "workspace.commit")
        {
            continue;
        }
        let parent = commit
            .parent_commit_ids
            .first()
            .and_then(|id| engine.state().commits.get(id));
        // Branch registration is an audit event, but it does not
        // claim the source version as newly written branch content.
        let pending_creation = matches!(
            commit.kind.as_str(),
            "file_branch_pending" | "branch_pending"
        ) || commit.kind == "file_branch"
            && engine
                .state()
                .operations
                .get(&commit.operation_id)
                .is_some_and(|operation| operation.result.get("placeholder_files").is_some());
        let timestamp = engine
            .state()
            .operations
            .get(&commit.operation_id)
            .and_then(|op| op.result.get("timestamp"))
            .cloned()
            .unwrap_or(Value::Null);
        let action = if commit.kind == "namespace" {
            engine
                .state()
                .operations
                .get(&commit.operation_id)
                .and_then(|op| op.result.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("namespace")
        } else {
            commit.kind.as_str()
        };
        for entry in &commit.entries {
            let pending_source = commit
                .pending_file_versions
                .get(&entry.file_id)
                .or_else(|| pending_creation.then_some(&entry.version_id));
            if pending_creation && entry.kind != "file" {
                continue;
            }
            if !(pending_creation && entry.kind == "file")
                && parent.is_some_and(|p| {
                    p.entries.iter().any(|old| {
                        old.entry_id == entry.entry_id
                            && old.path == entry.path
                            && old.version_id == entry.version_id
                            && p.pending_file_versions.contains_key(&old.file_id)
                                == pending_source.is_some()
                    })
                })
            {
                continue;
            }
            let display = if repo.file_manager_root {
                entry.path.clone()
            } else {
                format!("{}/{}", repo.name, entry.path)
            };
            let mut event = json!({"event_id":format!("{}:{}:{}",commit.commit_id,entry.entry_id,entry.path),"timestamp":timestamp,"sequence":commit.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":display,"action":action,"principal":commit.author,"version_id":entry.version_id,"file_id":entry.file_id,"message":commit.message});
            if entry.kind == "file" {
                event.as_object_mut().unwrap().extend(
                    content_wire(
                        engine,
                        Some(&entry.version_id),
                        pending_source.map(String::as_str),
                    )
                    .as_object()
                    .unwrap()
                    .clone(),
                );
            }
            record_event(&mut events, event);
        }
        if !matches!(
            commit.kind.as_str(),
            "file_branch"
                | "file_branch_pending"
                | "branch_pending"
                | "file_content"
                | "historical_save"
        ) && let Some(parent) = parent
        {
            for entry in &parent.entries {
                if !commit.entries.iter().any(|e| e.entry_id == entry.entry_id) {
                    record_event(
                        &mut events,
                        json!({"event_id":format!("{}:{}:{}:deleted",commit.commit_id,entry.entry_id,entry.path),"timestamp":timestamp,"sequence":commit.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":if repo.file_manager_root{entry.path.clone()}else{format!("{}/{}",repo.name,entry.path)},"action":"delete","principal":commit.author,"version_id":entry.version_id,"file_id":entry.file_id,"message":commit.message}),
                    );
                }
            }
        }
    }
    for operation in engine.state().operations.values().filter(|op| {
        matches!(
            op.kind.as_str(),
            "file.metadata"
                | "version.metadata"
                | "file.branch.rename"
                | "file.path.metadata"
                | "file.branches.merge"
                | "file.tags.update"
        ) && op.result.get("no_change").and_then(Value::as_bool) != Some(true)
    }) {
        if !history_sequence_may_fit(&events, operation.sequence) {
            continue;
        }
        let Some(file) = operation.result.get("file_id").and_then(Value::as_str) else {
            continue;
        };
        let Ok(repo) = file_repo(engine, file) else {
            continue;
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
            .ok()
            .and_then(|entries| entries.into_iter().find(|entry| entry.file_id == file));
        let relative = path_node
            .map(|node| node.path.clone())
            .or_else(|| current.map(|entry| entry.path))
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
            &mut events,
            json!({"event_id":operation.operation_id,"timestamp":operation.result.get("timestamp"),"sequence":operation.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":display,"action":operation.kind,"principal":engine.operation_owner(&operation.operation_id),"file_id":file,"version_id":path_node.map(|node|json!(node.version_id)).or_else(||metadata_node.map(|node|json!(node.version_id))).or_else(||operation.result.get("version_id").cloned()),"node_id":operation.result.get("node_id"),"branch_id":operation.result.get("branch_id"),"message":operation.result.get("name").and_then(Value::as_str).unwrap_or("")}),
        );
    }
    for operation in engine.state().operations.values().filter(|op| {
        matches!(
            op.kind.as_str(),
            "directory.branches.create"
                | "directory.branches.select"
                | "directory.default.select"
                | "directory.branch.rename"
                | "directory.scope.initialize"
        ) && op.result.get("no_change").and_then(Value::as_bool) != Some(true)
    }) {
        if !history_sequence_may_fit(&events, operation.sequence) {
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
            &mut events,
            json!({
                "event_id":operation.operation_id,"timestamp":operation.result.get("timestamp"),
                "sequence":operation.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":display,
                "action":operation.kind,"principal":engine.operation_owner(&operation.operation_id),
                "file_id":"","version_id":null,"directory_branch_id":id,
                "head_tree_id":operation.result.get("head_tree_id"),
                "message":operation.result.get("name").and_then(Value::as_str).unwrap_or("")
            }),
        );
    }
    for operation in engine
        .state()
        .operations
        .values()
        .filter(|op| op.kind == "backup.configure")
    {
        if !history_sequence_may_fit(&events, operation.sequence) {
            continue;
        }
        let Some(repo) = operation
            .result
            .get("history_tree_id")
            .and_then(Value::as_str)
            .and_then(|history| repo_for_history(engine, history).ok())
        else {
            continue;
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
            &mut events,
            json!({"event_id":operation.operation_id,"timestamp":operation.result.get("timestamp"),
            "sequence":operation.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":display,"action":"backup.configure",
            "principal":engine.operation_owner(&operation.operation_id),"file_id":operation.result["policy"]["scope_file_id"].as_str().unwrap_or(""),"version_id":null,
            "target_copies":operation.result.get("target_copies"),"message":""}),
        );
    }
    for node in engine.state().file_path_nodes.values() {
        if !history_sequence_may_fit(&events, node.sequence) {
            continue;
        }
        let Ok(repo) = repo_for_history(engine, &node.history_tree_id) else {
            continue;
        };
        let display = if repo.file_manager_root {
            node.path.clone()
        } else {
            format!("{}/{}", repo.name, node.path)
        };
        record_event(
            &mut events,
            json!({"event_id":node.node_id,"node_id":node.node_id,
            "timestamp":node.created_at.to_string(),"sequence":node.sequence.to_string(),"slot_id":repo.primary_slot_id,
            "path":display,"previous_path":node.previous_path,"action":"file.path.rename","principal":node.author,
            "file_id":node.file_id,"version_id":node.version_id,"branch_id":node.branch_id,"message":""}),
        );
    }
    // The full page's oldest sequence can only rise, so older
    // operation metadata cannot be needed by a retained tree below.
    let operations_by_sequence = engine
        .state()
        .operations
        .values()
        .filter(|operation| history_sequence_may_fit(&events, operation.sequence))
        .map(|operation| (operation.sequence, operation))
        .collect::<BTreeMap<_, _>>();
    for tree in engine.state().directory_trees.values() {
        if !history_sequence_may_fit(&events, tree.sequence) {
            continue;
        }
        let Some(operation) = operations_by_sequence.get(&tree.sequence) else {
            continue;
        };
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
            &mut events,
            json!({
                "event_id":tree.tree_id,"timestamp":operation.result.get("timestamp"),
                "sequence":tree.sequence.to_string(),"slot_id":repo.primary_slot_id,"path":display,
                "action":action,"principal":engine.operation_owner(&operation.operation_id),
                "file_id":"","version_id":null,"directory_branch_id":tree.directory_branch_id,
                "head_tree_id":tree.tree_id,"parent_tree_id":tree.parent_tree_id,"message":""
            }),
        );
    }
    let events = events
        .into_values()
        .map(|mut event| {
            utc_event_timestamp(&mut event)?;
            Ok(event)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(json!({"events":events,"limit":HISTORY_LIMIT}))
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/activity_tests.rs"]
mod activity_tests;
