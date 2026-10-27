//! Synthetic metadata selection regressions, not physical-file imports or a
//! complete capacity benchmark. The flat backend still scans N inputs per page;
//! only retained candidates and response size are bounded by the page limit.
use super::*;

use crate::backend_listing::DEFAULT_LIMIT;
const LIMIT: usize = 200;
fn with_snapshot(
    input: &Value,
    slot: &str,
    path: &str,
    snapshot: String,
) -> Result<PageRequest, ApiError> {
    let mut query: BTreeMap<String, String> = input
        .get("query")
        .map(|query| json_codec::from_value(query.clone()).map_err(ApiError::invalid_source))
        .transpose()?
        .unwrap_or_default();
    query.insert("path".into(), path.into());
    PageRequest::with_snapshot(crate::ReadListing::parse(slot.into(), query)?, snapshot)
}

fn request(cursor: Option<&str>, query: &str) -> PageRequest {
    let mut input = json!({"query":{"limit":LIMIT.to_string(),"query":query}});
    if let Some(cursor) = cursor {
        input["query"]["cursor"] = cursor.into();
    }
    with_snapshot(&input, "P01", "docs", "synthetic-snapshot".into()).unwrap()
}

fn key(index: usize) -> SortKey {
    let name = format!("file-{index:06}.txt");
    SortKey::new(
        false,
        &name,
        &format!("docs/{name}"),
        &format!("id-{index:06}"),
    )
}

fn select(size: usize, request: PageRequest) -> Page<usize> {
    let mut selector = PageSelector::new(request);
    let bound = selector.request.limit + 1;
    // Reverse enumeration ensures that the result is actually selected/sorted,
    // rather than merely taking a conveniently ordered input prefix.
    for index in (0..size).rev() {
        selector.consider(key(index), index);
        assert!(selector.candidates.len() <= bound);
    }
    selector.finish().unwrap()
}

#[test]
fn thousand_metadata_items_traverse_both_directions_without_gaps_or_duplicates() {
    let mut cursor = None;
    let mut forward = Vec::new();
    let last = loop {
        let page = select(1_000, request(cursor.as_deref(), ""));
        assert_eq!(page.total, 1_000);
        assert!(page.values.len() <= LIMIT);
        forward.extend_from_slice(&page.values);
        if page.next.is_none() {
            break page;
        }
        cursor = page.next;
    };
    assert_eq!(forward, (0..1_000).collect::<Vec<_>>());
    let mut backward = last.values.clone();
    cursor = last.previous;
    while let Some(current) = cursor {
        let page = select(1_000, request(Some(&current), ""));
        assert!(page.values.len() <= LIMIT);
        let mut preceding = page.values;
        preceding.extend(backward);
        backward = preceding;
        cursor = page.previous;
    }
    assert_eq!(backward, forward);
}

#[test]
fn synthetic_large_directories_bound_retained_candidates_and_response_bytes() {
    for size in [1_000, 10_000, 100_000] {
        // Representative first, middle and final positions. This is deliberately
        // not an O(N * number_of_pages) exhaustive traversal at 100k entries.
        for start in [0, size / 2, size - LIMIT] {
            let first = request(None, "");
            let cursor = (start > 0).then(|| first.cursor(&key(start - 1), false).unwrap());
            let page = select(size, request(cursor.as_deref(), ""));
            assert_eq!(page.total, size);
            assert_eq!(page.values, (start..start + LIMIT).collect::<Vec<_>>());
            assert_eq!(page.previous.is_some(), start > 0);
            assert_eq!(page.next.is_some(), start + LIMIT < size);
            // Project a realistic, fixed-width metadata fixture only AFTER
            // selecting the page, as the production listing does.
            let entries: Vec<_> = page
                .values
                .iter()
                .map(|index| {
                    json!({
                        "entry_id":format!("id-{index:06}"),
                        "name":format!("file-{index:06}.txt"),
                        "path":format!("docs/file-{index:06}.txt"),
                        "kind":"file", "size":"1024", "version_id":"fixture-version"
                    })
                })
                .collect();
            let mut response = json!({"entries":entries});
            response["total_entries"] = page.total.to_string().into();
            response["snapshot"] = page.snapshot.clone().into();
            response["previous_cursor"] = page.previous.clone().into();
            response["next_cursor"] = page.next.clone().into();
            let bytes = json_codec::to_vec(&response).unwrap().len();
            // Bound applies to this fixed-width fixture, not arbitrary path
            // lengths or all possible real-world metadata payloads.
            assert!(
                bytes <= LIMIT * 256 + 4096,
                "fixture response grew: {bytes}"
            );
            println!(
                "synthetic_metadata={size}; start={start}; scanned={size}; retained_candidates_bound={}; projected={}; response_bytes={bytes}; physical_files=0",
                LIMIT + 1,
                page.values.len()
            );
        }
    }
}

#[test]
fn filtering_precedes_paging_and_counts_all_matching_metadata() {
    let first = select(10_000, request(None, "FILE-000"));
    assert_eq!(first.total, 1_000);
    assert_eq!(first.values, (0..LIMIT).collect::<Vec<_>>());
    let next = select(10_000, request(first.next.as_deref(), "file-000"));
    assert_eq!(next.values, (LIMIT..LIMIT * 2).collect::<Vec<_>>());
    let empty = select(10_000, request(None, "absent"));
    assert_eq!(empty.total, 0);
    assert!(empty.values.is_empty());
    assert!(empty.previous.is_none() && empty.next.is_none());
}

