use super::*;
fn entry(id: &str, path: &str) -> storage_contract::Entry {
    storage_contract::Entry {
        entry_id: id.into(),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: storage_contract::NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: "version".into(),
        size: 1,
    }
}
#[test]
fn selected_facts_preserve_original_path_association_and_ambiguous_alias_absence() {
    let entries = vec![
        entry("shared", "folder/a.txt"),
        entry("shared", "folder/b.txt"),
        entry("single", "old/moved.txt"),
    ];
    let selections = selections(&[
        ("shared", "folder/a.txt"),
        ("shared", "folder/missing.txt"),
        ("single", "folder/current.txt"),
    ]);
    let (count, facts) =
        selected_entry_facts(&entries, &selections, &crate::tests::history_read_control()).unwrap();
    assert_eq!(count, entries.len());
    assert_eq!(
        facts
            .iter()
            .map(|fact| (
                fact.entry_id.as_str(),
                fact.path.as_str(),
                fact.selected_path.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("shared", "folder/a.txt", "folder/a.txt"),
            ("single", "old/moved.txt", "folder/current.txt")
        ]
    );
}
#[test]
fn unselected_duplicate_aliases_do_not_become_a_single_identity() {
    let entries = (0..10_000)
        .map(|index| entry("shared", &format!("folder/file{index}.txt")))
        .collect::<Vec<_>>();
    let selections = selections(&[
        ("shared", "folder/missing.txt"),
        ("shared", "folder/file9.txt"),
    ]);
    let (count, facts) =
        selected_entry_facts(&entries, &selections, &crate::tests::history_read_control()).unwrap();
    assert_eq!(count, entries.len());
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].selected_path, "folder/file9.txt");
}

#[test]
fn selected_alias_facts_match_canonical_keys_without_losing_display_or_request_paths() {
    let entries = vec![
        entry("shared", "Café/Å.txt"),
        entry("shared", "Other/Å.txt"),
    ];
    let query = "Cafe\u{301}/A\u{30a}.txt";
    let (_, facts) = selected_entry_facts(
        &entries,
        &selections(&[("shared", query)]),
        &crate::tests::history_read_control(),
    )
    .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].entry_id, "shared");
    assert_eq!(facts[0].path, "Café/Å.txt");
    assert_eq!(facts[0].selected_path, query);
    assert!(
        selected_entry_facts(
            &entries,
            &selections(&[("shared", "Missing/Å.txt")]),
            &crate::tests::history_read_control()
        )
        .unwrap()
        .1
        .is_empty()
    );
}

fn selections(items: &[(&str, &str)]) -> Vec<SelectedDirectoryEntry> {
    let mut selected = items
        .iter()
        .map(|(id, path)| SelectedDirectoryEntry {
            entry_id: (*id).into(),
            path: name_policy::LookupPath::parse(path).unwrap(),
        })
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        (&left.entry_id, left.path.as_str()).cmp(&(&right.entry_id, right.path.as_str()))
    });
    selected
}

#[test]
fn selected_scan_counts_all_files_but_retains_only_requested_facts() {
    let mut directory = entry("directory", "folder/subdir");
    directory.kind = storage_contract::NamespaceEntryKind::Directory;
    let entries = vec![
        entry("wanted", "folder/wanted.txt"),
        entry("other", "folder/other.txt"),
        directory,
    ];
    let (count, facts) = selected_entry_facts(
        &entries,
        &selections(&[("wanted", "folder/wanted.txt")]),
        &crate::tests::history_read_control(),
    )
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].entry_id, "wanted");
    let (count, facts) =
        selected_entry_facts(&entries, &[], &crate::tests::history_read_control()).unwrap();
    assert_eq!(count, 2);
    assert!(facts.is_empty());
}

#[test]
fn cancelled_source_scan_does_not_return_partial_facts() {
    let control = crate::tests::history_read_control();
    drop(control.wait_owner());
    let error = selected_entry_facts(
        &[entry("wanted", "folder/wanted.txt")],
        &selections(&[("wanted", "folder/wanted.txt")]),
        &control,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::RequestTimeout);
    assert_eq!(error.status, ::http::StatusCode::REQUEST_TIMEOUT);
}
