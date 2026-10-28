use super::*;

#[test]
fn named_wire_records_require_objects_at_every_nesting_level() {
    let merge = json_codec::json!({"branch_id":"branch", "expected_node_id":"node"});
    assert!(decode_strict::<crate::FileMergeInput>(&json_codec::to_vec(&merge).unwrap()).is_ok());
    assert!(decode_strict::<crate::FileMergeInput>(br#"["branch","node"]"#).is_err());
    assert!(
        json_codec::from_value::<Vec<crate::FileMergeInput>>(json_codec::json!([[
            "branch", "node"
        ]]))
        .is_err()
    );
    assert!(
        json_codec::from_value::<Vec<crate::FileMergeInput>>(json_codec::json!([merge])).is_ok()
    );

    let source = json_codec::json!({
        "directory_branch_id":"branch", "expected_tree_id":"tree",
        "path":"file", "entry_id":"entry"
    });
    let valid = json_codec::json!({
        "kind":"delete", "items":[{"source":source, "target":null}]
    });
    assert!(json_codec::from_value::<crate::DirectoryNamespaceBatch>(valid.clone()).is_ok());
    let mut invalid = valid;
    invalid["items"][0]["source"] = json_codec::json!(["branch", "tree", "file", "entry"]);
    assert!(json_codec::from_value::<crate::DirectoryNamespaceBatch>(invalid).is_err());
}

#[test]
fn request_digest_preserves_envelope_and_borrowed_body() {
    let payload = json_codec::json!({
        "message": "目录 😀",
        "file_placement": {
            "文件": {"primary_slot_id": "slot-1", "target_copies": 2}
        },
        "nested": {"request_digest": "keep nested digest"}
    });
    let path = json_codec::json!({"workspace": "workspace-1"});
    let query = json_codec::json!({"label": "é"});
    let expected = object_hash(
        "request",
        &json_codec::json!({
            "method": "POST",
            "route": "/api/workspaces/{workspace}/commit",
            "path_parameters": path,
            "query": query,
            "body": payload
        }),
    )
    .unwrap();
    let mut body = payload;
    body["request_digest"] = "ignore top-level digest".into();
    let original = body.clone();
    assert_eq!(
        request_digest(
            "pOsT",
            "/api/workspaces/{workspace}/commit",
            path,
            query,
            &body,
        )
        .unwrap(),
        expected
    );
    assert_eq!(body, original);
}

#[test]
fn encoded_object_preserves_canonical_bytes_and_domain_digest() {
    let value = json_codec::json!({"z": [true, null], "a": 1});
    let (bytes, hash) = encode_hashed_object("state-page", &value).unwrap();
    assert_eq!(bytes, br#"{"a":1,"z":[true,null]}"#);
    let mut expected = blake3::Hasher::new();
    expected.update(b"zxs:state-page\0");
    expected.update(br#"{"a":1,"z":[true,null]}"#);
    assert_eq!(hash, expected.finalize().to_hex().as_str());
    assert_eq!(object_hash("state-page", &value).unwrap(), hash);
    assert_ne!(object_hash("snapshot", &value).unwrap(), hash);
    assert_eq!(
        encode_hashed_object("Invalid", &value).unwrap_err().code(),
        crate::ErrorCode::InvalidObjectType
    );
    assert!(encode_hashed_object("state-page", &json_codec::json!({"n": u64::MAX})).is_err());
}
#[test]
fn published_vectors() {
    let v: Value =
        json_codec::from_str(include_str!("../../../../docs/contracts/vectors.json")).unwrap();
    for c in v["canonical"].as_array().unwrap() {
        let bytes = canonical_json(&c["value"]).unwrap();
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex, c["canonical_utf8_hex"]);
        let prefix = c["prefix_hex"].as_str().unwrap();
        let purpose = match c["name"].as_str().unwrap() {
            "version-jcs" => "version",
            "request-jcs" => "request",
            "utf16-key-order" => "vector",
            name => panic!("unknown canonical vector {name}"),
        };
        let expected_prefix = format!("zxs:{purpose}\0")
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(prefix, expected_prefix);
        assert_eq!(object_hash(purpose, &c["value"]).unwrap(), c["blake3"]);
        let mut h = blake3::Hasher::new();
        for i in (0..prefix.len()).step_by(2) {
            h.update(&[u8::from_str_radix(&prefix[i..i + 2], 16).unwrap()]);
        }
        h.update(&bytes);
        assert_eq!(
            h.finalize().to_hex().as_str(),
            c["blake3"].as_str().unwrap()
        );
    }
    for c in v["strict_json_negative"].as_array().unwrap() {
        assert!(
            strict_json(c["text"].as_str().unwrap().as_bytes()).is_err(),
            "{}",
            c["name"]
        );
    }
    for c in v["binary"].as_array().unwrap() {
        let s = c["manifest_hex"].as_str().unwrap();
        let b = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ChecksumManifest::decode(&b).unwrap().encode().unwrap(), b);
        assert_eq!(
            blake3::hash(&b).to_hex().as_str(),
            c["manifest_hash"].as_str().unwrap()
        );
    }
}
#[test]
fn utf16_order() {
    let v = json_codec::json!({"\u{e000}":1,"\u{10000}":2});
    assert_eq!(
        String::from_utf8(canonical_json(&v).unwrap()).unwrap(),
        "{\"𐀀\":2,\"\":1}"
    );
}

#[test]
fn canonical_strings_preserve_escaping_unicode_and_long_values() {
    let text = "\0\u{1}\u{8}\u{b}\u{c}\u{1f}\n\r\t\"\\中文𐀀";
    let value = json_codec::json!({
        "\u{e000}": "bmp",
        "\u{10000}": "astral",
        "long": "long/".repeat(40),
        "array": [text, {"quote\"\\\n": text}],
    });
    let expected = format!(
        concat!(
            r#"{{"array":["\u0000\u0001\b\u000b\f\u001f\n\r\t\"\\中文𐀀","#,
            r#"{{"quote\"\\\n":"\u0000\u0001\b\u000b\f\u001f\n\r\t\"\\中文𐀀"}}],"#,
            r#""long":"{}","𐀀":"astral","":"bmp"}}"#,
        ),
        "long/".repeat(40),
    );
    assert_eq!(canonical_json(&value).unwrap(), expected.as_bytes());
    let (bytes, hash) = encode_hashed_object("string-escaping", &value).unwrap();
    assert_eq!(bytes, expected.as_bytes());
    assert_eq!(
        hash,
        "177d51ad5c965742b9f2d657453d1acf861863bec86cc2c43e89d631081231c4"
    );
}

#[test]
fn canonical_json_rejects_float_types_and_unsafe_numeric_integers() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0, 1.5] {
        assert!(canonical_json(&value).is_err());
        assert!(canonical_json(&vec![Some(value)]).is_err());
    }
    assert!(canonical_json(&u64::MAX).is_err());
    assert!(canonical_json(&i64::MIN).is_err());
    assert_eq!(
        canonical_json(&((1u64 << 53) - 1)).unwrap(),
        b"9007199254740991"
    );
    assert_eq!(
        canonical_json(&(-9_007_199_254_740_991i64)).unwrap(),
        b"-9007199254740991"
    );
    assert!(canonical_json(&json_codec::json!({"native_size": u64::MAX})).is_err());
}

