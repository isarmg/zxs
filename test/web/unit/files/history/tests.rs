//! UI-only fixtures. Strict current wire admission is covered by window_tests.rs.
use super::*;
use crate::snapshot::Snapshot;
use json_codec::json;
use storage_contract::history_window::{
    HistoryCollection, HistoryContentDisposition, HistoryFieldInfo, HistoryNodeFields,
    HistoryPageLimit, HistoryQueryHash, HistoryRepositoryId, HistoryRoot, HistoryTreeId,
    HistoryVersionId,
};

fn text(value: &Value, field: &str) -> Option<String> {
    value[field].as_str().map(str::to_owned)
}

fn number(value: &Value, field: &str, default: u64) -> u64 {
    match value.get(field) {
        None => default,
        Some(Value::String(value)) => value.parse().unwrap(),
        Some(value) => value.as_u64().unwrap(),
    }
}

pub(crate) fn branch_wire(overrides: Value) -> Value {
    let mut value = json!({
        "branch_id":"branch", "name":"main", "state":"active", "alias_of":null,
        "materialized":true, "inherited":false, "version_id":"version",
        "own_version_id":"version", "source_version_id":null, "content_state":"LIVE",
        "node_id":"version", "current":false
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    value
}

pub(crate) fn branch(overrides: Value) -> FileBranch {
    let value = branch_wire(overrides);
    let version = text(&value, "version_id").filter(|value| !value.is_empty());
    let head = text(&value, "node_id")
        .filter(|value| !value.is_empty())
        .or_else(|| text(&value, "file_node_id").filter(|value| !value.is_empty()))
        .or_else(|| version.clone());
    let pending = value["pending"].as_bool().unwrap_or_else(|| {
        version.is_none()
            || value["inherited"] == true
            || value["materialized"] == false
            || value["content_state"] == "PENDING"
    });
    let writable = value["read_only"]
        .as_bool()
        .map(|read_only| !read_only)
        .unwrap_or_else(|| value["state"] == "active" && value["alias_of"].is_null());
    FileBranch {
        id: text(&value, "branch_id").unwrap(),
        name: text(&value, "name").unwrap(),
        head,
        pending,
        writable,
    }
}

pub(crate) fn node(value: Value) -> FileNode {
    let id = text(&value, "node_id")
        .or_else(|| text(&value, "version_id"))
        .unwrap_or_else(|| "fixture-node".into());
    let version = match value.get("version_id") {
        None => Some(id.clone()),
        Some(Value::Null) => None,
        Some(value) => Some(value.as_str().unwrap().to_owned()),
    };
    let kind = match value["kind"].as_str().unwrap_or("content") {
        "content" => NodeKind::Content,
        "rename" => NodeKind::Rename,
        "tags" => NodeKind::Tags,
        "merge" => NodeKind::Merge,
        kind => panic!("unknown UI fixture node kind: {kind}"),
    };
    let note = text(&value, "note").unwrap_or_default();
    let body_note = match value.get("body_note") {
        Some(Value::Null) => None,
        Some(value) => Some(value.as_str().unwrap().to_owned()),
        None => (kind == NodeKind::Content && version.is_some()).then(|| note.clone()),
    };
    let parents = if let Some(edges) = value["parent_edges"].as_array() {
        edges
            .iter()
            .map(|edge| Parent {
                id: text(edge, "node_id").unwrap(),
                endpoint: match edge["endpoint"].as_str().unwrap() {
                    "in_window" => ParentEndpoint::InWindow,
                    "outside_window" => ParentEndpoint::OutsideWindow,
                    endpoint => panic!("unknown UI fixture endpoint: {endpoint}"),
                },
            })
            .collect()
    } else {
        value["parent_node_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|id| Parent {
                id: id.as_str().unwrap().to_owned(),
                endpoint: ParentEndpoint::InWindow,
            })
            .collect()
    };
    let parent_set = match value["parent_set"]["state"].as_str().unwrap_or("complete") {
        "complete" => ParentSet::Complete,
        "more" => ParentSet::More {
            cursor: text(&value["parent_set"], "cursor").unwrap(),
        },
        state => panic!("unknown UI fixture parent set: {state}"),
    };
    let tags = value["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tag| tag.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(tags.len() <= storage_contract::MAX_FILE_TAGS);
    assert!(
        tags.iter()
            .all(|tag| tag.len() <= storage_contract::MAX_TAG_BYTES)
    );
    let field = |name| HistoryFieldInfo {
        byte_length: number(&value["fields"][name], "byte_length", 0),
    };
    FileNode {
        id,
        version,
        sequence: number(&value, "sequence", 0),
        kind,
        note,
        body_note,
        tags,
        path: text(&value, "path"),
        previous_path: text(&value, "previous_path"),
        parents,
        parent_set,
        content_state: match value["content_state"].as_str() {
            Some("AVAILABLE") => Some(storage_contract::ContentState::Available),
            Some("PURGED") => Some(storage_contract::ContentState::Purged),
            Some("MIGRATED") => Some(storage_contract::ContentState::Migrated),
            _ => None,
        },
        fields: HistoryNodeFields {
            message: field("message"),
            author: field("author"),
            created_at: field("created_at"),
        },
    }
}

fn identity(max_entries: u64) -> HistoryPageIdentity {
    HistoryPageIdentity {
        history_root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
        file_id: format!("file_{:032x}", 1).parse().unwrap(),
        collection: HistoryCollection::Nodes,
        context: HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&format!("repo_{:032x}", 1)).unwrap(),
            history_tree_id: HistoryTreeId::parse(&format!("history_{:032x}", 1)).unwrap(),
            path: None,
            current_node_id: None,
            selected_branch_id: None,
        },
        query_hash: HistoryQueryHash::parse(&"b".repeat(64)).unwrap(),
        limit: HistoryPageLimit::default_for(&HistoryCollection::Nodes),
        action_max_entries: max_entries,
    }
}

