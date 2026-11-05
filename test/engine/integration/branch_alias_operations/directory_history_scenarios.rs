use super::*;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    ErrorCode, OperationId,
    directory_history::*,
    history_window::{HistoryEndpoint, HistoryWindowCancellation},
};

fn commit_path(p: &mut Pool, path: &str) {
    p.checkpoint(path);
}

fn request(
    p: &Pool,
    directory: &str,
    collection: DirectoryHistoryCollection,
    query: BTreeMap<String, String>,
) -> DirectoryHistoryRequest {
    DirectoryHistoryRequest::parse(&p.repo.repository_id, directory, collection, query).unwrap()
}
fn page(p: &Pool, request: DirectoryHistoryRequest) -> DirectoryHistoryPage {
    let read = p
        .engine
        .capture_directory_history(request, HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    p.engine.recheck_directory_history(&read).unwrap();
    page
}
#[test]
fn directory_history_empty_root_reads_do_not_initialize_branches() {
    let p = Pool::new();
    let before = storage_engine::canonical_json(p.engine.state()).unwrap();
    for collection in [
        DirectoryHistoryCollection::Nodes,
        DirectoryHistoryCollection::Branches,
    ] {
        let page = page(
            &p,
            request(&p, &p.repo.repository_id, collection, BTreeMap::new()),
        );
        assert!(page.nodes.is_empty());
        assert!(page.branches.is_empty());
        assert_eq!(page.current.directory_branch_id, None);
    }
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before
    );
}
#[test]
fn directory_history_pages_real_edges_and_pending_branches_share_a_real_head() {
    let mut p = Pool::new();
    p.save("docs/a.txt", b"one");
    commit_path(&mut p, "docs");
    let directory = p.current_entry("docs").entry_id;
    let first = p.register("docs", "pending one");
    let second = p.register("docs", "pending two");
    let before = storage_engine::canonical_json(p.engine.state()).unwrap();
    let branches = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Branches,
            BTreeMap::new(),
        ),
    );
    let a = branches
        .branches
        .iter()
        .find(|b| b.directory_branch_id == first)
        .unwrap();
    let b = branches
        .branches
        .iter()
        .find(|b| b.directory_branch_id == second)
        .unwrap();
    assert!(!a.materialized && !b.materialized);
    assert_eq!(a.head_tree_id, b.head_tree_id);
    assert!(a.committed_tree_id.is_some());
    assert_eq!(a.committed_tree_id, b.committed_tree_id);
    let nodes = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    assert_eq!(
        nodes
            .nodes
            .iter()
            .filter(|n| Some(n.tree_id.as_str()) == a.committed_tree_id.as_deref())
            .count(),
        1
    );
    assert!(
        !nodes
            .nodes
            .iter()
            .any(|n| n.directory_branch_id == first || n.directory_branch_id == second)
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before
    );
    p.select("docs", "pending one");
    p.save("docs/a.txt", b"two");
    commit_path(&mut p, "docs");
    p.save("docs/a.txt", b"three");
    commit_path(&mut p, "docs");
    let mut query = BTreeMap::from([("limit".into(), "1".into())]);
    let mut seen = BTreeSet::new();
    let mut first_page = None;
    loop {
        let window = page(
            &p,
            request(
                &p,
                &directory,
                DirectoryHistoryCollection::Nodes,
                query.clone(),
            ),
        );
        assert_eq!(window.nodes.len(), 1);
        if first_page.is_none() {
            first_page = Some(window.clone());
        }
        let node = &window.nodes[0];
        assert!(seen.insert(node.tree_id.clone()));
        let raw = &p.engine.state().directory_trees[&node.tree_id];
        let parent = raw
            .parent_tree_id
            .as_ref()
            .and_then(|id| p.engine.state().directory_trees.get(id))
            .and_then(|tree| {
                storage_contract::directory_staging::committed_directory_ancestor(
                    p.engine.state(),
                    tree,
                )
            })
            .map(|tree| tree.tree_id.clone());
        assert_eq!(node.parent_tree_id, parent);
        assert_eq!(
            node.parent_endpoint,
            parent.as_ref().map(|_| HistoryEndpoint::OutsideWindow)
        );
        match window.page.next_cursor {
            Some(cursor) => {
                query.insert("cursor".into(), cursor);
            }
            None => break,
        }
    }
    let expected = p
        .engine
        .state()
        .directory_trees
        .values()
        .filter(|tree| {
            p.engine.state().directory_branches[&tree.directory_branch_id].directory_id == directory
                && storage_contract::directory_staging::directory_tree_is_committed(
                    p.engine.state(),
                    tree,
                )
        })
        .map(|tree| tree.tree_id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(seen, expected);
    let first_page = first_page.unwrap();
    let next = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::from([
                ("limit".into(), "1".into()),
                (
                    "cursor".into(),
                    first_page.page.next_cursor.clone().unwrap(),
                ),
            ]),
        ),
    );
    let previous = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::from([
                ("limit".into(), "1".into()),
                ("cursor".into(), next.page.previous_cursor.unwrap()),
            ]),
        ),
    );
    assert_eq!(previous, first_page);
    let anchored = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::from([("anchor_tree_id".into(), next.nodes[0].tree_id.clone())]),
        ),
    );
    assert_eq!(anchored.nodes[0].tree_id, next.nodes[0].tree_id);
    let branch_page = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Branches,
            BTreeMap::from([("limit".into(), "1".into())]),
        ),
    );
    assert!(branch_page.page.next_cursor.is_some());
    let branch_next = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Branches,
            BTreeMap::from([
                ("limit".into(), "1".into()),
                ("cursor".into(), branch_page.page.next_cursor.unwrap()),
            ]),
        ),
    );
    assert_ne!(
        branch_next.branches[0].directory_branch_id,
        branch_page.branches[0].directory_branch_id
    );
}
#[test]
fn directory_history_stable_identity_survives_rename_delete_and_path_recreation() {
    let mut p = Pool::new();
    p.save("docs/a.txt", b"one");
    commit_path(&mut p, "docs");
    let directory = p.current_entry("docs").entry_id;
    p.register("docs", "retained");
    let old = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    p.engine
        .file_namespace(
            &p.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "docs",
                Some("archive"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    commit_path(&mut p, "archive");
    assert_eq!(p.current_entry("archive").entry_id, directory);
    let renamed = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    assert!(
        old.nodes
            .iter()
            .all(|n| renamed.nodes.iter().any(|r| r.tree_id == n.tree_id))
    );
    p.save("archive/a.txt", b"uncommitted directory changes");
    let dirty_branch = p.engine.state().directory_git_repositories[&directory]
        .current_branch_id
        .clone();
    p.engine
        .file_namespace(
            &p.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::DeleteTree,
                "archive",
                None,
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let deleted = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    assert_eq!(deleted.current.directory_branch_id, None);
    assert!(!deleted.nodes.is_empty());
    let deleted_branches = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Branches,
            BTreeMap::new(),
        ),
    );
    assert_eq!(deleted_branches.current.directory_branch_id, None);
    let retained_dirty = deleted_branches
        .branches
        .iter()
        .find(|branch| branch.directory_branch_id == dirty_branch)
        .unwrap();
    assert!(!retained_dirty.current && retained_dirty.staged);
    p.save("archive/new.txt", b"new identity");
    commit_path(&mut p, "archive");
    let replacement = p.current_entry("archive").entry_id;
    assert_ne!(replacement, directory);
    let new = page(
        &p,
        request(
            &p,
            &replacement,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    assert!(
        new.nodes
            .iter()
            .all(|n| !deleted.nodes.iter().any(|old| old.tree_id == n.tree_id))
    );
    let old_again = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Nodes,
            BTreeMap::new(),
        ),
    );
    assert_eq!(old_again.nodes, deleted.nodes);
    let old_branches = page(
        &p,
        request(
            &p,
            &directory,
            DirectoryHistoryCollection::Branches,
            BTreeMap::new(),
        ),
    );
    assert_eq!(old_branches.current.directory_branch_id, None);
    assert_eq!(old_branches.branches, deleted_branches.branches);
    let foreign = storage_contract::new_id("entry").unwrap();
    assert_eq!(
        p.engine
            .capture_directory_history(
                request(
                    &p,
                    &foreign,
                    DirectoryHistoryCollection::Nodes,
                    BTreeMap::new()
                ),
                HistoryWindowCancellation::new()
            )
            .err()
            .unwrap()
            .code(),
        ErrorCode::DirectoryNotFound
    );
}
#[test]
fn directory_history_rejects_root_drift_cross_scope_anchor_and_cancellation() {
    let mut p = Pool::new();
    p.save("docs/a.txt", b"one");
    commit_path(&mut p, "docs");
    p.register("docs", "retained");
    let directory = p.current_entry("docs").entry_id;
    let request = request(
        &p,
        &directory,
        DirectoryHistoryCollection::Nodes,
        BTreeMap::new(),
    );
    let read = p
        .engine
        .capture_directory_history(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let before = read.page().unwrap();
    p.save("docs/a.txt", b"two");
    commit_path(&mut p, "docs");
    assert_eq!(
        p.engine
            .recheck_directory_history(&read)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    let stale = DirectoryHistoryRequest::parse(
        &p.repo.repository_id,
        &directory,
        DirectoryHistoryCollection::Nodes,
        BTreeMap::from([(
            "history_root".into(),
            before.identity.history_root.to_string(),
        )]),
    )
    .unwrap();
    assert_eq!(
        p.engine
            .capture_directory_history(stale, HistoryWindowCancellation::new())
            .err()
            .unwrap()
            .code(),
        ErrorCode::HistoryChanged
    );
    let cancel = HistoryWindowCancellation::new();
    let read = p
        .engine
        .capture_directory_history(request, cancel.clone())
        .unwrap();
    cancel.cancel();
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::OperationAborted);
    let wrong = DirectoryHistoryRequest::parse(
        &p.repo.repository_id,
        &p.repo.repository_id,
        DirectoryHistoryCollection::Nodes,
        BTreeMap::from([("anchor_tree_id".into(), before.nodes[0].tree_id.clone())]),
    )
    .unwrap();
    let read = p
        .engine
        .capture_directory_history(wrong, HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(
        read.page().unwrap_err().code(),
        ErrorCode::DirectoryNotFound
    );
}
