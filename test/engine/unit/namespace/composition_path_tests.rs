//! A typed request path is not proof for a path composed with different
//! stored ancestor spelling. These tests exercise the actual mutation boundary.
use super::*;

fn aliased_destination(leaf_units: usize) -> String {
    format!("Straße/{}", "x".repeat(leaf_units))
}

#[test]
fn file_and_directory_upserts_reject_invalid_composed_paths_before_sealing() {
    let entries = [directory("STRASSE", 100_001)];
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    for directory_entry in [false, true] {
        let mut editor = NamespaceEditor::open(&root, &source).unwrap();
        let requested = aliased_destination(193);
        let error = if directory_entry {
            editor
                .upsert_directory(&directory(&requested, 1))
                .unwrap_err()
        } else {
            editor.upsert_file(&file(&requested, 1)).unwrap_err()
        };
        assert_eq!(error.code(), ErrorCode::PathInvalid);
        assert!(
            editor
                .entry_kind(&name_policy::path_key(&requested))
                .unwrap()
                .is_none()
        );
        let mut fresh = NamespaceObjects::new();
        assert_eq!(editor.seal(&mut fresh).unwrap(), root);
        assert_eq!(fresh.encoding_work(), (0, 0));
    }
}

#[test]
fn file_rename_rejects_invalid_composed_path_without_removing_the_source() {
    let entries = [directory("STRASSE", 100_001), file("source", 1)];
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let requested = aliased_destination(193);
    let mut editor = NamespaceEditor::open(&root, &source).unwrap();
    let operation = NamespaceOperation::parse(
        storage_contract::NamespaceOperationKind::Rename,
        "source",
        Some(&requested),
    )
    .unwrap();
    let error = editor.apply(&operation).unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert_eq!(
        editor.entry_kind("source").unwrap(),
        Some(NamespaceEntryKind::File)
    );
    let mut fresh = NamespaceObjects::new();
    assert_eq!(editor.seal(&mut fresh).unwrap(), root);
    assert_eq!(fresh.encoding_work(), (0, 0));
}

#[test]
fn aliased_parent_composition_accepts_the_exact_actual_utf16_limit() {
    let entries = [directory("STRASSE", 100_001)];
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let requested = file(&aliased_destination(192), 1);
    let mut editor = NamespaceEditor::open(&root, &source).unwrap();
    editor.upsert_file(&requested).unwrap();
    let mut fresh = NamespaceObjects::new();
    let edited = editor.seal(&mut fresh).unwrap();
    let source = DraftSource {
        fresh: &fresh,
        base: &source,
    };
    let actual = NamespaceView::new(&edited, &source)
        .resolve_key(requested.lookup_key())
        .unwrap()
        .unwrap();
    assert_eq!(actual.path(), format!("STRASSE/{}", "x".repeat(192)));
    assert_eq!(actual.path().encode_utf16().count(), 200);
    assert_eq!(verify_namespace(&edited, &source, 2).unwrap(), 2);
}

#[test]
fn full_builder_checks_the_same_stored_parent_composition_boundary() {
    for directory_entry in [false, true] {
        let requested = aliased_destination(193);
        let child = if directory_entry {
            directory(&requested, 1)
        } else {
            file(&requested, 1)
        };
        let entries = [directory("STRASSE", 100_001), child];
        let mut objects = NamespaceObjects::new();
        let error = build_from_entries(&entries, &mut objects).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PathInvalid);
        assert_eq!(objects.encoding_work(), (0, 0));
    }
}

#[test]
fn reregistering_a_directory_keeps_its_existing_name_without_composing_a_new_path() {
    let stored_parent = format!("{}STRASSE", "a".repeat(186));
    let requested_parent = format!("{}Straße", "a".repeat(186));
    let stored = format!("{stored_parent}/Straße");
    let requested = format!("{requested_parent}/STRASSE");
    assert_eq!(stored.encode_utf16().count(), 200);
    assert_eq!(requested.encode_utf16().count(), 200);
    let entries = [directory(&stored_parent, 100_001), directory(&stored, 1)];
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let mut editor = NamespaceEditor::open(&root, &source).unwrap();
    editor.upsert_directory(&directory(&requested, 2)).unwrap();
    let mut fresh = NamespaceObjects::new();
    assert_eq!(editor.seal(&mut fresh).unwrap(), root);
    assert_eq!(fresh.encoding_work(), (0, 0));
}
