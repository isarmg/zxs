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
    let facts = selected_entry_facts(&entries, &selections);
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
    let facts = selected_entry_facts(&entries, &selections);
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
    let facts = selected_entry_facts(&entries, &selections(&[("shared", query)]));
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].entry_id, "shared");
    assert_eq!(facts[0].path, "Café/Å.txt");
    assert_eq!(facts[0].selected_path, query);
    assert!(selected_entry_facts(&entries, &selections(&[("shared", "Missing/Å.txt")])).is_empty());
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
