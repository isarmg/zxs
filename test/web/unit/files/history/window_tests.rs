use super::*;
use json_codec::json;

fn id(prefix: &str, value: u64) -> String {
    format!("{prefix}_{value:032x}")
}

fn root() -> wire::HistoryRoot {
    wire::HistoryRoot::parse(&"a".repeat(64)).unwrap()
}

fn context() -> wire::HistoryResolvedContext {
    wire::HistoryResolvedContext {
        repository_id: wire::HistoryRepositoryId::parse(&id("repo", 1)).unwrap(),
        history_tree_id: wire::HistoryTreeId::parse(&id("history", 1)).unwrap(),
        path: Some(name_policy::PortablePath::parse("folder/file.txt").unwrap()),
        current_node_id: Some(wire::HistoryNodeId::parse(&id("ver", 1)).unwrap()),
        selected_branch_id: Some(wire::HistoryBranchId::parse(&id("br", 1)).unwrap()),
    }
}

fn read_request(collection: wire::HistoryCollection) -> wire::HistoryWindowRequest {
    let limit = wire::HistoryPageLimit::default_for(&collection);
    wire::HistoryWindowRequest {
        file_id: wire::HistoryFileId::parse(&id("file", 1)).unwrap(),
        collection,
        location: Some(wire::HistoryLocation::new(
            context().repository_id,
            name_policy::LookupPath::parse("folder/file.txt").unwrap(),
        )),
        history_root: Some(root()),
        cursor: None,
        anchor_node_id: None,
        limit,
    }
}

fn page_identity(request: &wire::HistoryWindowRequest) -> wire::HistoryPageIdentity {
    let context = context();
    let anchor = request.anchor_node_id.as_ref().or_else(|| {
        request
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.payload().anchor_node_id.as_ref())
    });
    wire::HistoryPageIdentity {
        history_root: request.history_root.unwrap_or_else(root),
        file_id: request.file_id.clone(),
        collection: request.collection.clone(),
        query_hash: request.query_hash(&context, anchor),
        context,
        limit: request.limit,
        action_max_entries: 100_000,
    }
}

fn cursor(
    request: &wire::HistoryWindowRequest,
    position: wire::HistoryPosition,
    direction: wire::HistoryDirection,
) -> wire::HistoryCursor {
    let identity = page_identity(request);
    wire::HistoryCursor::from_payload(wire::HistoryCursorPayload {
        format: wire::HistoryCursorFormat::HistoryWindowV1,
        history_root: identity.history_root,
        history_tree_id: identity.context.history_tree_id,
        file_id: identity.file_id,
        collection: identity.collection,
        query_hash: identity.query_hash,
        limit: identity.limit,
        direction,
        position,
        anchor_node_id: request.anchor_node_id.clone().or_else(|| {
            request.cursor.as_ref().and_then(|cursor| cursor.payload().anchor_node_id.clone())
        }),
    })
    .unwrap()
}

fn node(number: u64) -> wire::HistoryNodeSummary {
    wire::HistoryNodeSummary {
        node_id: wire::HistoryNodeId::parse(&id("ver", number)).unwrap(),
        version_id: wire::HistoryVersionId::parse(&id("ver", number)).unwrap(),
        kind: wire::HistoryNodeKind::Content,
        sequence: 1_000 - number,
        current: number == 1,
        branch_id: Some(wire::HistoryBranchId::parse(&id("br", 1)).unwrap()),
        path: "folder/file.txt".into(),
        previous_path: None,
        note: String::new(),
        tags: Vec::new(),
        size: 0,
        content_hash: "b".repeat(64).parse().unwrap(),
        content_state: None,
        download_name: "file.txt".into(),
        storage_name: "v000001_fixture.txt".into(),
        parents: wire::HistoryParentSet::Complete,
        fields: wire::HistoryNodeFields {
            message: wire::HistoryFieldInfo { byte_length: 0 },
            author: wire::HistoryFieldInfo { byte_length: 0 },
            created_at: wire::HistoryFieldInfo { byte_length: 0 },
        },
    }
}

fn nodes(request: &wire::HistoryWindowRequest, count: u64) -> wire::HistoryWindowPage {
    wire::HistoryWindowPage::Nodes {
        identity: page_identity(request),
        context: wire::HistoryContextSummary {
            note: String::new(),
            content: Some(wire::HistoryContentDisposition {
                version_id: Some(wire::HistoryVersionId::parse(&id("ver", 1)).unwrap()),
                source: wire::HistoryContentSource::Owned,
                stored_state: None,
            }),
        },
        nodes: (1..=count).map(node).collect(),
        edges: Vec::new(),
        page: wire::HistoryPageCursors {
            previous_cursor: None,
            next_cursor: None,
        },
    }
}