#[test]
fn cursor_is_rejected_after_snapshot_or_request_context_changes() {
    let cursor = select(1_000, request(None, "")).next.unwrap();
    let input = json!({"query":{"limit":"200","cursor":cursor}});
    let stale = with_snapshot(&input, "P01", "docs", "different-snapshot".into())
        .err()
        .unwrap();
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
    for (slot, path) in [("P02", "docs"), ("P01", "other")] {
        assert!(with_snapshot(&input, slot, path, "synthetic-snapshot".into()).is_err());
    }
    for (field, value) in [("query", "changed"), ("limit", "100")] {
        let mut changed = input.clone();
        changed["query"][field] = value.into();
        assert!(with_snapshot(&changed, "P01", "docs", "synthetic-snapshot".into()).is_err());
    }
}

#[test]
fn malformed_cursors_limits_and_queries_are_rejected_without_panicking() {
    for cursor in [
        "".to_string(),
        "a".into(),
        "gg".into(),
        "00".into(),
        "a".repeat(MAX_CURSOR_BYTES + 2),
    ] {
        let input = json!({"query":{"cursor":cursor}});
        assert!(with_snapshot(&input, "P01", "docs", "snapshot".into()).is_err());
    }
    for limit in [
        json!("0"),
        json!("501"),
        json!("-1"),
        json!("1.5"),
        json!(200),
        json!(null),
    ] {
        assert!(
            with_snapshot(
                &json!({"query":{"limit":limit}}),
                "P01",
                "docs",
                "snapshot".into()
            )
            .is_err()
        );
    }
    assert!(
        with_snapshot(
            &json!({"query":{"query":"x".repeat(1025)}}),
            "P01",
            "docs",
            "snapshot".into()
        )
        .is_err()
    );
    assert_eq!(
        with_snapshot(&json!({}), "P01", "docs", "snapshot".into())
            .unwrap()
            .limit,
        DEFAULT_LIMIT
    );
}

#[test]
fn total_sort_key_retains_equal_folded_names_and_aliases() {
    let fixtures = [
        (false, "z", "docs/z", "z"),
        (false, "a", "docs/a", "b"),
        (true, "Z", "docs/Z", "directory"),
        (false, "A", "docs/A", "a"),
        (false, "a", "docs/alias", "b"),
        (false, "a", "docs/a", "c"),
    ];
    let mut selector = PageSelector::new(request(None, ""));
    for (index, (directory, name, path, id)) in fixtures.into_iter().enumerate().rev() {
        selector.consider(SortKey::new(directory, name, path, id), index);
    }
    let page = selector.finish().unwrap();
    assert_eq!(page.total, fixtures.len());
    assert_eq!(page.values, [2, 3, 1, 5, 4, 0]);
    assert!(page.next.is_none() && page.previous.is_none());
}

#[test]
fn admitted_entry_sort_keys_reuse_component_keys_without_changing_order_or_cursors() {
    let paths = [
        "UPPER.txt",
        "Café/Straße.txt",
        "Straße/ẞ.txt",
        "Σ/ς.txt",
        "İ/İ.txt",
        "Å/Éclair.txt",
        "가/项目.txt",
    ];
    for kind in [
        storage_contract::NamespaceEntryKind::Directory,
        storage_contract::NamespaceEntryKind::File,
    ] {
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        for path in paths {
            let entry = storage_contract::Entry {
                entry_id: format!("entry-{path}"),
                path: name_policy::PortablePath::parse(path).unwrap(),
                kind,
                file_id: "file".into(),
                version_id: "version".into(),
                size: 0,
            };
            for display in [path.to_owned(), format!("Repository/{path}")] {
                let key = SortKey::for_entry(&entry, &display);
                let original = SortKey::new(
                    kind == storage_contract::NamespaceEntryKind::Directory,
                    entry.name(),
                    &display,
                    &entry.entry_id,
                );
                assert_eq!(key, original);
                let encoded = request(None, "").cursor(&key, false).unwrap();
                let decoded = request(Some(&encoded), "");
                assert_eq!(decoded.cursor.unwrap().key, original);
                actual.push(key);
                expected.push(original);
            }
        }
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }
    assert_ne!(
        "Straße.txt".to_lowercase(),
        name_policy::lookup_key("Straße.txt")
    );
}

#[test]
fn empty_exact_and_partial_final_pages_have_consistent_boundaries() {
    for size in [0, 1, LIMIT - 1, LIMIT, LIMIT + 1, LIMIT * 2 + 1] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page = select(size, request(cursor.as_deref(), ""));
            assert_eq!(page.total, size);
            assert_eq!(page.previous.is_some(), !seen.is_empty());
            assert!(page.values.len() <= LIMIT);
            seen.extend(page.values);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
            assert!(seen.len() < size, "spurious next cursor at the end");
        }
        assert_eq!(seen, (0..size).collect::<Vec<_>>());
    }
}

#[test]
fn cursor_preserves_request_fingerprint_and_rejects_tampered_sort_components() {
    let original = request(None, "");
    let cursor = original.cursor(&key(10), false).unwrap();
    let bytes = cursor
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    let mut parsed: Value = json_codec::from_slice(&bytes).unwrap();
    let request_digest = blake3::hash(&json_codec::to_vec(&json!({"slot":"P01","path":"docs","query":"","limit":200,"order":"directory-lowercase-normalized-name-path-identity","unicode":name_policy::UNICODE_VERSION,"lowercase_unicode":std::char::UNICODE_VERSION})).unwrap()).to_hex().to_string();
    assert_eq!(parsed["request"], request_digest);
    parsed["key"]["folded"] = "tampered".into();
    let tampered = json_codec::to_vec(&parsed)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let input = json!({"query":{"cursor":tampered}});
    let error = with_snapshot(&input, "P01", "docs", "synthetic-snapshot".into())
        .err()
        .unwrap();
    assert_eq!(error.message, "cursor sort key is invalid");
    let stale = with_snapshot(&input, "P01", "docs", "new-snapshot".into())
        .err()
        .unwrap();
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
}