/// Construct local control state without a wire decoder or legacy response fallback.
pub(crate) fn history(value: Value) -> Snapshot<FileHistory> {
    if value.is_null() {
        return Default::default();
    }
    let nodes = value["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(position, value)| {
            let mut value = value.clone();
            if value.get("node_id").is_none() && value.get("version_id").is_none() {
                value["node_id"] = format!("fixture-node-{position}").into();
            }
            node(value)
        })
        .collect::<Vec<_>>();
    let branches = value["branches"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .map(branch)
        .collect::<Vec<_>>();
    let selected = text(&value, "selected_branch_id");
    let current_id = text(&value, "current_node_id")
        .or_else(|| text(&value, "node_id"))
        .or_else(|| {
            value["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|node| node["current"] == true)
                .and_then(|node| text(node, "node_id"))
        });
    let current_version = match value.get("version_id") {
        Some(Value::Null) => None,
        Some(value) => Some(value.as_str().unwrap().to_owned()),
        None => nodes
            .iter()
            .find(|node| Some(node.id()) == current_id.as_deref())
            .and_then(FileNode::version_id)
            .map(str::to_owned),
    };
    let current_inherited =
        value["inherited"] == true || current_version.is_some() && value["materialized"] == false;
    let current_node = nodes
        .iter()
        .find(|node| Some(node.id()) == current_id.as_deref())
        .cloned()
        .or_else(|| {
            value["current_node"]
                .as_object()
                .map(|_| node(value["current_node"].clone()))
        })
        .or_else(|| {
            current_id.as_ref().map(|id| {
                node(json!({
                    "node_id":id, "version_id":current_version, "inherited":current_inherited,
                    "note":value["current_note"].as_str().unwrap_or("")
                }))
            })
        });
    let current_branch = branches
        .iter()
        .find(|branch| Some(branch.id()) == selected.as_deref())
        .cloned()
        .or_else(|| {
            value["current_branch"]
                .as_object()
                .map(|_| branch(value["current_branch"].clone()))
        });
    let content = current_version.as_ref().map(|_| HistoryContentDisposition {
        version_id: Some(HistoryVersionId::parse(&format!("ver_{:032x}", 1)).unwrap()),
        source: if current_inherited {
            HistoryContentSource::Inherited {
                source_version_id: None,
            }
        } else {
            HistoryContentSource::Owned
        },
        stored_state: None,
    });
    let loaded = value["loaded"] != false;
    FileHistory {
        identity: loaded.then(|| identity(number(&value, "action_max_entries", 100_000))),
        root: loaded.then(|| text(&value, "history_root").unwrap_or_else(|| "root-one".into())),
        selected,
        context: loaded.then(|| HistoryContextSummary {
            note: text(&value, "note").unwrap_or_default(),
            content,
        }),
        current_id,
        current_version,
        current_inherited,
        current_node,
        nodes: nodes.into(),
        branches: branches.into(),
        current_branch,
        page: loaded.then_some(HistoryPageCursors {
            previous_cursor: None,
            next_cursor: None,
        }),
        ready: loaded && value["ready"] != false,
        window_key: number(&value, "window_key", 1),
        ..Default::default()
    }
    .into()
}

fn node_value(node: &FileNode) -> Value {
    let mut value = json!({
        "node_id":node.id(), "version_id":node.version_id(), "sequence":node.sequence().to_string(),
        "kind":match node.kind() {
            NodeKind::Content => "content", NodeKind::Rename => "rename",
            NodeKind::Tags => "tags", NodeKind::Merge => "merge",
        },
        "note":node.note(), "tags":node.tags(), "path":node.path(), "previous_path":node.previous_path(),
        "content_state":node.content_state(),
        "parent_edges":node.parents().iter().map(|parent| json!({
            "node_id":parent.id(), "endpoint":match parent.endpoint() {
                ParentEndpoint::InWindow => "in_window", ParentEndpoint::OutsideWindow => "outside_window",
            }
        })).collect::<Vec<_>>(),
        "parent_set":match node.parent_set() {
            ParentSet::Complete => json!({"state":"complete"}),
            ParentSet::More { cursor } => json!({"state":"more", "cursor":cursor}),
        },
        "fields":{
            "message":{"byte_length":node.field_bytes(HistoryField::Message).to_string()},
            "author":{"byte_length":node.field_bytes(HistoryField::Author).to_string()},
            "created_at":{"byte_length":node.field_bytes(HistoryField::CreatedAt).to_string()}
        }
    });
    if node.body_note()
        != (node.kind() == NodeKind::Content && node.visible_content()).then(|| node.note())
    {
        value["body_note"] = json!(node.body_note());
    }
    value
}

fn branch_value(branch: &FileBranch) -> Value {
    json!({
        "branch_id":branch.id(), "name":branch.name(), "node_id":branch.head(),
        "version_id":branch.head(), "pending":branch.pending(), "read_only":!branch.writable()
    })
}

pub(crate) fn update_metadata(
    snapshot: &mut Snapshot<FileHistory>,
    update: impl FnOnce(&mut Value),
) {
    let mut value = json!({
        "loaded":snapshot.loaded(), "ready":snapshot.ready(), "history_root":snapshot.root(),
        "window_key":snapshot.window_key(), "selected_branch_id":snapshot.selected(),
        "note":snapshot.file_note(), "current_node_id":snapshot.current_node_id(),
        "version_id":snapshot.current_version.as_deref(), "inherited":snapshot.current_inherited(),
        "materialized":!snapshot.current_inherited(),
        "current_node":snapshot.current_node.as_ref().map(node_value),
        "current_branch":snapshot.current_branch.as_ref().map(branch_value),
        "nodes":snapshot.nodes().iter().map(node_value).collect::<Vec<_>>(),
        "branches":snapshot.branches().iter().map(branch_value).collect::<Vec<_>>(),
        "action_max_entries":snapshot.action_max_entries()
    });
    update(&mut value);
    *snapshot = history(value);
}

#[test]
fn unloaded_and_loaded_empty_ui_histories_have_distinct_state() {
    let unloaded = history(Value::Null);
    assert!(!unloaded.loaded() && !unloaded.ready());
    let empty = history(json!({}));
    assert!(empty.loaded() && empty.ready());
    assert_eq!(empty.root(), Some("root-one"));
    assert!(empty.nodes().is_empty());
    assert_eq!(empty.action_max_entries(), Some(100_000));
}

#[test]
fn current_summary_stays_independent_from_a_replaced_node_window() {
    let history = history(json!({
        "current_node_id":"stored", "version_id":"stored-body", "inherited":true,
        "nodes":[{"node_id":"newer","version_id":"newer-body"}]
    }));
    assert_eq!(history.current_node_id(), Some("stored"));
    assert!(history.current_inherited());
    assert_eq!(
        history.node("stored").unwrap().version_id(),
        Some("stored-body")
    );
    assert_eq!(history.nodes().len(), 1);
    assert_eq!(history.nodes()[0].id(), "newer");
}

#[test]
fn typed_body_notes_are_separate_from_path_and_metadata_notes() {
    let content = node(json!({"node_id":"body", "kind":"content", "note":"body note"}));
    let rename =
        node(json!({"node_id":"rename", "version_id":"body", "kind":"rename", "note":"path note"}));
    assert_eq!(content.body_note(), Some("body note"));
    assert_eq!(rename.note(), "path note");
    assert_eq!(rename.body_note(), None);
}

#[test]
fn updating_a_ui_fixture_rebuilds_typed_records_and_preserves_parent_boundaries() {
    let mut history = history(json!({
        "current_node_id":"node",
        "nodes":[{"node_id":"node","note":"before",
            "parent_edges":[{"node_id":"outside","endpoint":"outside_window"}],
            "parent_set":{"state":"more","cursor":"continue"}}]
    }));
    update_metadata(&mut history, |value| {
        value["nodes"][0]["note"] = "after".into()
    });
    let node = history.node("node").unwrap();
    assert_eq!(node.note(), "after");
    assert_eq!(node.body_note(), Some("after"));
    assert_eq!(node.parents()[0].endpoint(), ParentEndpoint::OutsideWindow);
    assert!(matches!(node.parent_set(), ParentSet::More { cursor } if cursor == "continue"));
}

fn admitted_node_page(note: &str) -> HistoryWindowPage {
    use storage_contract::history_window::{HistoryNodeKind, HistoryNodeSummary, HistoryParentSet};
    let mut identity = identity(100_000);
    let id = format!("ver_{:032x}", 2);
    identity.context.current_node_id = Some(id.parse().unwrap());
    identity.context.path = Some(name_policy::PortablePath::parse("folder/file.txt").unwrap());
    HistoryWindowPage::Nodes {
        identity,
        context: HistoryContextSummary {
            note: "file note".into(),
            content: Some(HistoryContentDisposition {
                version_id: Some(id.parse().unwrap()),
                source: HistoryContentSource::Owned,
                stored_state: None,
            }),
        },
        nodes: vec![HistoryNodeSummary {
            node_id: id.parse().unwrap(),
            version_id: id.parse().unwrap(),
            kind: HistoryNodeKind::Content,
            sequence: 2,
            current: true,
            branch_id: None,
            path: "folder/file.txt".into(),
            previous_path: None,
            note: note.into(),
            tags: vec![],
            size: 0,
            content_hash: "c".repeat(64).parse().unwrap(),
            content_state: None,
            download_name: "file.txt".into(),
            storage_name: "v000002_fixture.txt".into(),
            parents: HistoryParentSet::Complete,
            fields: HistoryNodeFields {
                message: HistoryFieldInfo { byte_length: 0 },
                author: HistoryFieldInfo { byte_length: 0 },
                created_at: HistoryFieldInfo { byte_length: 0 },
            },
        }],
        edges: vec![],
        page: HistoryPageCursors {
            previous_cursor: None,
            next_cursor: None,
        },
    }
}

#[test]
fn installing_a_newer_window_does_not_move_the_backup_current_identity() {
    let stored = format!("ver_{:032x}", 1);
    let newer = format!("ver_{:032x}", 2);
    let entry = crate::files::test_entries::entry(json!({
        "backup":true, "node_id":stored, "version_id":stored,
        "file_id":format!("file_{:032x}", 1), "repository_id":format!("repo_{:032x}", 1),
        "path":"backup/file.txt", "repository_path":"folder/file.txt"
    }));
    let mut history = FileHistory::default();
    history
        .install_nodes(admitted_node_page("newer body"), &entry, true)
        .unwrap();
    assert_eq!(history.current_node_id(), Some(stored.as_str()));
    assert_eq!(history.current_version.as_deref(), Some(stored.as_str()));
    assert_eq!(history.nodes()[0].id(), newer);
    assert!(history.nodes().iter().all(|node| node.id() != stored));
    assert!(entry.read_only());
}

#[test]
fn metadata_refresh_preserves_the_window_key_but_explicit_replacement_advances_it() {
    let entry = crate::files::test_entries::entry(json!({
        "file_id":format!("file_{:032x}", 1), "repository_id":format!("repo_{:032x}", 1),
        "path":"folder/file.txt", "repository_path":"folder/file.txt",
        "node_id":format!("ver_{:032x}", 2), "version_id":format!("ver_{:032x}", 2)
    }));
    let mut history = FileHistory::default();
    history
        .install_nodes(admitted_node_page("before"), &entry, true)
        .unwrap();
    let first_key = history.window_key();
    history
        .install_nodes(admitted_node_page("after"), &entry, false)
        .unwrap();
    assert_eq!(history.window_key(), first_key);
    assert_eq!(history.nodes()[0].note(), "after");
    history
        .install_nodes(admitted_node_page("replacement"), &entry, true)
        .unwrap();
    assert_eq!(history.window_key(), first_key + 1);
    assert_eq!(history.nodes().len(), 1);
}