fn value(page: &wire::HistoryWindowPage) -> Value {
    json_codec::to_value(page).unwrap()
}

fn accepted(page: &wire::HistoryWindowPage, request: &wire::HistoryWindowRequest) -> bool {
    admit(value(page), request, None).is_ok()
}

#[test]
fn node_page_is_bounded_by_the_exact_requested_limit() {
    let mut request = read_request(wire::HistoryCollection::Nodes);
    request.limit = wire::HistoryPageLimit::for_collection(2, &request.collection).unwrap();
    assert!(accepted(&nodes(&request, 2), &request));
    assert!(!accepted(&nodes(&request, 3), &request));
    let mut page = value(&nodes(&request, 1));
    page["nodes"][0]["size"] = u64::MAX.to_string().into();
    page["nodes"][0]["fields"]["created_at"]["byte_length"] = u64::MAX.to_string().into();
    page["identity"]["action_max_entries"] = u64::MAX.to_string().into();
    assert!(admit(page, &request, None).is_ok());
}

#[test]
fn page_identity_cannot_change_file_root_collection_limit_query_or_action_capability() {
    let request = read_request(wire::HistoryCollection::Nodes);
    for (field, replacement) in [
        ("file_id", json!(id("file", 2))),
        ("history_root", json!("c".repeat(64))),
        ("collection", json!({"kind":"branches"})),
        ("limit", json!(16)),
        ("query_hash", json!("d".repeat(64))),
        ("action_max_entries", json!("0")),
    ] {
        let mut page = value(&nodes(&request, 1));
        page["identity"][field] = replacement;
        assert!(admit(page, &request, None).is_err(), "accepted changed {field}");
    }
    let mut page = value(&nodes(&request, 1));
    page["kind"] = "node_parents".into();
    page.as_object_mut().unwrap().remove("context");
    page.as_object_mut().unwrap().remove("nodes");
    page.as_object_mut().unwrap().remove("edges");
    page["parents"] = json!([]);
    assert!(admit(page, &request, None).is_err());
}

#[test]
fn exact_requested_and_previously_resolved_contexts_remain_bound() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let page = value(&nodes(&request, 1));
    assert!(admit(page.clone(), &request, Some(&context())).is_ok());
    let mut changed = context();
    changed.current_node_id = Some(wire::HistoryNodeId::parse(&id("ver", 2)).unwrap());
    assert!(admit(page, &request, Some(&changed)).is_err());
    let mut page = value(&nodes(&request, 1));
    page["identity"]["context"]["path"] = "folder/alias.txt".into();
    assert!(admit(page, &request, None).is_err());
}

#[test]
fn nullable_page_and_record_fields_must_be_present() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let removals: [fn(&mut Value); 6] = [
        |page| { page["identity"]["context"].as_object_mut().unwrap().remove("current_node_id"); },
        |page| { page["page"].as_object_mut().unwrap().remove("next_cursor"); },
        |page| { page["context"].as_object_mut().unwrap().remove("content"); },
        |page| { page["context"]["content"].as_object_mut().unwrap().remove("stored_state"); },
        |page| { page["nodes"][0].as_object_mut().unwrap().remove("branch_id"); },
        |page| { page["nodes"][0].as_object_mut().unwrap().remove("previous_path"); },
    ];
    for remove in removals {
        let mut page = value(&nodes(&request, 1));
        remove(&mut page);
        assert!(admit(page, &request, None).is_err());
    }
    assert!(admit(json!({"file_id":id("file",1),"versions":[],"nodes":[],"branches":[]}), &request, None).is_err());
}

#[test]
fn node_identity_order_current_markers_and_small_fields_are_admitted_together() {
    let request = read_request(wire::HistoryCollection::Nodes);
    for change in [
        json!({"current":false}),
        json!({"note":"n".repeat(129)}),
        json!({"path":"../escape.txt"}),
        json!({"tags":["same","same"]}),
        json!({"created_at":"2026-01-01T00:00:00Z"}),
    ] {
        let mut page = value(&nodes(&request, 1));
        page["nodes"][0].as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
        assert!(admit(page, &request, None).is_err());
    }
    let mut page = value(&nodes(&request, 2));
    page["nodes"].as_array_mut().unwrap().swap(0, 1);
    assert!(admit(page, &request, None).is_err());
    let mut page = value(&nodes(&request, 2));
    page["nodes"][1]["node_id"] = page["nodes"][0]["node_id"].clone();
    assert!(admit(page, &request, None).is_err());
}

