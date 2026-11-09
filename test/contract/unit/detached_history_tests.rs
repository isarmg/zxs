use super::*;
fn file(index: usize) -> File {
    File {
        file_id: HistoryFileId::parse(&format!("file_{index:032x}")).unwrap(),
        repository_id: HistoryRepositoryId::parse(&format!("repo_{}", "a".repeat(32))).unwrap(),
        history_tree_id: HistoryTreeId::parse(&format!("history_{}", "b".repeat(32))).unwrap(),
        history_root: HistoryRoot::parse(&"c".repeat(64)).unwrap(),
        representative_version_id: HistoryVersionId::parse(&format!("ver_{index:032x}")).unwrap(),
        recorded_name: "retained.txt".into(),
        representative_size: 4,
        retained_versions: 2,
    }
}
#[test]
fn discovery_requires_complete_current_cursor_and_sorted_unique_file_identity() {
    let root = file(1).history_root;
    let request = Request {
        snapshot: Some(root),
        after_file: Some(file(1).file_id),
    };
    let mut page = Page {
        snapshot: root,
        files: vec![file(2), file(3)],
        next_after: None,
    };
    page.validate(&request).unwrap();
    page.files.swap(0, 1);
    assert!(page.validate(&request).is_err());
    page.files = vec![file(2), file(2)];
    assert!(page.validate(&request).is_err());
    page.files = vec![file(1)];
    assert!(page.validate(&request).is_err());
    assert!(
        Request {
            snapshot: Some(root),
            after_file: None
        }
        .validate()
        .is_err()
    );
    assert!(
        Request {
            snapshot: None,
            after_file: Some(file(1).file_id)
        }
        .validate()
        .is_err()
    );
}
#[test]
fn next_page_and_wire_boundaries_do_not_accept_truncated_or_retargeted_records() {
    let root = file(1).history_root;
    let mut page = Page {
        snapshot: root,
        files: (0..PAGE_SIZE).map(file).collect(),
        next_after: Some(file(PAGE_SIZE - 1).file_id),
    };
    page.validate(&Request::default()).unwrap();
    page.next_after = Some(file(0).file_id);
    assert!(page.validate(&Request::default()).is_err());
    page.next_after = None;
    page.files[0].retained_versions = 0;
    assert!(page.validate(&Request::default()).is_err());
    let mut wire = json_codec::to_value(file(1)).unwrap();
    wire["current_path"] = "invented/current/file.txt".into();
    assert!(json_codec::from_value::<File>(wire).is_err());
}