#[test]
fn checksum_header_is_semantic_and_rejects_tagged_or_truncated_inputs() {
    let manifest = ChecksumManifest {
        chunk_bytes: 65_536,
        logical_size: 3,
        chunks: vec![*blake3::hash(b"abc").as_bytes()],
    };
    let bytes = manifest.encode().unwrap();
    assert_eq!(bytes.len(), CHECKSUM_HEADER_BYTES + 32);
    assert_eq!(&bytes[..8], b"ZXSCHKS\0");
    assert_eq!(&bytes[8..10], &1u16.to_le_bytes());
    assert_eq!(&bytes[10..14], &65_536u32.to_le_bytes());
    assert_eq!(&bytes[14..22], &3u64.to_le_bytes());
    assert_eq!(&bytes[22..30], &1u64.to_le_bytes());
    for length in 0..bytes.len() {
        assert!(
            ChecksumManifest::decode(&bytes[..length]).is_err(),
            "length {length}"
        );
    }
    let mut unsupported = b"ZXSCHK2\0".to_vec();
    unsupported.extend(2u16.to_le_bytes());
    unsupported.extend_from_slice(&bytes[8..]);
    assert_eq!(
        ChecksumManifest::decode(&unsupported).unwrap_err().code(),
        crate::ErrorCode::ChecksumFormatInvalid
    );
    let mut wrong_algorithm = bytes.clone();
    wrong_algorithm[8..10].copy_from_slice(&2u16.to_le_bytes());
    assert!(ChecksumManifest::decode(&wrong_algorithm).is_err());
    assert_eq!(
        ChecksumManifest::decode(&bytes).unwrap().encode().unwrap(),
        bytes
    );
}