#[test]
fn current_context_can_be_outside_the_node_window_without_moving_its_marker() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let mut page = nodes(&request, 1);
    if let wire::HistoryWindowPage::Nodes { nodes, .. } = &mut page {
        nodes[0] = node(2);
    }
    assert!(accepted(&page, &request));
    let mut forged = value(&page);
    forged["nodes"][0]["current"] = true.into();
    assert!(admit(forged, &request, None).is_err());
}

#[test]
fn an_initial_anchor_must_be_present_but_continuation_pages_do_not_retain_it() {
    let mut request = read_request(wire::HistoryCollection::Nodes);
    request.anchor_node_id = Some(node(2).node_id);
    assert!(!accepted(&nodes(&request, 1), &request));
    assert!(accepted(&nodes(&request, 2), &request));
    request.cursor = Some(cursor(&request, wire::HistoryPosition::Node {
        sequence: 998, node_id: node(2).node_id,
    }, wire::HistoryDirection::Forward));
    request.anchor_node_id = None;
    let mut page = nodes(&request, 1);
    if let wire::HistoryWindowPage::Nodes { nodes, .. } = &mut page {
        nodes[0] = node(3);
    }
    assert!(accepted(&page, &request));
}

#[test]
fn edge_preview_has_its_own_budget_and_explicit_endpoint_membership() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let mut page = nodes(&request, 1);
    if let wire::HistoryWindowPage::Nodes { edges, .. } = &mut page {
        *edges = (2..258).map(|number| wire::HistoryGraphEdge {
            node_id: node(1).node_id,
            parent_node_id: node(number).node_id,
            endpoint: wire::HistoryEndpoint::OutsideWindow,
        }).collect();
    }
    assert!(accepted(&page, &request));
    let mut oversized = value(&page);
    oversized["edges"].as_array_mut().unwrap().push(json!({
        "node_id":id("ver",1),"parent_node_id":id("ver",258),"endpoint":"outside_window"
    }));
    assert!(admit(oversized, &request, None).is_err());
    let mut forged = value(&page);
    forged["edges"][0]["endpoint"] = "in_window".into();
    assert!(admit(forged, &request, None).is_err());
    let mut forged = value(&page);
    forged["edges"][0]["node_id"] = id("ver", 2).into();
    assert!(admit(forged, &request, None).is_err());
}

#[test]
fn parent_more_cursor_uses_its_own_collection_query_and_inclusive_start() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let mut parent_request = read_request(wire::HistoryCollection::NodeParents { node_id: node(1).node_id });
    parent_request.location = request.location.clone();
    let more = cursor(&parent_request, wire::HistoryPosition::Start, wire::HistoryDirection::Forward);
    let mut page = nodes(&request, 1);
    if let wire::HistoryWindowPage::Nodes { nodes, .. } = &mut page {
        nodes[0].parents = wire::HistoryParentSet::More { cursor: more };
    }
    assert!(accepted(&page, &request));
    let wrong = read_request(wire::HistoryCollection::NodeParents { node_id: node(2).node_id });
    let mut forged = value(&page);
    forged["nodes"][0]["parents"]["cursor"] = json!(cursor(&wrong, wire::HistoryPosition::Start, wire::HistoryDirection::Forward));
    assert!(admit(forged, &request, None).is_err());
}

#[test]
fn page_cursors_bind_query_direction_and_cannot_repeat_a_pending_position() {
    let mut request = read_request(wire::HistoryCollection::Nodes);
    let next = cursor(&request, wire::HistoryPosition::Node { sequence: 999, node_id: node(1).node_id }, wire::HistoryDirection::Forward);
    let mut page = nodes(&request, 1);
    if let wire::HistoryWindowPage::Nodes { page, .. } = &mut page {
        page.next_cursor = Some(next.clone());
    }
    assert!(accepted(&page, &request));
    let mut wrong = value(&page);
    let backward = cursor(&request, next.payload().position.clone(), wire::HistoryDirection::Backward);
    wrong["page"]["next_cursor"] = json!(backward);
    assert!(admit(wrong, &request, None).is_err());
    let mut payload = next.payload().clone();
    payload.history_root = wire::HistoryRoot::parse(&"c".repeat(64)).unwrap();
    let mut wrong = value(&page);
    wrong["page"]["next_cursor"] = json!(wire::HistoryCursor::from_payload(payload).unwrap());
    assert!(admit(wrong, &request, None).is_err());
    let mut empty = value(&nodes(&request, 0));
    empty["page"]["next_cursor"] = json!(next.clone());
    assert!(admit(empty, &request, None).is_err());
    request.cursor = Some(next);
    assert!(!accepted(&page, &request));
}

