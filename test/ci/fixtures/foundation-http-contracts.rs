// Public HTTP constructor, iteration and reservation contract regressions.
mod http_public_contract_regressions {
    use http::HeaderMap;
    use http::header::{ACCEPT, CONTENT_LENGTH, HOST, HeaderName};
    use std::iter;
    use std::panic::{self, AssertUnwindSafe};

    const VALID_TCHAR_NAME: &str = "x!#$%&'*+-.^_`|~012abc";
    const VALID_LONG_LOWER: &str = concat!(
        "abcdefghijklmnop",
        "abcdefghijklmnop",
        "abcdefghijklmnop",
        "abcdefghijklmnop",
        "z"
    );
    const VALID_LONG_MIXED: &str = concat!(
        "AbCdEfGhIjKlMnOp",
        "AbCdEfGhIjKlMnOp",
        "AbCdEfGhIjKlMnOp",
        "AbCdEfGhIjKlMnOp",
        "Z"
    );
    const INVALID_LONG_NAMES: [&str; 6] = [
        concat!(
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            ":"
        ),
        concat!(
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            ":",
            "abcdefghijklmnop",
            "abcdefghijklmnop"
        ),
        concat!(
            ":",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop"
        ),
        concat!(
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "\0"
        ),
        concat!(
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "\""
        ),
        concat!(
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "abcdefghijklmnop",
            "é"
        ),
    ];

    #[test]
    fn lowercase_name_rejects_double_quote() {
        for name in ["\"", "x\"name", "name\""] {
            assert!(HeaderName::from_bytes(name.as_bytes()).is_err());
            assert!(HeaderName::from_lowercase(name.as_bytes()).is_err());
        }
    }

    #[test]
    fn static_name_rejects_double_quote() {
        for name in ["\"", "x\"name", "name\""] {
            assert!(panic::catch_unwind(|| HeaderName::from_static(name)).is_err());
        }
    }

    #[test]
    fn valid_name_constructors_preserve_token_punctuation() {
        let parsed = HeaderName::from_bytes(VALID_TCHAR_NAME.as_bytes())
            .expect("invariant: fixture contains only lowercase tchar bytes");
        let lowercase = HeaderName::from_lowercase(VALID_TCHAR_NAME.as_bytes())
            .expect("invariant: fixture contains only lowercase tchar bytes");
        assert_eq!(parsed, lowercase);
        assert_eq!(parsed, HeaderName::from_static(VALID_TCHAR_NAME));
        assert!(HeaderName::from_lowercase(b"X-Name").is_err());
        assert_eq!(
            HeaderName::from_bytes(b"X-Name")
                .expect("invariant: mixed-case field names are valid through from_bytes"),
            HeaderName::from_static("x-name")
        );
    }

    #[test]
    fn borrowed_long_invalid_names_return_error_before_entry() {
        // Fixed literals cross the 64-byte scratch boundary; no generated large input.
        for name in INVALID_LONG_NAMES {
            assert!(name.len() == 65 || name.len() == 66);
            assert!(HeaderName::from_bytes(name.as_bytes()).is_err());
            let mut map = HeaderMap::<usize>::with_capacity(4);
            map.insert(HOST, 7);
            // try_entry uses AsHeaderName's borrowed &str path even for a static literal.
            let borrowed: &str = name;
            assert!(map.try_entry(borrowed).is_err());
            assert_eq!(map.len(), 1);
            assert_eq!(map.get(HOST), Some(&7));
        }
        let mut map = HeaderMap::<usize>::default();
        assert!(map.try_entry("short:name").is_err());
    }

    #[test]
    fn static_long_invalid_names_panic_before_try_insert() {
        for name in INVALID_LONG_NAMES {
            let mut map = HeaderMap::<usize>::with_capacity(4);
            map.insert(HOST, 7);
            let result = panic::catch_unwind(AssertUnwindSafe(|| map.try_insert(name, 9)));
            assert!(result.is_err());
            assert_eq!(map.len(), 1);
            assert_eq!(map.get(HOST), Some(&7));
        }
    }

    #[test]
    fn static_long_invalid_names_panic_before_try_append() {
        for name in INVALID_LONG_NAMES {
            let mut map = HeaderMap::<usize>::with_capacity(4);
            map.insert(HOST, 7);
            let result = panic::catch_unwind(AssertUnwindSafe(|| map.try_append(name, 9)));
            assert!(result.is_err());
            assert_eq!(map.len(), 1);
            assert_eq!(map.get(HOST), Some(&7));
        }
    }

