use super::*;
use json_codec::json;

#[test]
fn commit_wire_borrows_collections_and_preserves_open_kind_and_canonical_bytes() {
    for populated in [false, true] {
        let mut commit = Commit {
            commit_id: "commit".into(),
            history_tree_id: "history".into(),
            kind: "extension.kind/α".into(),
            parent_commit_ids: vec!["parent".into()],
            tree_id: "tree".into(),
            sequence: u64::MAX,
            operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
            author: "author".into(),
            message: "message".into(),
            entries: vec![Entry {
                entry_id: "entry".into(),
                path: name_policy::PortablePath::parse("Café.txt").unwrap(),
                kind: storage_contract::NamespaceEntryKind::File,
                file_id: "file".into(),
                version_id: "version".into(),
                size: u64::MAX,
            }],
            namespace_deletions: None,
            pending_file_versions: BTreeMap::new(),
            provenance: None,
        };
        let mut expected = json!({"commit_id":"commit","history_tree_id":"history","kind":"extension.kind/α","parent_commit_ids":["parent"],"tree_id":"tree","sequence":"18446744073709551615","operation_id":"op_00000000000000000000000000000000","author":"author","message":"message","entries":[{"entry_id":"entry","path":"Café.txt","name":"Café.txt","lookup_key":"café.txt","kind":"file","file_id":"file","version_id":"version","size":"18446744073709551615"}],"provenance":null});
        if populated {
            commit
                .pending_file_versions
                .insert("file".into(), "source".into());
            commit.namespace_deletions = Some(NamespaceDeletions {
                timestamp: u64::MAX,
                paths: vec![storage_contract::DeletedPath {
                    path: "deleted.txt".into(),
                    kind: storage_contract::NamespaceEntryKind::File,
                }],
            });
            commit.provenance = Some("provenance".into());
            expected["pending_file_versions"] = json!({"file":"source"});
            expected["namespace_deletions"] = json!({"timestamp":"18446744073709551615","paths":[{"path":"deleted.txt","kind":"file"}]});
            expected["provenance"] = json!("provenance");
        }
        let commit = Arc::new(commit);
        let view = CommitRecord::from(commit.as_ref());
        assert_eq!(view.entries.as_ptr(), commit.entries.as_ptr());
        assert_eq!(
            view.parent_commit_ids.as_ptr(),
            commit.parent_commit_ids.as_ptr()
        );
        assert!(std::ptr::eq(
            view.pending_file_versions,
            &commit.pending_file_versions
        ));
        let response = CommitResponse::from(Arc::clone(&commit));
        assert_eq!(json_codec::to_value(&response).unwrap(), expected);
        assert_eq!(
            storage_contract::canonical_json(&response).unwrap(),
            storage_contract::canonical_json_value(&expected).unwrap()
        );
        assert_eq!(
            json_codec::to_vec(&response).unwrap(),
            json_codec::to_vec(commit.as_ref()).unwrap()
        );
        assert_eq!(Arc::strong_count(&commit), 2);
    }
}
