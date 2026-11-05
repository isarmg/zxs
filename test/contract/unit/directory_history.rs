use super::*;
const REPO: &str = "repo_00000000000000000000000000000000";
const DIR: &str = "entry_00000000000000000000000000000000";
const TREE: &str = "dtree_00000000000000000000000000000000";
fn request(query: BTreeMap<String, String>) -> DirectoryHistoryRequest {
    DirectoryHistoryRequest::parse(REPO, DIR, DirectoryHistoryCollection::Nodes, query).unwrap()
}
#[test]
fn directory_history_query_and_cursor_bind_stable_identity_collection_and_limit() {
    let request = request(BTreeMap::new());
    assert_eq!(request.limit(), 64);
    crate::api_path::validate_api_path(&request.route()).unwrap();
    let root = HistoryRoot::parse(&"a".repeat(64)).unwrap();
    let cursor = request
        .page_cursor(root, 7, TREE, DirectoryHistoryDirection::After)
        .unwrap();
    let query = BTreeMap::from([("cursor".into(), cursor.clone())]);
    let next =
        DirectoryHistoryRequest::parse(REPO, DIR, DirectoryHistoryCollection::Nodes, query.clone())
            .unwrap();
    assert_eq!(next.expected_root(), Some(root));
    assert_eq!(next.cursor().unwrap().id(), TREE);
    assert!(
        DirectoryHistoryRequest::parse(
            REPO,
            REPO,
            DirectoryHistoryCollection::Nodes,
            query.clone()
        )
        .is_err()
    );
    assert!(
        DirectoryHistoryRequest::parse(
            REPO,
            DIR,
            DirectoryHistoryCollection::Branches,
            query.clone()
        )
        .is_err()
    );
    for (key, value) in [
        ("limit", "0"),
        ("limit", "129"),
        ("limit", "nope"),
        ("cursor", "zz"),
        ("unknown", "yes"),
    ] {
        assert!(
            DirectoryHistoryRequest::parse(
                REPO,
                DIR,
                DirectoryHistoryCollection::Nodes,
                BTreeMap::from([(key.into(), value.into())])
            )
            .is_err()
        );
    }
    let mut wrong = query.clone();
    wrong.insert("history_root".into(), "b".repeat(64));
    assert!(
        DirectoryHistoryRequest::parse(REPO, DIR, DirectoryHistoryCollection::Nodes, wrong)
            .is_err()
    );
    let mut wrong = query;
    wrong.insert("limit".into(), "1".into());
    assert!(
        DirectoryHistoryRequest::parse(REPO, DIR, DirectoryHistoryCollection::Nodes, wrong)
            .is_err()
    );
    let anchor = BTreeMap::from([("anchor_tree_id".into(), TREE.into())]);
    assert!(
        DirectoryHistoryRequest::parse(
            REPO,
            DIR,
            DirectoryHistoryCollection::Nodes,
            anchor.clone()
        )
        .is_ok()
    );
    assert!(
        DirectoryHistoryRequest::parse(REPO, DIR, DirectoryHistoryCollection::Branches, anchor)
            .is_err()
    );
}
#[test]
fn directory_history_response_admission_rejects_fake_parent_boundaries_and_mixed_pages() {
    let request = request(BTreeMap::new());
    let mut page = DirectoryHistoryPage {
        identity: DirectoryHistoryIdentity {
            repository_id: REPO.into(),
            directory_id: DIR.into(),
            history_tree_id: "history_00000000000000000000000000000000".into(),
            history_root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
            collection: DirectoryHistoryCollection::Nodes,
            limit: 64,
        },
        current: DirectoryHistoryCurrent {
            committed_tree_id: None,
            staged: false,
            directory_branch_id: None,
            tree_id: None,
        },
        nodes: vec![DirectoryHistoryNode {
            message: "Commit directory".into(),
            tree_id: TREE.into(),
            directory_branch_id: "dbr_00000000000000000000000000000000".into(),
            parent_tree_id: Some("dtree_11111111111111111111111111111111".into()),
            parent_endpoint: Some(HistoryEndpoint::OutsideWindow),
            commit_id: "commit_00000000000000000000000000000000".into(),
            path: "docs".into(),
            sequence: 9,
        }],
        branches: vec![],
        page: DirectoryHistoryPagination {
            previous_cursor: None,
            next_cursor: None,
        },
    };
    page.validate(&request).unwrap();
    let encoded = json_codec::to_value(&page).unwrap();
    assert_eq!(encoded["nodes"][0]["sequence"], "9");
    assert_eq!(
        json_codec::from_value::<DirectoryHistoryPage>(encoded).unwrap(),
        page
    );
    page.nodes[0].parent_endpoint = Some(HistoryEndpoint::InWindow);
    assert!(page.validate(&request).is_err());
    page.nodes[0].parent_endpoint = Some(HistoryEndpoint::OutsideWindow);
    page.nodes.push(page.nodes[0].clone());
    assert!(page.validate(&request).is_err());
    page.nodes.pop();
    page.identity.collection = DirectoryHistoryCollection::Branches;
    assert!(page.validate(&request).is_err());
}