    #[test]
    fn static_long_invalid_names_panic_before_entry() {
        for name in INVALID_LONG_NAMES {
            let mut map = HeaderMap::<usize>::with_capacity(4);
            map.insert(HOST, 7);
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let _ = map.entry(name);
            }));
            assert!(result.is_err());
            assert_eq!(map.len(), 1);
            assert_eq!(map.get(HOST), Some(&7));
        }
    }

    #[test]
    fn valid_long_name_paths_preserve_normalization_and_lookup() {
        assert_eq!(VALID_LONG_LOWER.len(), 65);
        assert_eq!(VALID_LONG_MIXED.len(), 65);
        let typed = HeaderName::from_bytes(VALID_LONG_MIXED.as_bytes())
            .expect("invariant: long mixed-case fixture contains only tchar bytes");
        assert_eq!(typed.as_str(), VALID_LONG_LOWER);
        let mut map = HeaderMap::<usize>::default();
        map.insert(VALID_LONG_MIXED, 11);
        assert_eq!(map.get(VALID_LONG_LOWER), Some(&11));
        assert_eq!(map.get(&typed), Some(&11));
        assert_eq!(map.keys().next(), Some(&typed));
        *map.try_entry(VALID_LONG_LOWER)
            .expect("invariant: long lowercase fixture is a valid field name")
            .or_insert(0) += 1;
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&typed), Some(&12));
    }

    fn maps_with_known_lengths() -> [(HeaderMap<usize>, usize); 5] {
        let empty = HeaderMap::default();
        let mut single = HeaderMap::default();
        single.insert(HOST, 1);
        let mut same_key = HeaderMap::default();
        same_key.append(HOST, 1);
        same_key.append(HOST, 2);
        same_key.append(HOST, 3);
        let mut distinct = HeaderMap::default();
        distinct.insert(HOST, 1);
        distinct.insert(ACCEPT, 2);
        distinct.insert(CONTENT_LENGTH, 3);
        let mut mixed = HeaderMap::default();
        mixed.append(HOST, 1);
        mixed.append(HOST, 2);
        mixed.insert(ACCEPT, 3);
        mixed.insert(CONTENT_LENGTH, 4);
        [
            (empty, 0),
            (single, 1),
            (same_key, 3),
            (distinct, 3),
            (mixed, 4),
        ]
    }

    fn assert_hint_bounds<I: Iterator>(iter: &I, remaining: usize) {
        let (lower, upper) = iter.size_hint();
        assert!(
            lower <= remaining,
            "lower bound {} exceeds remaining {}",
            lower,
            remaining
        );
        if let Some(upper) = upper {
            assert!(
                remaining <= upper,
                "upper bound {} is below remaining {}",
                upper,
                remaining
            );
        }
    }

    fn assert_all_remaining_hints<I: Iterator>(mut iter: I, expected_len: usize) {
        assert_hint_bounds(&iter, expected_len);
        for remaining in (0..expected_len).rev() {
            assert!(iter.next().is_some());
            assert_hint_bounds(&iter, remaining);
        }
        assert!(iter.next().is_none());
        assert_hint_bounds(&iter, 0);
        // These four http iterators also expose FusedIterator semantics.
        assert!(iter.next().is_none());
        assert_hint_bounds(&iter, 0);
    }

    #[test]
    fn iter_size_hint_bounds_follow_remaining_values() {
        for (map, expected_len) in maps_with_known_lengths() {
            assert_all_remaining_hints(map.iter(), expected_len);
        }
    }

    #[test]
    fn iter_mut_size_hint_bounds_follow_remaining_values() {
        for (mut map, expected_len) in maps_with_known_lengths() {
            assert_all_remaining_hints(map.iter_mut(), expected_len);
        }
    }

    #[test]
    fn values_size_hint_bounds_follow_remaining_values() {
        for (map, expected_len) in maps_with_known_lengths() {
            assert_all_remaining_hints(map.values(), expected_len);
        }
    }

    #[test]
    fn values_mut_size_hint_bounds_follow_remaining_values() {
        for (mut map, expected_len) in maps_with_known_lengths() {
            assert_all_remaining_hints(map.values_mut(), expected_len);
        }
    }

    #[derive(Debug)]
    struct StopInfiniteRepeat;

    struct StopOnClone(u8);

    impl Clone for StopOnClone {
        fn clone(&self) -> Self {
            // Repeat has a genuine infinite lower bound; terminate before its first item.
            panic::panic_any(StopInfiniteRepeat)
        }
    }

    fn expect_repeat_stop(action: impl FnOnce()) {
        match panic::catch_unwind(AssertUnwindSafe(action)) {
            Err(payload) if payload.is::<StopInfiniteRepeat>() => {}
            Err(payload) => panic::resume_unwind(payload),
            Ok(()) => panic!("expected the first Repeat clone to stop the test"),
        }
    }

    #[test]
    fn extend_named_entries_handles_maximum_infinite_lower_bound() {
        let mut map = HeaderMap::default();
        map.insert(HOST, StopOnClone(7));
        let entries = iter::repeat((ACCEPT, StopOnClone(9)));
        assert_eq!(entries.size_hint(), (usize::MAX, None));
        expect_repeat_stop(|| map.extend(entries));
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(HOST)
                .expect("invariant: original header was not consumed")
                .0,
            7
        );
    }

    #[test]
    fn extend_optional_entries_handles_maximum_infinite_lower_bound() {
        let mut map = HeaderMap::default();
        map.insert(HOST, StopOnClone(7));
        let entries = iter::repeat((Some(ACCEPT), StopOnClone(9)));
        assert_eq!(entries.size_hint(), (usize::MAX, None));
        expect_repeat_stop(|| map.extend(entries));
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(HOST)
                .expect("invariant: original header was not consumed")
                .0,
            7
        );
    }
}