#[test]
fn checksum_decoder_rejects_count_overflow_extensions_and_unsupported_chunks() {
    let base = ChecksumManifest {
        chunk_bytes: 65536,
        logical_size: 0,
        chunks: vec![],
    };
    assert_eq!(
        ChecksumManifest::decode(&base.encode().unwrap())
            .unwrap()
            .chunk_bytes,
        65536
    );
    let mut corrupt = base.encode().unwrap();
    corrupt[22..30].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(ChecksumManifest::decode(&corrupt).is_err());
    let mut extension = base.encode().unwrap();
    extension.push(0);
    assert!(ChecksumManifest::decode(&extension).is_err());
    assert!(
        ChecksumManifest {
            chunk_bytes: 16777216,
            ..base
        }
        .encode()
        .is_err()
    );
}

#[test]
fn borrowed_value_encoding_matches_generic_bytes_and_hashes() {
    let values = [
        json_codec::json!({"\u{e000}": 2, "\u{10000}": 1}),
        json_codec::json!({"escaped": "\"\\\n\r\t\u{0000}/", "nested": [true, null, {"x": []}]}),
        json_codec::json!([-9_007_199_254_740_991i64, 0, 9_007_199_254_740_991u64]),
    ];
    assert_eq!(
        canonical_json_value(&values[0]).unwrap(),
        "{\"\u{10000}\":1,\"\u{e000}\":2}".as_bytes()
    );
    for value in values {
        assert_eq!(
            canonical_json_value(&value).unwrap(),
            canonical_json(&value).unwrap()
        );
        for kind in ["state-page", "history-manifest"] {
            assert_eq!(
                encode_hashed_value(kind, &value).unwrap(),
                encode_hashed_object(kind, &value).unwrap()
            );
            assert_eq!(
                object_hash_value(kind, &value).unwrap(),
                object_hash(kind, &value).unwrap()
            );
        }
    }
}

#[test]
fn borrowed_value_plan_encoding_preserves_frozen_inputs_and_digests() {
    let check = |input: &Value, expected: &str| {
        // The expected digest is bound to literal bytes, not either encoder.
        let mut hash = blake3::Hasher::new();
        hash.update(b"zxs:plan\0");
        hash.update(expected.as_bytes());
        let expected_hash = hash.finalize().to_hex().to_string();
        assert_eq!(baseline::canonical(input).unwrap(), expected.as_bytes());
        assert_eq!(canonical_json(input).unwrap(), expected.as_bytes());
        assert_eq!(canonical_json_value(input).unwrap(), expected.as_bytes());
        assert_eq!(object_hash("plan", input).unwrap(), expected_hash);
        assert_eq!(object_hash_value("plan", input).unwrap(), expected_hash);
        expected_hash
    };
    let purge = json_codec::json!({
        "history_tree_id": "history-a",
        "version_ids": ["version-a", "version-b"]
    });
    let purge_hash = check(
        &purge,
        r#"{"history_tree_id":"history-a","version_ids":["version-a","version-b"]}"#,
    );
    check(
        &json_codec::json!({"history_tree_id":"history-a", "cut_commit_id":"commit-a"}),
        r#"{"cut_commit_id":"commit-a","history_tree_id":"history-a"}"#,
    );
    let historical = json_codec::json!({
        "history_tree_id": "history-a", "source_commit_id": "commit-a",
        "name": "历史 É分支", "workspace_id": "ws-a",
        "sealed_manifest_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "workspace_fencing_token": "7", "expected_topology_generation": "9",
        "primary_slot_id": null, "target_copies": null,
        "branch_creation": "placeholder-until-write", "placeholder_files": 2,
        "full_copy_bytes": "21", "full_copy_files": 1,
        "required_bytes_by_slot": {"P02":"21", "P01":"21"}
    });
    let historical_bytes = concat!(
        r#"{"branch_creation":"placeholder-until-write","expected_topology_generation":"9","#,
        r#""full_copy_bytes":"21","full_copy_files":1,"history_tree_id":"history-a","#,
        r#""name":"历史 É分支","placeholder_files":2,"primary_slot_id":null,"#,
        r#""required_bytes_by_slot":{"P01":"21","P02":"21"},"#,
        r#""sealed_manifest_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","#,
        r#""source_commit_id":"commit-a","target_copies":null,"#,
        r#""workspace_fencing_token":"7","workspace_id":"ws-a"}"#,
    );
    let historical_hash = check(&historical, historical_bytes);
    let mut omitted = historical;
    omitted.as_object_mut().unwrap().remove("primary_slot_id");
    assert_ne!(
        check(
            &omitted,
            &historical_bytes.replace("\"primary_slot_id\":null,", "")
        ),
        historical_hash,
    );

    let mut reversed = purge.clone();
    reversed["version_ids"].as_array_mut().unwrap().reverse();
    assert_ne!(
        check(
            &reversed,
            r#"{"history_tree_id":"history-a","version_ids":["version-b","version-a"]}"#,
        ),
        purge_hash,
    );
    let mut extended = purge;
    extended["preview_extra"] = json_codec::json!({"nested":[null, true, {"label":"附加"}]});
    assert_ne!(
        check(
            &extended,
            concat!(
                r#"{"history_tree_id":"history-a","preview_extra":{"nested":[null,true,{"label":"附加"}]},"#,
                r#""version_ids":["version-a","version-b"]}"#,
            ),
        ),
        purge_hash,
    );
}