fn field_page(
    request: &wire::HistoryWindowRequest,
    total_bytes: u64,
    byte_offset: u64,
    text: &str,
    end_of_field: bool,
) -> wire::HistoryWindowPage {
    let wire::HistoryCollection::NodeField { field, .. } = &request.collection else {
        panic!("field fixture requires a field request");
    };
    wire::HistoryWindowPage::NodeField {
        identity: page_identity(request),
        field: *field,
        total_bytes,
        byte_offset,
        text: text.into(),
        end_of_field,
        next_cursor: (!end_of_field).then(|| cursor(request, wire::HistoryPosition::Field {
            byte_offset: byte_offset + text.len() as u64,
        }, wire::HistoryDirection::Forward)),
    }
}

#[test]
fn utf8_fragments_are_replaced_with_exact_ranges_and_explicit_right_boundary() {
    let mut request = read_request(wire::HistoryCollection::NodeField {
        node_id: node(1).node_id, field: wire::HistoryField::Author,
    });
    request.limit = wire::HistoryPageLimit::new(4).unwrap();
    let first = field_page(&request, 6, 0, "界", false);
    assert!(accepted(&first, &request));
    if let wire::HistoryWindowPage::NodeField { next_cursor, .. } = &first {
        request.cursor = next_cursor.clone();
    }
    let last = field_page(&request, 6, 3, "界", true);
    let admitted = admit(value(&last), &request, None).unwrap();
    assert!(matches!(admitted, wire::HistoryWindowPage::NodeField {
        byte_offset: 3, text, end_of_field: true, ..
    } if text == "界"));
    let mut wrong = value(&last);
    wrong["byte_offset"] = "0".into();
    assert!(admit(wrong, &request, None).is_err());
}

#[test]
fn fragment_empty_overflow_budget_and_next_cursor_relations_are_strict() {
    let mut request = read_request(wire::HistoryCollection::NodeField {
        node_id: node(1).node_id, field: wire::HistoryField::Message,
    });
    assert!(accepted(&field_page(&request, 0, 0, "", true), &request));
    let mut wrong = value(&field_page(&request, 3, 0, "界", true));
    wrong["end_of_field"] = false.into();
    assert!(admit(wrong, &request, None).is_err());
    request.limit = wire::HistoryPageLimit::new(2).unwrap();
    assert!(!accepted(&field_page(&request, 3, 0, "界", true), &request));
    request.limit = wire::HistoryPageLimit::new(4).unwrap();
    let mut wrong = value(&field_page(&request, 6, 0, "界", false));
    wrong["next_cursor"] = json!(cursor(&request, wire::HistoryPosition::Field { byte_offset: 4 }, wire::HistoryDirection::Forward));
    assert!(admit(wrong, &request, None).is_err());
    request.cursor = Some(cursor(&request, wire::HistoryPosition::Field { byte_offset: u64::MAX }, wire::HistoryDirection::Forward));
    assert!(!accepted(&field_page(&request, u64::MAX, u64::MAX, "x", true), &request));
}

fn empty_branch() -> wire::HistoryBranchSummary {
    wire::HistoryBranchSummary {
        branch_id: wire::HistoryBranchId::parse(&id("br", 1)).unwrap(),
        name: "pending".into(),
        head_commit_id: wire::HistoryCommitId::parse(&id("commit", 1)).unwrap(),
        fork_commit_id: wire::HistoryCommitId::parse(&id("commit", 1)).unwrap(),
        node_id: None,
        state: storage_contract::BranchState::Active,
        branch_kind: storage_contract::BranchKind::File,
        generation: 1,
        current: true,
        read_only: false,
        alias_of: None,
        content: wire::HistoryContentDisposition {
            version_id: None,
            source: wire::HistoryContentSource::Inherited { source_version_id: None },
            stored_state: None,
        },
    }
}

