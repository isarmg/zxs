use super::*;

fn assert_canonical(value: &Value, expected: &[u8]) {
    assert_eq!(baseline::canonical(value).unwrap(), expected);
    assert_eq!(canonical_json_value(value).unwrap(), expected);
    assert_eq!(canonical_json(value).unwrap(), expected);
    for domain in ["state-page", "request", "version"] {
        let mut digest = blake3::Hasher::new();
        digest.update(format!("zxs:{domain}\0").as_bytes());
        digest.update(expected);
        let hash = digest.finalize().to_hex().to_string();
        assert_eq!(
            encode_hashed_value(domain, value).unwrap(),
            (expected.to_vec(), hash.clone())
        );
        assert_eq!(
            encode_hashed_object(domain, value).unwrap(),
            (expected.to_vec(), hash)
        );
    }
}

#[test]
fn ascii_keys_keep_control_and_quote_escaping_in_fixed_order() {
    let keys = ["z", "a", "\\", "\"", "\n", "\0", ""];
    let object = keys
        .into_iter()
        .map(|key| (key.to_owned(), Value::from(1)))
        .collect();
    assert_canonical(
        &Value::Object(object),
        br#"{"":1,"\u0000":1,"\n":1,"\"":1,"\\":1,"a":1,"z":1}"#,
    );
    assert_canonical(&json_codec::json!({}), b"{}");
    assert_canonical(&json_codec::json!({"only": {}}), br#"{"only":{}}"#);
}

#[test]
fn object_shortcut_follows_actual_map_order_across_feature_unification() {
    // BTreeMap sorts every insertion order; preserve_order uses IndexMap.
    // Exercise both the already-sorted shortcut and its insertion-order fallback
    // without assuming dependency cfg flags are visible in storage-contract.
    let keys = ["", "\0", "\n", "\"", "\\", "a", "z", "\u{7f}"];
    for length in 0..=keys.len() {
        for rotation in 0..keys.len() {
            for reverse in [false, true] {
                let mut insertion = keys[..length].to_vec();
                if length > 0 {
                    insertion.rotate_left(rotation % length);
                }
                if reverse {
                    insertion.reverse();
                }
                let object = insertion
                    .into_iter()
                    .map(|key| {
                        (
                            key.to_owned(),
                            json_codec::json!([key, {"z": 1, "a": true}]),
                        )
                    })
                    .collect::<Map<_, _>>();
                assert_eq!(ascii_keys_are_ordered(&object), object.keys().is_sorted());
                let value = Value::Object(object);
                let expected = baseline::canonical(&value).unwrap();
                assert_canonical(&value, &expected);
            }
        }
    }
}

#[test]
fn mixed_unicode_always_retains_utf16_order_not_utf8_order() {
    let keys = ["\u{e000}", "\u{10ffff}", "\u{10000}", "\u{d7ff}", "a"];
    let expected = concat!(
        "{\"a\":0,\"\u{d7ff}\":0,\"\u{10000}\":0,",
        "\"\u{10ffff}\":0,\"\u{e000}\":0}"
    );
    for rotation in 0..keys.len() {
        let mut insertion = keys;
        insertion.rotate_left(rotation);
        let object = insertion
            .into_iter()
            .map(|key| (key.to_owned(), Value::from(0)))
            .collect::<Map<_, _>>();
        assert!(!ascii_keys_are_ordered(&object));
        assert_canonical(&Value::Object(object), expected.as_bytes());
    }
    assert_canonical(
        &json_codec::json!({"\u{e000}": 0}),
        "{\"\u{e000}\":0}".as_bytes(),
    );
}

#[test]
fn object_shortcut_does_not_admit_invalid_numbers_or_json() {
    for value in [
        json_codec::json!({"a": 1.0}),
        json_codec::json!({"a": [9_007_199_254_740_992u64]}),
        json_codec::json!({"a": {"z": -9_007_199_254_740_992i64}}),
    ] {
        let expected = baseline::canonical(&value).unwrap_err();
        for error in [
            canonical_json_value(&value).unwrap_err(),
            canonical_json(&value).unwrap_err(),
        ] {
            assert_eq!(error.code(), expected.code());
            assert_eq!(error.message(), expected.message());
        }
    }
    for input in [
        br#"{"a":1,"\u0061":2}"#.as_slice(),
        br#"{"a":{"b":1,"b":2}}"#,
        br#"{"a":"\uD800"}"#,
        br#"{"a":"\x00"}"#,
    ] {
        assert!(strict_json(input).is_err());
    }
}