#[test]
fn borrowed_value_encoding_preserves_validation_errors_and_domain_priority() {
    fn sources(error: &dyn std::error::Error) -> Vec<String> {
        let mut messages = Vec::new();
        let mut source = error.source();
        while let Some(error) = source {
            messages.push(error.to_string());
            source = error.source();
        }
        messages
    }
    for value in [
        json_codec::json!({"x": [1.5]}),
        json_codec::json!({"x": [-0.0]}),
        json_codec::json!({"x": u64::MAX}),
        json_codec::json!({"x": i64::MIN}),
    ] {
        let old = canonical_json(&value).unwrap_err();
        let new = canonical_json_value(&value).unwrap_err();
        assert_eq!((&new.code(), &new.message()), (&old.code(), &old.message()));
        for kind in ["state-page", "plan", "", "Invalid", "bad_domain"] {
            let old = encode_hashed_object(kind, &value).unwrap_err();
            let new = encode_hashed_value(kind, &value).unwrap_err();
            assert_eq!((&new.code(), &new.message()), (&old.code(), &old.message()));
            assert_eq!(new.is_retryable(), old.is_retryable());
            assert_eq!(sources(&new), sources(&old));
            let hash_error = object_hash_value(kind, &value).unwrap_err();
            assert_eq!(
                (&hash_error.code(), &hash_error.message()),
                (&old.code(), &old.message())
            );
            assert_eq!(hash_error.is_retryable(), old.is_retryable());
            assert_eq!(sources(&hash_error), sources(&old));
            if !matches!(kind, "state-page" | "plan") {
                assert_eq!(new.code(), crate::ErrorCode::InvalidObjectType);
            }
        }
    }
}

#[test]
fn generic_encoding_keeps_original_serializer_call_count() {
    struct Counted<'a>(&'a std::cell::Cell<usize>);
    impl Serialize for Counted<'_> {
        fn serialize<S: data_serialization::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            self.0.set(self.0.get() + 1);
            serializer.serialize_u64(7)
        }
    }
    let count = std::cell::Cell::new(0);
    assert_eq!(canonical_json(&Counted(&count)).unwrap(), b"7");
    assert_eq!(count.get(), 2);
    count.set(0);
    encode_hashed_object("state-page", &Counted(&count)).unwrap();
    assert_eq!(count.get(), 2);
    count.set(0);
    assert!(encode_hashed_object("Invalid", &Counted(&count)).is_err());
    assert_eq!(count.get(), 0);
}

#[test]
fn typed_operation_id_preserves_request_envelope_digest() {
    let raw = "op_0123456789abcdef0123456789abcdef";
    let id: crate::OperationId = raw.parse().unwrap();
    let legacy = json_codec::json!({"operation_id":raw,"message":"unchanged"});
    let typed = json_codec::json!({"operation_id":id,"message":"unchanged"});
    assert_eq!(
        canonical_json(&typed).unwrap(),
        canonical_json(&legacy).unwrap()
    );
    let digest = |body: &json_codec::Value| {
        request_digest(
            "POST",
            "/api/workspaces/{workspace}/commit",
            json_codec::json!({"workspace":"workspace"}),
            json_codec::json!({}),
            body,
        )
        .unwrap()
    };
    assert_eq!(digest(&typed), digest(&legacy));
}

mod baseline;
mod number_emission;
mod object_ordering;
