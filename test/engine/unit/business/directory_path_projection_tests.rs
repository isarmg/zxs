//! The old relocation and sorted-range algorithms are frozen test-only oracles.
use super::*;

fn original_rebase_entries(entries: &[Entry], from: &str, to: &str) -> Result<Vec<Entry>> {
    let depth = if from.is_empty() {
        0
    } else {
        from.split('/').count()
    };
    let target = if to.is_empty() {
        None
    } else {
        Some(name_policy::PortablePath::parse(to)?)
    };
    entries
        .iter()
        .map(|entry| {
            let path = if let Some(target) = &target {
                entry.location().rebase(depth, target)?
            } else {
                entry.location().strip_components(depth).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "directory snapshot contains its own root",
                    )
                })?
            };
            let mut entry = entry.clone();
            entry.relocate(path);
            Ok(entry)
        })
        .collect()
}

fn original_descendants<'a>(entries: &'a [Entry], key: &str) -> &'a [Entry] {
    if key.is_empty() {
        return entries;
    }
    let prefix = format!("{key}/");
    let start = entries.partition_point(|entry| entry.lookup_key() < prefix.as_str());
    let descendants = &entries[start..];
    let end = descendants.partition_point(|entry| entry.lookup_key().starts_with(&prefix));
    &descendants[..end]
}

fn entry(path: &str, ordinal: usize) -> Entry {
    Entry {
        entry_id: format!("entry-{ordinal}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: if ordinal.is_multiple_of(2) {
            NamespaceEntryKind::File
        } else {
            NamespaceEntryKind::Directory
        },
        file_id: format!("file-{ordinal}"),
        version_id: format!("version-{ordinal}"),
        size: ordinal as u64,
    }
}

fn result(value: Result<Vec<Entry>>) -> std::result::Result<Vec<u8>, (ErrorCode, String)> {
    value
        .map(|entries| canonical_json(&entries).unwrap())
        .map_err(|error| (error.code(), error.to_string()))
}

#[test]
fn shared_rebase_keeps_payload_order_root_errors_and_destination_admission() {
    let entries = [
        "Root/A",
        "Root/É/File",
        "Root/Straße",
        "root/ss",
        "Other/Leaf",
        "Rooted/Neighbor",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, path)| entry(path, index))
    .collect::<Vec<_>>();
    let before = canonical_json(&entries).unwrap();
    for start in 0..=entries.len() {
        for end in start..=entries.len() {
            for from in ["", "Root", "ROOT", "a/b", "a/b/c", "a/b/c/d", "root/"] {
                for to in [
                    "",
                    "Else",
                    "É/Target",
                    "STRASSE",
                    "x/../y",
                    "bad\\name",
                    "..",
                ] {
                    let input = &entries[start..end];
                    assert_eq!(
                        result(rebase_entries(input, from, to)),
                        result(original_rebase_entries(input, from, to)),
                        "input={start}..{end}, from={from:?}, to={to:?}",
                    );
                }
            }
        }
    }
    assert_eq!(canonical_json(&entries).unwrap(), before);
}

#[test]
fn shared_sorted_range_borrows_exact_original_slice_and_keeps_prefix_boundaries() {
    let mut entries = [
        "Root/A",
        "Root/É/File",
        "Root/Straße",
        "root/ss",
        "Other/Leaf",
        "Rooted/Neighbor",
        "root",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, path)| entry(path, index))
    .collect::<Vec<_>>();
    namespace::sorted(&mut entries);
    for start in 0..=entries.len() {
        for end in start..=entries.len() {
            let input = &entries[start..end];
            for key in [
                "",
                "root",
                "ROOT",
                "root/é",
                "root/s",
                "root/strasse",
                "rooted",
                "missing",
                "r",
                "root/",
            ] {
                let actual = namespace::sorted_descendants(input, key);
                let original = original_descendants(input, key);
                assert!(
                    std::ptr::eq(actual, original),
                    "slice={start}..{end}, key={key:?}"
                );
                let expected = input
                    .iter()
                    .filter(|entry| {
                        key.is_empty() || namespace::is_descendant(entry.lookup_key(), key)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(actual.iter().collect::<Vec<_>>(), expected);
            }
        }
    }
}
