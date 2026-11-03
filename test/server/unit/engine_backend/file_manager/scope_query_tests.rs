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
    let facts = SelectedEntries::new(&selections).facts(&entries);
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
    let facts = SelectedEntries::new(&selections).facts(&entries);
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
    let facts = SelectedEntries::new(&selections(&[("shared", query)])).facts(&entries);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].entry_id, "shared");
    assert_eq!(facts[0].path, "Café/Å.txt");
    assert_eq!(facts[0].selected_path, query);
    assert!(
        SelectedEntries::new(&selections(&[("shared", "Missing/Å.txt")]))
            .facts(&entries)
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
fn prepared_selection_matches_independent_scan_across_reused_branch_queries() {
    let selections = selections(&[
        ("shared", "Cafe\u{301}/A\u{30a}.txt"),
        ("shared", "Other/Å.txt"),
        ("shared", "Missing/Å.txt"),
        ("single", "current/moved.txt"),
        ("missing", "missing.txt"),
    ]);
    let selected = SelectedEntries::new(&selections);
    let candidates = [
        entry("shared", "Café/Å.txt"),
        entry("shared", "Other/Å.txt"),
        entry("shared", "Unselected/Å.txt"),
        entry("single", "old/moved.txt"),
        entry("single", "other/moved.txt"),
        entry("unselected", "Café/Å.txt"),
        entry("unselected", "Other/Å.txt"),
    ];
    for mask in 0..1 << candidates.len() {
        let mut entries = candidates
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>();
        for _ in 0..2 {
            let expected = reference_facts(&entries, &selections);
            let actual = selected.facts(&entries);
            assert_eq!(fact_rows(&actual), fact_rows(&expected), "mask={mask}");
            entries.reverse();
        }
    }
    assert!(SelectedEntries::new(&[]).facts(&candidates).is_empty());
}

fn reference_facts(
    entries: &[storage_contract::Entry],
    selections: &[SelectedDirectoryEntry],
) -> Vec<SelectedEntryFact> {
    selections
        .iter()
        .filter_map(|selected| {
            let mut matches = entries
                .iter()
                .filter(|entry| entry.entry_id == selected.entry_id);
            let first = matches.next()?;
            let chosen = if matches.next().is_none() {
                first
            } else {
                entries.iter().rev().find(|entry| {
                    entry.entry_id == selected.entry_id && entry.lookup_key() == selected.path.key()
                })?
            };
            Some(SelectedEntryFact {
                entry_id: chosen.entry_id.clone(),
                path: chosen.path.as_str().into(),
                selected_path: selected.path.as_str().into(),
            })
        })
        .collect()
}

fn fact_rows(facts: &[SelectedEntryFact]) -> Vec<(&str, &str, &str)> {
    facts
        .iter()
        .map(|fact| {
            (
                fact.entry_id.as_str(),
                fact.path.as_str(),
                fact.selected_path.as_str(),
            )
        })
        .collect()
}
