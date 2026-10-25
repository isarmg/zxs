use super::*;
fn entry(id: &str, path: &str) -> storage_contract::Entry {
    storage_contract::Entry {
        entry_id: id.into(),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        lookup_key: name_policy::path_key(path),
        kind: "file".into(),
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
    let selections = BTreeSet::from([
        ("shared", "folder/a.txt"),
        ("shared", "folder/missing.txt"),
        ("single", "folder/current.txt"),
    ]);
    let facts = selected_entry_facts(&entries, &selections);
    assert_eq!(
        facts,
        json!([
            {"entry_id":"shared","path":"folder/a.txt","selected_path":"folder/a.txt"},
            {"entry_id":"single","path":"old/moved.txt","selected_path":"folder/current.txt"}
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}
#[test]
fn unselected_duplicate_aliases_do_not_become_a_single_identity() {
    let entries = (0..10_000)
        .map(|index| entry("shared", &format!("folder/file{index}.txt")))
        .collect::<Vec<_>>();
    let selections = BTreeSet::from([
        ("shared", "folder/missing.txt"),
        ("shared", "folder/file9.txt"),
    ]);
    let facts = selected_entry_facts(&entries, &selections);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0]["selected_path"], "folder/file9.txt");
}

#[test]
fn selected_alias_facts_match_canonical_keys_without_losing_display_or_request_paths() {
    let entries = vec![
        entry("shared", "Café/Å.txt"),
        entry("shared", "Other/Å.txt"),
    ];
    let query = "Cafe\u{301}/A\u{30a}.txt";
    let facts = selected_entry_facts(&entries, &BTreeSet::from([("shared", query)]));
    assert_eq!(
        facts,
        vec![json!({"entry_id":"shared","path":"Café/Å.txt","selected_path":query})]
    );
    assert!(
        selected_entry_facts(&entries, &BTreeSet::from([("shared", "Missing/Å.txt")])).is_empty()
    );
}