#[test]
fn an_explicit_empty_source_branch_keeps_its_missing_head_and_write_disposition() {
    let request = read_request(wire::HistoryCollection::Branches);
    let page = wire::HistoryWindowPage::Branches {
        identity: page_identity(&request), branches: vec![empty_branch()],
        page: wire::HistoryPageCursors { previous_cursor: None, next_cursor: None },
    };
    assert!(accepted(&page, &request));
    let mut missing = value(&page);
    missing["branches"][0].as_object_mut().unwrap().remove("node_id");
    assert!(admit(missing, &request, None).is_err());
    let mut wrong = value(&page);
    wrong["branches"][0]["alias_of"] = id("br", 2).into();
    assert!(admit(wrong, &request, None).is_err());
    let mut alias = value(&page);
    alias["branches"][0]["alias_of"] = id("br", 2).into();
    alias["branches"][0]["read_only"] = true.into();
    assert!(admit(alias, &request, None).is_ok());
}

#[test]
fn relation_members_are_bounded_contiguous_and_keep_input_pairs_together() {
    let request = read_request(wire::HistoryCollection::MergeInputs {
        merge_id: wire::HistoryMergeId::parse(&id("fmerge", 1)).unwrap(),
    });
    let page = wire::HistoryWindowPage::MergeInputs {
        identity: page_identity(&request),
        inputs: vec![wire::HistoryMergeInputMember {
            ordinal: u64::MAX,
            branch_id: wire::HistoryBranchId::parse(&id("br", 1)).unwrap(),
            node_id: node(1).node_id,
        }],
        page: wire::HistoryPageCursors { previous_cursor: None, next_cursor: None },
    };
    assert!(accepted(&page, &request));
    let mut wrong = value(&page);
    wrong["inputs"].as_array_mut().unwrap().push(json!({
        "ordinal":"0","branch_id":id("br",2),"node_id":id("ver",2)
    }));
    assert!(admit(wrong, &request, None).is_err());
    let mut wrong = value(&page);
    wrong["inputs"][0]["node_id"] = id("file", 1).into();
    assert!(admit(wrong, &request, None).is_err());
}

#[test]
fn body_versions_preserve_ascending_order_and_reject_node_ids_as_versions() {
    let request = read_request(wire::HistoryCollection::Versions);
    let versions = [1, 2].map(|number| {
        let node = node(number);
        wire::HistoryVersionSummary {
            version_id: node.version_id,
            version_sequence: number,
            size: node.size,
            content_hash: node.content_hash,
            content_state: node.content_state,
            note: node.note,
            download_name: node.download_name,
            storage_name: node.storage_name,
            source_count: 0,
            fields: node.fields,
        }
    });
    let page = wire::HistoryWindowPage::Versions {
        identity: page_identity(&request), versions: versions.into(),
        page: wire::HistoryPageCursors { previous_cursor: None, next_cursor: None },
    };
    assert!(accepted(&page, &request));
    let mut wrong = value(&page);
    wrong["versions"].as_array_mut().unwrap().swap(0, 1);
    assert!(admit(wrong, &request, None).is_err());
    let mut wrong = value(&page);
    wrong["versions"][0]["version_id"] = id("fnode", 1).into();
    assert!(admit(wrong, &request, None).is_err());
}

#[test]
fn merge_summaries_keep_descending_sequence_and_unique_identities() {
    let request = read_request(wire::HistoryCollection::Merges);
    let merge = |number: u64| json!({
        "merge_id":id("fmerge",number),"sequence":(100-number).to_string(),
        "chosen_node_id":id("ver",1),"result_node_id":id("ver",1),
        "surviving_branch_id":id("br",1),"reused_node":true,
        "operation_id":id("op",number),"input_count":"2","parent_count":"0"
    });
    let page = json!({"kind":"merges","identity":page_identity(&request),
        "merges":[merge(1),merge(2)],"page":{"previous_cursor":null,"next_cursor":null}});
    assert!(admit(page.clone(), &request, None).is_ok());
    let mut wrong = page.clone();
    wrong["merges"].as_array_mut().unwrap().swap(0,1);
    assert!(admit(wrong, &request, None).is_err());
    let mut wrong = page;
    wrong["merges"][1]["merge_id"] = id("fmerge",1).into();
    assert!(admit(wrong, &request, None).is_err());
}

#[test]
fn invalid_paths_and_notes_retain_their_boundary_error_sources() {
    let request = read_request(wire::HistoryCollection::Nodes);
    let mut page = value(&nodes(&request, 1));
    page["nodes"][0]["path"] = "../invalid".into();
    assert!(matches!(admit(page, &request, None), Err(DecodeError::Path(_))));
    let mut page = value(&nodes(&request, 1));
    page["nodes"][0]["note"] = "x".repeat(129).into();
    assert!(matches!(admit(page, &request, None), Err(DecodeError::Name(_))));
}
