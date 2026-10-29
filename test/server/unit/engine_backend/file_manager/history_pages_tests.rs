//! Engine-backed page reads: root binding, cursor admission, anchors and the
//! parent preview budget over a real graph.
use super::*;
use crate::backend_file_history::{PageBudget, ReadIndexedSlice};
use storage_engine::{CommitRequest, Config, FileMergeInput};

struct Graph {
    _root: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    file: String,
    base: String,
    merge_node: String,
    forks: Vec<String>,
}

fn save(
    engine: &mut Engine,
    branch: &str,
    path: &str,
    bytes: &[u8],
    message: &str,
) -> storage_contract::Entry {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: message.into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap()
}

/// A base body, forty forked saves and one merge re-adopting the base body,
/// so the merge node has forty-one graph parents (above the per-node preview).
fn graph() -> Graph {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Pages").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let base = save(
        &mut engine,
        &repo.main_branch_id,
        "doc.txt",
        b"base",
        "base",
    );
    let file = base.file_id.clone();
    let mut forks = Vec::new();
    for index in 0..40 {
        let branch = engine
            .create_file_branch_from_node(
                &file,
                &base.version_id,
                &base.version_id,
                &storage_contract::BranchName::parse(&format!("fork{index:02}")).unwrap(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        engine
            .select_file_branch_at(
                &repo.repository_id,
                &name_policy::PortablePath::parse("doc.txt").unwrap(),
                &file,
                &branch,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        save(
            &mut engine,
            &repo.main_branch_id,
            "doc.txt",
            format!("body {index}").as_bytes(),
            "fork save",
        );
        forks.push(branch);
    }
    engine
        .select_file_branch_at(
            &repo.repository_id,
            &name_policy::PortablePath::parse("doc.txt").unwrap(),
            &file,
            &forks[0],
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    // Choosing the base body (no input's head) creates a real merge node
    // whose parents are the 40 input heads plus the chosen body.
    let chosen = base.version_id.clone();
    let inputs = forks
        .iter()
        .map(|branch| FileMergeInput {
            branch_id: branch.clone(),
            expected_node_id: engine.file_branch_head(branch).unwrap(),
        })
        .collect::<Vec<_>>();
    let merge_node = engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &file,
            inputs: &inputs,
            chosen_node_id: &chosen,
            surviving_branch_id: &forks[0],
            location: None,
            principal: "owner",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap()
        .result["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    Graph {
        _root: root,
        engine,
        repo,
        file,
        base: base.version_id,
        merge_node,
        forks,
    }
}

fn query(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn page(engine: &Engine, file: &str, pairs: &[(&str, &str)]) -> Result<Value, ApiError> {
    history_pages::file_history(
        engine,
        crate::ReadFileHistory::parse(file.into(), query(pairs))?,
    )
    .and_then(|page| page.encode(PageBudget::PROTOCOL))
    .map(|encoded| storage_contract::strict_json(encoded.body()).unwrap())
}

fn ids(value: &Value) -> Vec<&str> {
    value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["node_id"].as_str().unwrap())
        .collect()
}

#[test]
fn parent_and_branch_pages_round_trip_through_every_previous_cursor() {
    let g = graph();
    let root = g.engine.history_root(&g.repo.history_tree_id).unwrap();
    for parents in [true, false] {
        let read = |pairs: &[(&str, &str)]| {
            let encoded = if parents {
                history_pages::node_parents(
                    &g.engine,
                    crate::ReadNodeParents::parse(
                        g.file.clone(),
                        g.merge_node.clone(),
                        query(pairs),
                    )
                    .unwrap(),
                )
                .unwrap()
                .encode(PageBudget::PROTOCOL)
            } else {
                history_pages::branches(
                    &g.engine,
                    crate::ReadHistoryBranches::parse(g.file.clone(), query(pairs)).unwrap(),
                )
                .unwrap()
                .encode(PageBudget::PROTOCOL)
            }
            .unwrap();
            storage_contract::strict_json(encoded.body()).unwrap()
        };
        let mut pages = vec![read(&[("history_root", root.as_str()), ("limit", "7")])];
        while let Some(cursor) = pages.last().unwrap()["next_cursor"].as_str() {
            let next = read(&[("cursor", cursor), ("limit", "7")]);
            pages.push(next);
            assert!(pages.len() < 20, "forward navigation must terminate");
        }
        assert!(pages.len() > 3);
        let collection = if parents { "parents" } else { "branches" };
        let identity = if parents { "node_id" } else { "branch_id" };
        let all = pages
            .iter()
            .flat_map(|page| page[collection].as_array().unwrap())
            .map(|row| row[identity].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        let mut current = pages.last().unwrap().clone();
        for expected in pages[..pages.len() - 1].iter().rev() {
            current = read(&[
                ("cursor", current["previous_cursor"].as_str().unwrap()),
                ("limit", "7"),
            ]);
            assert_eq!(
                &current, expected,
                "backward cursors reconstruct each count-bounded page"
            );
        }
        assert!(current["previous_cursor"].is_null());
    }
}

#[test]
fn escaped_field_pages_fit_the_wire_budget_and_reassemble_with_utf8_progress() {
    let mut g = graph();
    let message = "\0\u{1f}é😀\"\\中".repeat(160_000);
    let saved = save(
        &mut g.engine,
        &g.repo.main_branch_id,
        "doc.txt",
        b"large message",
        &message,
    );
    let root = g.engine.history_root(&g.repo.history_tree_id).unwrap();
    let mut offset = 0;
    let mut assembled = String::new();
    loop {
        let offset_text = offset.to_string();
        let page = history_pages::node_field(
            &g.engine,
            crate::ReadNodeField::parse(
                g.file.clone(),
                saved.version_id.clone(),
                "message".into(),
                query(&[
                    ("history_root", root.as_str()),
                    ("offset", &offset_text),
                    ("max_bytes", "1048576"),
                ]),
            )
            .unwrap(),
        )
        .unwrap();
        let encoded = page.encode().unwrap();
        assert!(encoded.body().len() <= storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES);
        assert_eq!(page.offset, offset);
        assert!(
            page.text.len()
                <= (storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES - 8192) / 6
        );
        let value: Value = storage_contract::strict_json(encoded.body()).unwrap();
        assert_eq!(value["text"].as_str().unwrap(), page.text);
        assembled.push_str(&page.text);
        if page.complete {
            break;
        }
        assert!(!page.text.is_empty());
        offset += page.text.len();
        assert_eq!(value["next_offset"].as_u64().unwrap() as usize, offset);
    }
    assert_eq!(assembled, message);
    for (offset, bytes) in [(2, 1), (4, 3), (3, 100)] {
        let error = history_pages::node_field(
            &g.engine,
            crate::ReadNodeField::parse(
                g.file.clone(),
                saved.version_id.clone(),
                "message".into(),
                query(&[
                    ("history_root", root.as_str()),
                    ("offset", &offset.to_string()),
                    ("max_bytes", &bytes.to_string()),
                ]),
            )
            .unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
    }
}

#[test]
fn root_binding_rejects_stale_cursors_and_foreign_queries_but_not_unrelated_writes() {
    let mut g = graph();
    let first = page(&g.engine, &g.file, &[("limit", "10")]).unwrap();
    let root = first["history_root"].as_str().unwrap().to_owned();
    assert_eq!(
        root,
        g.engine
            .history_root(&g.repo.history_tree_id)
            .unwrap()
            .as_str()
    );
    assert_eq!(ids(&first).len(), 10);
    let next = first["next_cursor"].as_str().unwrap().to_owned();
    assert!(first["previous_cursor"].is_null());
    // A cursor continues only the same query: another limit, context or file is a 400.
    let mismatch = page(&g.engine, &g.file, &[("cursor", &next), ("limit", "11")]).unwrap_err();
    assert_eq!(mismatch.message, "cursor does not match the history query");
    let other_context = page(
        &g.engine,
        &g.file,
        &[
            ("cursor", &next),
            ("repository_id", &g.repo.repository_id),
            ("path", "doc.txt"),
        ],
    )
    .unwrap_err();
    assert_eq!(other_context.status, http::StatusCode::BAD_REQUEST);
    let second = page(&g.engine, &g.file, &[("cursor", &next), ("limit", "10")]).unwrap();
    assert_eq!(ids(&second).len(), 10);
    assert!(second["previous_cursor"].is_string());
    assert!(ids(&first).iter().all(|id| !ids(&second).contains(id)));
    // A write to an unrelated history leaves this root (and the cursor) valid.
    let other = g
        .engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save(&mut g.engine, &other.main_branch_id, "o.txt", b"o", "other");
    assert_eq!(
        page(&g.engine, &g.file, &[("cursor", &next), ("limit", "10")]).unwrap(),
        second
    );
    // A note on this file changes the root: the cursor and an explicit root
    // are refused with 409, a fresh first page captures the new root.
    g.engine
        .set_file_note(
            &g.file,
            None,
            &storage_contract::Note::parse("n".into()).unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let stale = page(&g.engine, &g.file, &[("cursor", &next), ("limit", "10")]).unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::HistoryChanged);
    assert_eq!(stale.status, http::StatusCode::CONFLICT);
    let stale = page(
        &g.engine,
        &g.file,
        &[("history_root", &root), ("limit", "10")],
    )
    .unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::HistoryChanged);
    let fresh = page(&g.engine, &g.file, &[("limit", "10")]).unwrap();
    assert_ne!(fresh["history_root"], root);
    assert_eq!(fresh["context"]["note"], "n");
    // Object authorization precedes the root comparison: an unknown file is
    // 404 even with a stale root, never 409.
    let missing = page(&g.engine, "file_missing", &[("history_root", &root)]).unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::FileNotFound);
}

#[test]
fn anchors_position_the_page_and_parent_previews_always_remain_continuable() {
    let g = graph();
    let all = page(&g.engine, &g.file, &[("limit", "200")]).unwrap();
    let root = all["history_root"].as_str().unwrap();
    let order = ids(&all)
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    assert!(all["next_cursor"].is_null());
    let position = order.iter().position(|id| *id == g.merge_node).unwrap();
    let anchored = page(
        &g.engine,
        &g.file,
        &[
            ("anchor_node_id", &g.merge_node),
            ("limit", "3"),
            ("history_root", root),
        ],
    )
    .unwrap();
    assert_eq!(
        anchored["anchor"],
        json!({"node_id": g.merge_node, "in_page": true})
    );
    assert_eq!(
        ids(&anchored),
        order[position..position + 3]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert_eq!(anchored["previous_cursor"].is_string(), position > 0);
    assert!(anchored["next_cursor"].is_string());
    let last = order.last().unwrap();
    let tail = page(
        &g.engine,
        &g.file,
        &[
            ("anchor_node_id", last),
            ("limit", "3"),
            ("history_root", root),
        ],
    )
    .unwrap();
    assert_eq!(ids(&tail), [last.as_str()]);
    assert!(tail["next_cursor"].is_null());
    let unknown = page(
        &g.engine,
        &g.file,
        &[("anchor_node_id", "node_missing"), ("history_root", root)],
    )
    .unwrap_err();
    assert_eq!(unknown.code, storage_contract::ErrorCode::FileNodeNotFound);
    assert_eq!(unknown.status, http::StatusCode::NOT_FOUND);
    // The merge node has 41 parents; the preview holds 32 and continues.
    let merge = all["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == g.merge_node)
        .unwrap();
    assert_eq!(merge["kind"], "merge");
    assert_eq!(merge["parents"]["status"], "more");
    let edges = merge["parents"]["edges"].as_array().unwrap();
    assert_eq!(
        edges.len(),
        crate::backend_file_history::limits::MAX_PARENT_PREVIEW_PER_NODE
    );
    assert!(edges.iter().all(|edge| edge["in_window"] == true));
    let cursor = merge["parents"]["cursor"].as_str().unwrap().to_owned();
    let root = all["history_root"].as_str().unwrap().to_owned();
    let mut collected = edges
        .iter()
        .map(|edge| edge["node_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let mut next = Some(cursor);
    while let Some(cursor) = next.take() {
        let more = history_pages::node_parents(
            &g.engine,
            crate::ReadNodeParents::parse(
                g.file.clone(),
                g.merge_node.clone(),
                query(&[("cursor", &cursor), ("limit", "4")]),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(more.history_root, root);
        assert!(more.parents.len() <= 4);
        collected.extend(more.parents.iter().cloned());
        let encoded: Value =
            json_codec::from_slice(more.encode(PageBudget::PROTOCOL).unwrap().body()).unwrap();
        next = encoded["next_cursor"].as_str().map(str::to_owned);
    }
    let expected = g
        .engine
        .file_node_parents(&g.file, &g.merge_node)
        .unwrap()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(collected.iter().cloned().collect::<BTreeSet<_>>(), expected);
    assert_eq!(collected.len(), 41);
    assert!(
        collected.windows(2).all(|pair| pair[0] < pair[1]),
        "parent ids ascend across preview and pages"
    );
    // A continuation whose node differs from the path is a malformed query.
    let foreign = history_pages::node_parents(
        &g.engine,
        crate::ReadNodeParents::parse(
            g.file.clone(),
            g.base.clone(),
            query(&[("cursor", merge["parents"]["cursor"].as_str().unwrap())]),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(foreign.status, http::StatusCode::BAD_REQUEST);
    // The base node has no parents: complete with zero edges.
    let base = all["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == g.base)
        .unwrap();
    assert_eq!(
        base["parents"],
        json!({"status":"complete","edges":[],"cursor":null})
    );
    // In a small page the edges of visible parents point outside the window.
    let small = page(
        &g.engine,
        &g.file,
        &[
            ("anchor_node_id", &g.merge_node),
            ("limit", "1"),
            ("history_root", &root),
        ],
    )
    .unwrap();
    let merge = &small["nodes"][0];
    assert!(
        merge["parents"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["in_window"] == false)
    );
}

#[test]
fn page_parent_edge_budget_leaves_later_nodes_a_restart_cursor() {
    let g = graph();
    // 200 nodes with up to 32 previews each cannot exceed 2048 edges here, so
    // model the exhausted budget through the per-page limit semantics: a page
    // whose previews consume the budget must still mint a cursor per node.
    let all = history_pages::file_history(
        &g.engine,
        crate::ReadFileHistory::parse(g.file.clone(), query(&[("limit", "200")])).unwrap(),
    )
    .unwrap();
    let edges =
        all.nodes
            .iter()
            .map(|node| match &node.parents {
                crate::ParentPreview::Complete(edges)
                | crate::ParentPreview::More { edges, .. } => edges.len(),
            })
            .sum::<usize>();
    assert!(edges <= crate::backend_file_history::limits::MAX_PAGE_PARENT_EDGES);
    for node in &all.nodes {
        if let crate::ParentPreview::More { cursor, .. } = &node.parents {
            let decoded: crate::backend_file_history::ParentCursor = json_codec::from_slice(
                &cursor
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            decoded.validate().unwrap();
            assert_eq!(decoded.key.node_id, node.node_id);
        }
    }
    let _ = &g.forks;
}

#[test]
fn branch_merge_version_and_field_pages_bind_the_same_root() {
    let g = graph();
    let first = page(&g.engine, &g.file, &[("limit", "1")]).unwrap();
    let root = first["history_root"].as_str().unwrap().to_owned();
    let branches = history_pages::branches(
        &g.engine,
        crate::ReadHistoryBranches::parse(
            g.file.clone(),
            query(&[("history_root", &root), ("limit", "7")]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(branches.history_root, root);
    assert_eq!(branches.branches.len(), 7);
    assert_eq!(
        branches.selected_branch_id.as_deref(),
        Some(g.forks[0].as_str())
    );
    let encoded: Value =
        json_codec::from_slice(branches.encode(PageBudget::PROTOCOL).unwrap().body()).unwrap();
    let mut collected = encoded["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|branch| branch["branch_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let mut next = encoded["next_cursor"].as_str().map(str::to_owned);
    while let Some(cursor) = next.take() {
        let page = history_pages::branches(
            &g.engine,
            crate::ReadHistoryBranches::parse(
                g.file.clone(),
                query(&[("cursor", &cursor), ("limit", "7")]),
            )
            .unwrap(),
        )
        .unwrap();
        let encoded: Value =
            json_codec::from_slice(page.encode(PageBudget::PROTOCOL).unwrap().body()).unwrap();
        collected.extend(
            encoded["branches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|branch| branch["branch_id"].as_str().unwrap().to_owned()),
        );
        next = encoded["next_cursor"].as_str().map(str::to_owned);
    }
    let expected = g
        .engine
        .state()
        .branches
        .values()
        .filter(|branch| {
            branch.file_id.as_deref() == Some(g.file.as_str())
                && branch.state != storage_contract::BranchState::Migrated
        })
        .map(|branch| branch.branch_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(collected, expected);
    let current = encoded["branches"]
        .as_array()
        .unwrap()
        .iter()
        .chain(std::iter::empty())
        .count();
    assert!(current <= 7);
    let merged = encoded["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["state"] == "merged");
    if let Some(merged) = merged {
        assert_eq!(merged["read_only"], true);
        assert!(merged["alias_of"].is_string());
        assert!(merged.get("pending_file_versions").is_none());
        assert!(merged["pending_count"].is_number());
    }
    let merges = merge_page(
        &g.engine,
        crate::ReadHistoryMerges::parse(g.file.clone(), query(&[("history_root", &root)])).unwrap(),
    )
    .unwrap();
    assert_eq!(merges.merges.len(), 1);
    let merge = &merges.merges[0];
    assert_eq!((merge.input_count, merge.parent_count), (40, 41));
    assert_eq!(merge.result_node_id, g.merge_node);
    let inputs = history_pages::merge_inputs(
        &g.engine,
        &ReadIndexedSlice::parse(
            g.file.clone(),
            "merge_id",
            merge.merge_id.clone(),
            query(&[("history_root", &root), ("offset", "38"), ("limit", "5")]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(inputs.total, 40);
    assert_eq!(inputs.items.len(), 2);
    assert_eq!(inputs.items[0].0, g.forks[38]);
    let parents = history_pages::merge_parents(
        &g.engine,
        &ReadIndexedSlice::parse(
            g.file.clone(),
            "merge_id",
            merge.merge_id.clone(),
            query(&[("history_root", &root), ("limit", "1000")]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        parents.items,
        g.engine.state().file_branch_merges[&merge.merge_id].parent_node_ids
    );
    let stale = merge_page(
        &g.engine,
        crate::ReadHistoryMerges::parse(
            g.file.clone(),
            query(&[("history_root", &"00".repeat(32))]),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::HistoryChanged);
    let unknown_merge = history_pages::merge_inputs(
        &g.engine,
        &ReadIndexedSlice::parse(
            g.file.clone(),
            "merge_id",
            "fmerge_missing".into(),
            query(&[("history_root", &root)]),
        )
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(unknown_merge.status, http::StatusCode::NOT_FOUND);
    let versions = version_page(
        &g.engine,
        crate::ReadVersionPage::parse(g.file.clone(), query(&[("limit", "500")])).unwrap(),
    )
    .unwrap();
    assert_eq!(versions.history_root, root);
    assert_eq!(versions.versions.len(), 41);
    assert!(versions.versions.windows(2).all(|pair| (
        pair[0].version_sequence,
        &pair[0].version_id
    ) < (
        pair[1].version_sequence,
        &pair[1].version_id
    )));
    let sources = history_pages::version_sources(
        &g.engine,
        &ReadIndexedSlice::parse(
            g.file.clone(),
            "version_id",
            versions.versions[1].version_id.clone(),
            query(&[("history_root", &root)]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        sources.items,
        g.engine.state().versions[&versions.versions[1].version_id].source_version_ids
    );
    let field = history_pages::node_field(
        &g.engine,
        crate::ReadNodeField::parse(
            g.file.clone(),
            g.base.clone(),
            "message".into(),
            query(&[("history_root", &root), ("max_bytes", "2")]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            field.total_bytes,
            field.offset,
            field.text.as_str(),
            field.complete
        ),
        (4, 0, "ba", false)
    );
    let rest = history_pages::node_field(
        &g.engine,
        crate::ReadNodeField::parse(
            g.file.clone(),
            g.base.clone(),
            "message".into(),
            query(&[("history_root", &root), ("offset", "2")]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!((rest.text.as_str(), rest.complete), ("se", true));
    let empty = version_page(
        &g.engine,
        crate::ReadVersionPage::parse("file_missing".into(), BTreeMap::new()).unwrap(),
    )
    .unwrap_err();
    assert_eq!(empty.code, storage_contract::ErrorCode::FileNotFound);
}

/// A lost reply is retried with the same request: the same root yields the
/// same bytes, including both cursors, on every page in both directions.
#[test]
fn retried_requests_under_the_same_root_return_byte_identical_pages() {
    let g = graph();
    let body = |pairs: &[(&str, &str)]| {
        history_pages::file_history(
            &g.engine,
            crate::ReadFileHistory::parse(g.file.clone(), query(pairs)).unwrap(),
        )
        .and_then(|page| page.encode(PageBudget::PROTOCOL))
        .map(|encoded| encoded.body().to_vec())
        .unwrap()
    };
    let mut forward = vec![body(&[("limit", "3")])];
    assert_eq!(body(&[("limit", "3")]), forward[0]);
    loop {
        let last: Value = storage_contract::strict_json(forward.last().unwrap()).unwrap();
        let Some(next) = last["next_cursor"].as_str() else {
            break;
        };
        let page = body(&[("cursor", next), ("limit", "3")]);
        assert_eq!(
            body(&[("cursor", next), ("limit", "3")]),
            page,
            "retry of a forward page differs"
        );
        forward.push(page);
    }
    assert!(forward.len() > 2, "the fixture spans several pages");
    // Walking back from the last page revisits the same node sets.
    let mut page: Value = storage_contract::strict_json(forward.last().unwrap()).unwrap();
    let mut seen = ids(&page)
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    while let Some(previous) = page["previous_cursor"].as_str().map(str::to_owned) {
        let bytes = body(&[("cursor", &previous), ("limit", "3")]);
        assert_eq!(
            body(&[("cursor", &previous), ("limit", "3")]),
            bytes,
            "retry of a backward page differs"
        );
        page = storage_contract::strict_json(&bytes).unwrap();
        let mut ids = ids(&page)
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        ids.extend(seen);
        seen = ids;
    }
    let all: Value = storage_contract::strict_json(&body(&[("limit", "200")])).unwrap();
    assert_eq!(
        seen,
        ids(&all),
        "backward navigation reaches every node exactly once"
    );
}

// Synchronous convenience for the existing transport projection fixtures.
fn version_page(engine: &Engine, request: crate::ReadVersionPage) -> Result<VersionPage, ApiError> {
    history_pages::prepare_version_page(engine, request)?.read()
}

fn merge_page(
    engine: &Engine,
    request: crate::ReadHistoryMerges,
) -> Result<HistoryMergesPage, ApiError> {
    Ok(history_pages::prepare_merge_page(engine, request)?.read())
}

#[test]
fn detached_merge_page_keeps_captured_wire_after_publication() {
    let mut g = graph();
    let root = g.engine.history_root(&g.repo.history_tree_id).unwrap();
    let request = || {
        crate::ReadHistoryMerges::parse(
            g.file.clone(),
            query(&[("history_root", root.as_str()), ("limit", "1")]),
        )
        .unwrap()
    };
    let expected = merge_page(&g.engine, request())
        .unwrap()
        .encode(PageBudget::PROTOCOL)
        .unwrap()
        .body()
        .to_vec();
    let plan = history_pages::prepare_merge_page(&g.engine, request()).unwrap();
    save(
        &mut g.engine,
        &g.repo.main_branch_id,
        "doc.txt",
        b"later",
        "later",
    );
    assert_ne!(
        g.engine.history_root(&g.repo.history_tree_id).unwrap(),
        root
    );
    let captured = plan.read();
    assert_eq!(captured.merges[0].input_count, 40);
    assert_eq!(captured.merges[0].parent_count, 41);
    assert_eq!(
        captured.encode(PageBudget::PROTOCOL).unwrap().body(),
        expected.as_slice()
    );
    assert_eq!(
        history_pages::prepare_merge_page(&g.engine, request())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::HistoryChanged
    );
}
