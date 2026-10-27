use super::*;
use json_codec::json;

fn query(values: &[(&str, &str)]) -> Result<ReadListing, ApiError> {
    ReadListing::parse(
        "P01".into(),
        values
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    )
}
#[test]
fn query_boundary_keeps_defaults_limits_lookup_spelling_and_selected_pair() {
    let request = query(&[]).unwrap();
    assert_eq!(request.limit, 200);
    assert_eq!(request.path(), "");
    assert_eq!(request.query, "");
    for limit in ["1", "500"] {
        assert!(query(&[("limit", limit)]).is_ok());
    }
    for limit in ["0", "501", "-1", "1.5", "18446744073709551616"] {
        assert!(query(&[("limit", limit)]).is_err());
    }
    assert!(query(&[("query", &"é".repeat(512))]).is_ok());
    assert!(query(&[("query", &"é".repeat(513))]).is_err());
    assert!(query(&[("unexpected", "")]).is_err());
    for path in ["../escape", "/absolute", "a//b"] {
        assert!(query(&[("path", path)]).is_err());
    }
    let request = query(&[
        ("path", "Cafe\u{301}"),
        ("selected_path", "CAFÉ/File"),
        ("selected_entry_id", "entry"),
    ])
    .unwrap();
    assert_eq!(request.path(), "Cafe\u{301}");
    assert_eq!(request.path.as_ref().unwrap().key(), "café");
    assert_eq!(request.selected, Some(("café/file".into(), "entry".into())));
    for pairs in [
        vec![("selected_path", "item")],
        vec![("selected_entry_id", "entry")],
        vec![
            ("selected_path", "other/item"),
            ("selected_entry_id", "entry"),
        ],
        vec![("selected_path", "item"), ("selected_entry_id", "")],
    ] {
        assert!(query(&pairs).is_err());
    }
    for cursor in [
        String::new(),
        "a".into(),
        "gg".into(),
        "00".into(),
        "a".repeat(MAX_CURSOR_BYTES + 2),
    ] {
        assert!(query(&[("cursor", &cursor)]).is_err());
    }
}
fn entry(version: ListingVersion) -> ListingEntry {
    ListingEntry {
        entry_id: "entry".into(),
        name: "file".into(),
        path: "Folder/file".into(),
        kind: storage_contract::NamespaceEntryKind::File,
        size: 9_007_199_254_740_993,
        file_id: "file-id".into(),
        version,
        location: Some(ListingLocation {
            repository_id: "repo".into(),
            repository_path: Some("file".into()),
            container: true,
            selected_branch_id: None,
        }),
        note: Some(String::new()),
        node_id: Some("node".into()),
        read_only: Some(false),
        backup: false,
        historical_only: None,
    }
}
#[test]
fn pending_and_backup_version_overlay_preserves_wire_fields_without_duplicate_keys() {
    let pending = || ContentDispositionSnapshot {
        version_id: Some("source".into()),
        source_version_id: Some("source".into()),
        stored_state: None,
    };
    let normal = json_codec::to_value(ListingEntryResponse::from(entry(ListingVersion::File {
        content: pending(),
        backup_version: None,
    })))
    .unwrap();
    assert_eq!(
        normal,
        json!({"entry_id":"entry","name":"file","path":"Folder/file","kind":"file","size":"9007199254740993","file_id":"file-id",
        "repository_id":"repo","repository_path":"file","container":true,"selected_branch_id":null,"note":"","node_id":"node","read_only":false,
        "version_id":"source","materialized":false,"inherited":true,"own_version_id":null,"source_version_id":"source","content_state":"PENDING","visible_content_state":"LIVE"})
    );
    let mut backup = entry(ListingVersion::File {
        content: pending(),
        backup_version: Some("stored".into()),
    });
    backup.backup = true;
    backup.read_only = Some(true);
    backup.historical_only = Some(false);
    let response = ListingEntryResponse::from(backup);
    let bytes = json_codec::to_vec(&response).unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone())
            .unwrap()
            .matches("\"version_id\":")
            .count(),
        1
    );
    let wire = storage_contract::strict_json(&bytes).unwrap();
    assert_eq!(wire["version_id"], "stored");
    assert_eq!(wire["source_version_id"], "source");
    assert_eq!(wire["own_version_id"], json!(null));
    assert_eq!(wire["historical_only"], false);
    assert_eq!(wire["read_only"], true);
}

#[test]
fn current_directory_has_context_and_empty_note_but_no_file_content_fields() {
    let mut directory = entry(ListingVersion::Directory(String::new()));
    directory.kind = storage_contract::NamespaceEntryKind::Directory;
    directory.file_id = String::new();
    directory.size = 0;
    directory.node_id = None;
    directory.read_only = None;
    directory.location.as_mut().unwrap().container = false;
    let wire = json_codec::to_value(ListingEntryResponse::from(directory)).unwrap();
    assert_eq!(
        wire,
        json!({"entry_id":"entry","name":"file","path":"Folder/file","kind":"directory","size":"0","file_id":"","version_id":"",
        "repository_id":"repo","repository_path":"file","container":false,"selected_branch_id":null,"note":""})
    );
}
