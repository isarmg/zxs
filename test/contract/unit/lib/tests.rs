use super::*;
use data_serialization::{Deserialize, Serialize};
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Integers {
    #[serialization(with = "crate::u64_string")]
    value: u64,
    #[serialization(default, with = "crate::optional_u64_string")]
    optional: Option<u64>,
    #[serialization(with = "crate::u64_string_map")]
    map: std::collections::BTreeMap<String, u64>,
}
#[test]
fn full_width_wire_integers_roundtrip_without_float_or_precision_loss() {
    let value = Integers {
        value: u64::MAX,
        optional: Some(u64::MAX),
        map: std::collections::BTreeMap::from([("P01".into(), u64::MAX)]),
    };
    let json = json_codec::to_value(&value).unwrap();
    assert_eq!(json["value"], u64::MAX.to_string());
    assert_eq!(json["optional"], u64::MAX.to_string());
    assert_eq!(json["map"]["P01"], u64::MAX.to_string());
    assert_eq!(
        decode_strict::<Integers>(&json_codec::to_vec(&json).unwrap()).unwrap(),
        value
    );
    for invalid in [
        "",
        "01",
        "+1",
        "-1",
        "1 ",
        "1\n",
        "１８",
        "18446744073709551616",
    ] {
        assert!(parse_u64_decimal(invalid).is_err(), "accepted {invalid:?}");
    }
    for invalid in [
        r#"{"value":1,"map":{}}"#,
        r#"{"value":"1","optional":1,"map":{}}"#,
        r#"{"value":"1","map":{"P01":1}}"#,
        r#"{"value":"1","map":{},"unknown":true}"#,
    ] {
        assert!(decode_strict::<Integers>(invalid.as_bytes()).is_err());
    }
}

struct DecimalMap<'a>(&'a std::collections::BTreeMap<String, u64>);
impl Serialize for DecimalMap<'_> {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        crate::u64_string_map::serialize(self.0, serializer)
    }
}

#[test]
fn decimal_map_stream_preserves_wire_bytes_hashes_and_tracked_state() {
    use std::collections::BTreeMap;
    let cases = [
        BTreeMap::new(),
        BTreeMap::from([("P01".into(), u64::MAX)]),
        BTreeMap::from([
            ("".into(), 0),
            ("quote\"slash\\\n".into(), 1),
            ("é".into(), (1 << 53) - 1),
            ("e\u{301}".into(), 1 << 53),
            ("\u{e000}".into(), (1 << 53) + 1),
            ("😀".into(), u64::MAX),
        ]),
        (0..1024)
            .map(|index| (format!("scope-{index:04}"), u64::MAX - index))
            .collect(),
    ];
    for map in cases {
        // Keep the former intermediate map only as an independent wire oracle.
        let previous: BTreeMap<_, _> = map
            .iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect();
        let actual = DecimalMap(&map);
        assert_eq!(
            json_codec::to_vec(&actual).unwrap(),
            json_codec::to_vec(&previous).unwrap()
        );
        assert_eq!(
            json_codec::to_string_pretty(&actual).unwrap(),
            json_codec::to_string_pretty(&previous).unwrap()
        );
        assert_eq!(
            canonical_json(&actual).unwrap(),
            canonical_json(&previous).unwrap()
        );
        assert_eq!(
            object_hash("state-page", &actual).unwrap(),
            object_hash("state-page", &previous).unwrap()
        );
        let state = State {
            directory_selection_generations: map.clone().into(),
            file_selection_generations: map.clone().into(),
            ..State::default()
        };
        let mut expected = json_codec::to_value(State::default()).unwrap();
        if !map.is_empty() {
            let values = json_codec::to_value(&previous).unwrap();
            expected["directory_selection_generations"] = values.clone();
            expected["file_selection_generations"] = values;
        }
        let bytes = canonical_json(&state).unwrap();
        assert_eq!(bytes, canonical_json(&expected).unwrap());
        assert_eq!(decode_strict::<State>(&bytes).unwrap(), state);
    }
}

#[test]
fn decimal_map_stream_preserves_writer_failures_and_partial_bytes() {
    use std::{collections::BTreeMap, io};
    struct BoundedWriter {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl io::Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let remaining = self.limit - self.bytes.len();
            if remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture limit"));
            }
            let written = bytes.len().min(remaining);
            self.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let map = BTreeMap::from([("a".into(), 0), ("中".into(), u64::MAX)]);
    let previous: BTreeMap<_, _> = map
        .iter()
        .map(|(key, value)| (key, value.to_string()))
        .collect();
    let expected = json_codec::to_vec(&previous).unwrap();
    for limit in 0..expected.len() {
        let writer = || BoundedWriter {
            bytes: Vec::new(),
            limit,
        };
        let mut before = writer();
        let before_error = json_codec::to_writer(&mut before, &previous).unwrap_err();
        let mut after = writer();
        let after_error = json_codec::to_writer(&mut after, &DecimalMap(&map)).unwrap_err();
        assert_eq!(after.bytes, before.bytes, "limit={limit}");
        assert_eq!(after.bytes, expected[..limit], "limit={limit}");
        assert_eq!(after_error.io_error_kind(), Some(io::ErrorKind::BrokenPipe));
        assert_eq!(after_error.to_string(), before_error.to_string());
    }
}

#[test]
fn persisted_workspace_requires_explicit_purpose_without_changing_current_bytes() {
    for purpose in ["normal", "historical_branch"] {
        let bytes = format!(
            r#"{{"base_head":"commit_fixture","branch_id":"br_fixture","created_at":"1","fencing_token":"1","files":{{}},"namespace_operations":[],"principal":"admin","purpose":"{purpose}","state":"open","workspace_id":"ws_fixture"}}"#
        );
        let workspace: Workspace = decode_strict(bytes.as_bytes()).unwrap();
        assert_eq!(workspace.purpose.as_str(), purpose);
        assert_eq!(canonical_json(&workspace).unwrap(), bytes.as_bytes());
        // Empty selection maps remain legitimately omitted in the current format.
        assert!(workspace.selected_file_contexts.is_empty());
        assert!(workspace.selected_directory_branches.is_empty());
        let mut missing = json_codec::to_value(&workspace).unwrap();
        assert!(missing.as_object_mut().unwrap().remove("purpose").is_some());
        assert!(json_codec::from_value::<Workspace>(missing).is_err());
    }
}

#[test]
fn persisted_state_requires_nonsparse_fields_without_changing_current_bytes() {
    for generation in [0, u64::MAX] {
        let bytes = format!(
            r#"{{"branches":{{}},"business_generation":"{generation}","commits":{{}},"content_states":{{}},"control_auth":null,"directory_git_repositories":{{}},"events":[],"file_deletions":{{}},"histories":{{}},"history_boundaries":{{}},"merge_links":{{}},"operation_principals":{{}},"operations":{{}},"placement_rules":{{}},"placements":{{}},"plans":{{}},"protection_policies":{{}},"repositories":{{}},"sequence":"0","topology_generation":"0","versions":{{}}}}"#
        );
        let state = State {
            business_generation: generation,
            ..Default::default()
        };
        assert_eq!(canonical_json(&state).unwrap(), bytes.as_bytes());
        assert_eq!(decode_strict::<State>(bytes.as_bytes()).unwrap(), state);
        for field in [
            "business_generation",
            "directory_git_repositories",
            "file_deletions",
            "history_boundaries",
            "placement_rules",
            "protection_policies",
            "operation_principals",
        ] {
            let mut missing = json_codec::to_value(&state).unwrap();
            assert!(missing.as_object_mut().unwrap().remove(field).is_some());
            assert!(
                json_codec::from_value::<State>(missing).is_err(),
                "accepted missing {field} at generation {generation}"
            );
        }
    }
}

#[test]
fn content_state_preserves_exact_stored_tokens_and_hashes() {
    use std::collections::BTreeMap;
    let cases = [
        (ContentState::Available, "AVAILABLE"),
        (ContentState::Purged, "PURGED"),
        (ContentState::Migrated, "MIGRATED"),
        (ContentState::Retired, "RETIRED"),
    ];
    for (state, token) in cases {
        assert_eq!(state.as_str(), token);
        let expected = format!("\"{token}\"");
        assert_eq!(canonical_json(&state).unwrap(), expected.as_bytes());
        assert_eq!(
            decode_strict::<ContentState>(expected.as_bytes()).unwrap(),
            state
        );
        assert_eq!(
            json_codec::from_reader::<_, ContentState>(std::io::Cursor::new(expected.as_bytes()))
                .unwrap(),
            state
        );
    }
    let typed = cases
        .into_iter()
        .map(|(state, key)| (key.to_owned(), state))
        .collect::<BTreeMap<_, _>>();
    let strings = cases
        .into_iter()
        .map(|(_, key)| (key.to_owned(), key.to_owned()))
        .collect::<BTreeMap<_, _>>();
    let expected =
        br#"{"AVAILABLE":"AVAILABLE","MIGRATED":"MIGRATED","PURGED":"PURGED","RETIRED":"RETIRED"}"#;
    assert_eq!(canonical_json(&typed).unwrap(), expected);
    assert_eq!(
        canonical_json(&typed).unwrap(),
        canonical_json(&strings).unwrap()
    );
    assert_eq!(
        object_hash("state-page", &typed).unwrap(),
        object_hash("state-page", &strings).unwrap()
    );
    let state = State {
        content_states: typed.into(),
        ..State::default()
    };
    let mut expected_state = json_codec::to_value(State::default()).unwrap();
    expected_state["content_states"] = json_codec::to_value(strings).unwrap();
    assert_eq!(
        canonical_json(&state).unwrap(),
        canonical_json(&expected_state).unwrap()
    );
    assert_eq!(
        decode_strict::<State>(&canonical_json(&state).unwrap()).unwrap(),
        state
    );
    let empty = State::default();
    assert!(
        decode_strict::<State>(&canonical_json(&empty).unwrap())
            .unwrap()
            .content_states
            .is_empty()
    );
}

#[test]
fn content_state_rejects_unknown_tokens_and_non_string_representations() {
    for value in [
        json_codec::json!("available"),
        json_codec::json!("purged"),
        json_codec::json!("migrated"),
        json_codec::json!("LIVE"),
        json_codec::json!("PENDING"),
        json_codec::json!("UNAVAILABLE"),
        json_codec::json!("UNKNOWN"),
        json_codec::json!(""),
        json_codec::json!(null),
        json_codec::json!(0),
        json_codec::json!({"AVAILABLE":null}),
        json_codec::json!(["AVAILABLE"]),
    ] {
        assert!(
            json_codec::from_value::<ContentState>(value.clone()).is_err(),
            "accepted {value}"
        );
        let mut state = json_codec::to_value(State::default()).unwrap();
        state["content_states"] = json_codec::json!({"version":value});
        assert!(
            decode_strict::<State>(&canonical_json(&state).unwrap()).is_err(),
            "accepted {state}"
        );
    }
}

#[test]
fn namespace_entry_kind_preserves_canonical_entries_and_hashes() {
    for (kind, token, hash) in [
        (
            NamespaceEntryKind::File,
            "file",
            "7080d878b8aef06b7ab33ad6236896e7b8e53625065cfd6814abdb9f743701a1",
        ),
        (
            NamespaceEntryKind::Directory,
            "directory",
            "b9e1e328754d8ef1fe06f1bfddde0d8e93f890ace1774ff9c2066219495a44c9",
        ),
    ] {
        let expected = format!(
            r#"{{"entry_id":"entry","file_id":"","kind":"{token}","lookup_key":"a","name":"a","path":"a","size":"0","version_id":""}}"#
        );
        let entry = Entry {
            entry_id: "entry".into(),
            path: name_policy::PortablePath::parse("a").unwrap(),
            kind,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        };
        assert_eq!(canonical_json(&entry).unwrap(), expected.as_bytes());
        assert_eq!(decode_strict::<Entry>(expected.as_bytes()).unwrap(), entry);
        assert_eq!(
            json_codec::from_reader::<_, Entry>(std::io::Cursor::new(expected.as_bytes())).unwrap(),
            entry
        );
        assert_eq!(object_hash("state-page", &entry).unwrap(), hash);
        let legacy: json_codec::Value = json_codec::from_str(&expected).unwrap();
        for domain in ["state-page", "history-manifest"] {
            assert_eq!(
                object_hash(domain, &entry).unwrap(),
                object_hash(domain, &legacy).unwrap()
            );
        }
        let deletion = DeletedPath {
            path: "a".into(),
            kind,
        };
        assert_eq!(
            canonical_json(&deletion).unwrap(),
            format!(r#"{{"kind":"{token}","path":"a"}}"#).as_bytes()
        );
    }
}

#[test]
fn namespace_entry_kind_rejects_invalid_raw_kinds_at_decode_boundary() {
    for raw in [
        r#""FILE""#,
        r#""unknown""#,
        r#""""#,
        "null",
        "0",
        "true",
        r#"{"file":null}"#,
        r#"{"directory":null}"#,
        r#"["file"]"#,
    ] {
        let value: json_codec::Value = json_codec::from_str(raw).unwrap();
        assert!(
            json_codec::from_value::<NamespaceEntryKind>(value.clone()).is_err(),
            "{raw}"
        );
        let entry = json_codec::json!({"entry_id":"entry","file_id":"","kind":value,"lookup_key":"a","name":"a","path":"a","size":"0","version_id":""});
        let bytes = canonical_json(&entry).unwrap();
        assert!(decode_strict::<Entry>(&bytes).is_err(), "{raw}");
        assert!(
            json_codec::from_reader::<_, Entry>(std::io::Cursor::new(&bytes)).is_err(),
            "{raw}"
        );
        assert!(
            decode_strict::<DeletedPath>(
                &canonical_json(&json_codec::json!({"path":"a","kind":value})).unwrap()
            )
            .is_err(),
            "{raw}"
        );
    }
}

#[test]
fn namespace_operation_variants_parse_only_their_required_paths() {
    for kind in [
        NamespaceOperationKind::Mkdir,
        NamespaceOperationKind::Delete,
        NamespaceOperationKind::DeleteTree,
    ] {
        let operation = NamespaceOperation::parse(kind, "Café", None).unwrap();
        let wire = json_codec::to_value(&operation).unwrap();
        assert_eq!(wire["kind"], kind.as_str());
        assert_eq!(wire["path"], "Café");
        assert!(wire["destination"].is_null());
        assert_eq!(
            NamespaceOperation::parse(kind, "Café", Some("other"))
                .unwrap_err()
                .code(),
            ErrorCode::InvalidRequest
        );
        assert!(
            json_codec::from_value::<NamespaceOperation>(json_codec::json!({
                "kind": kind.as_str(), "path": "Café", "destination": "other"
            }))
            .is_err()
        );
    }
    assert_eq!(
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "Café", None)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
    let rename =
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "Cafe\u{301}", Some("Moved"))
            .unwrap();
    assert!(
        matches!(&rename, NamespaceOperation::Rename { source, destination }
        if source.key() == "café" && destination.as_str() == "Moved")
    );
    let wire = json_codec::to_value(&rename).unwrap();
    assert_eq!(
        wire,
        json_codec::json!({"kind":"rename", "path":"Cafe\u{301}", "destination":"Moved"})
    );
    let read = json_codec::from_value::<NamespaceOperation>(wire).unwrap();
    assert_eq!(read.key(), rename.key());
    assert_eq!(read.destination(), rename.destination());
}

#[test]
fn operation_id_preserves_exact_wire_language_and_copy_representation() {
    assert_eq!(std::mem::size_of::<OperationId>(), 16);
    fn copied(value: OperationId) -> (OperationId, OperationId) {
        (value, value)
    }
    for raw in [
        "op_00000000000000000000000000000000",
        "op_ffffffffffffffffffffffffffffffff",
        "op_0123456789abcdef0123456789abcdef",
    ] {
        let id: OperationId = raw.parse().unwrap();
        assert_eq!(copied(id), (id, id));
        assert_eq!(id.to_string(), raw);
        assert_eq!(json_codec::to_string(&id).unwrap(), format!("\"{raw}\""));
        assert_eq!(
            json_codec::from_str::<OperationId>(&format!("\"{raw}\"")).unwrap(),
            id
        );
    }
    for raw in [
        "",
        "op_",
        "op_0123456789ABCDEF0123456789abcdef",
        "op_0123456789abcdef0123456789abcdeg",
        "op_01234567-89ab-cdef-0123-456789abcdef",
        "op_000000000000000000000000000000000",
        "br_00000000000000000000000000000000",
        " op_00000000000000000000000000000000",
    ] {
        assert_eq!(
            raw.parse::<OperationId>().unwrap_err().code(),
            ErrorCode::InvalidId
        );
        assert!(json_codec::from_str::<OperationId>(&format!("\"{raw}\"")).is_err());
    }
    for raw in ["null", "1", "{}", "[]"] {
        assert!(json_codec::from_str::<OperationId>(raw).is_err());
    }
    let generated = OperationId::generate().unwrap();
    assert_eq!(
        generated.to_string().parse::<OperationId>().unwrap(),
        generated
    );
}

#[test]
#[ignore = "manual canonical operation-key formatting cost; run optimized with --nocapture"]
fn measure_operation_id_formatting_cost() {
    use std::{hint::black_box, time::Instant};
    const ITERATIONS: usize = 1_000_000;
    fn sample(mut run: impl FnMut()) -> f64 {
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            run();
        }
        start.elapsed().as_nanos() as f64 / ITERATIONS as f64
    }
    let wire = String::from("op_0123456789abcdef0123456789abcdef");
    let id: OperationId = wire.parse().unwrap();
    assert_eq!(id.to_string(), wire);
    assert_eq!(
        blake3::hash(id.to_string().as_bytes()),
        blake3::hash(wire.as_bytes())
    );
    println!(
        "OperationId bytes={}, String header bytes={}, canonical wire bytes={}, iterations/sample={ITERATIONS}",
        std::mem::size_of::<OperationId>(),
        std::mem::size_of::<String>(),
        wire.len()
    );
    for round in 0..7 {
        let format = || {
            sample(|| {
                black_box(black_box(id).to_string());
            })
        };
        let clone = || {
            sample(|| {
                black_box(String::clone(black_box(&wire)));
            })
        };
        let (format_ns, clone_ns) = if round % 2 == 0 {
            (format(), clone())
        } else {
            let cloned = clone();
            (format(), cloned)
        };
        let copy_ns = sample(|| {
            black_box(black_box(id));
        });
        let old_hash = || {
            sample(|| {
                black_box(blake3::hash(black_box(wire.as_bytes())));
            })
        };
        let typed_hash = || {
            sample(|| {
                black_box(blake3::hash(black_box(id).to_string().as_bytes()));
            })
        };
        let (old_hash_ns, typed_hash_ns) = if round % 2 == 0 {
            (old_hash(), typed_hash())
        } else {
            let typed = typed_hash();
            (old_hash(), typed)
        };
        println!(
            "round={round} format_ns={format_ns:.3} old_string_clone_ns={clone_ns:.3} typed_copy_ns={copy_ns:.3} old_borrowed_key_hash_ns={old_hash_ns:.3} typed_format_key_hash_ns={typed_hash_ns:.3}"
        );
    }
}

#[test]
fn validate_id_matches_formatted_prefix_oracle() {
    // Preserve the original public API semantics, including arbitrary caller prefixes.
    let prefixes = [
        "boundary", "br", "commit", "dbr", "disk", "dtree", "entry", "fence", "file", "fmerge",
        "fmeta", "fnode", "history", "link", "plan", "policy", "pool", "recovery", "repo", "rule",
        "ver", "ws", "", "_", "__", "file_", "fi", "文件", "é", "e\u{301}", "\0",
    ];
    let suffixes = [
        "0123456789abcdef0123456789abcdef".to_owned(),
        "0".repeat(32),
        "f".repeat(32),
        String::new(),
        "0".repeat(31),
        "0".repeat(33),
        "A".repeat(32),
        format!("{}g", "0".repeat(31)),
        format!("{}é", "0".repeat(30)),
        format!("{}０", "0".repeat(29)),
        format!("{}\0", "0".repeat(31)),
        format!("{}\n", "0".repeat(31)),
        format!("{} ", "0".repeat(31)),
    ];
    for value_prefix in prefixes {
        for separator in ["_", "", "__", "＿"] {
            for suffix in &suffixes {
                let value = format!("{value_prefix}{separator}{suffix}");
                for prefix in prefixes {
                    let expected = value.strip_prefix(&format!("{prefix}_")).is_some_and(|v| {
                        v.len() == 32
                            && v.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    });
                    let actual = validate_id(&value, prefix);
                    assert_eq!(actual.is_ok(), expected, "{value:?}, {prefix:?}");
                    if let Err(error) = actual {
                        assert_eq!(error.code(), ErrorCode::InvalidId);
                        assert_eq!(error.message(), format!("invalid {prefix} identifier"));
                        assert!(!error.is_retryable());
                    }
                }
            }
        }
    }
}

#[test]
fn entry_value_equality_matches_canonical_bytes_for_every_field() {
    let base = Entry {
        entry_id: "entry".into(),
        path: name_policy::PortablePath::parse("Folder/A.txt").unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: "version".into(),
        size: 0,
    };
    let mut cases = vec![base.clone()];
    for text in ["", "quote\"\n", "中文😀"] {
        let mut entry = base.clone();
        entry.entry_id = text.into();
        cases.push(entry);
        let mut entry = base.clone();
        entry.file_id = text.into();
        cases.push(entry);
        let mut entry = base.clone();
        entry.version_id = text.into();
        cases.push(entry);
    }
    for path in ["folder/a.txt", "Folder/B.txt", "另一目录/文件.txt"] {
        let mut entry = base.clone();
        entry.path = name_policy::PortablePath::parse(path).unwrap();
        cases.push(entry);
    }
    let mut directory = base.clone();
    directory.kind = NamespaceEntryKind::Directory;
    cases.push(directory);
    for size in [1, (1 << 53) - 1, 1 << 53, u64::MAX] {
        let mut entry = base.clone();
        entry.size = size;
        cases.push(entry);
    }
    for left in &cases {
        let left_bytes = canonical_json(left).unwrap();
        assert_eq!(decode_strict::<Entry>(&left_bytes).unwrap(), *left);
        for right in &cases {
            assert_eq!(left == right, left_bytes == canonical_json(right).unwrap());
        }
    }
    let ordered = vec![cases[0].clone(), cases[1].clone()];
    let reversed = vec![cases[1].clone(), cases[0].clone()];
    assert_ne!(ordered, reversed);
    assert_ne!(
        canonical_json(&ordered).unwrap(),
        canonical_json(&reversed).unwrap()
    );
}

#[test]
fn disk_observation_issue_and_nullable_status_are_strict_current_wire() {
    let fixture: json_codec::Value = json_codec::from_str(include_str!(
        "../../../server/fixtures/offline-admin-disks.json"
    ))
    .unwrap();
    let status = fixture["disks"][1].clone();
    let parsed: DiskStatus = json_codec::from_value(status.clone()).unwrap();
    assert_eq!(parsed.available_bytes, None);
    assert_eq!(
        parsed.observation_failure,
        Some(DiskObservationIssue::Seal(ErrorCode::IoError))
    );
    for field in ["available_bytes", "observation_failure"] {
        let mut invalid = status.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<DiskStatus>(invalid).is_err());
    }
    for issue in [
        json_codec::json!({"stage":"unknown","code":"IO_ERROR"}),
        json_codec::json!({"stage":"seal","code":"UNKNOWN_CODE"}),
        json_codec::json!({"stage":"seal","code":"IO_ERROR","source":"private"}),
    ] {
        assert!(json_codec::from_value::<DiskObservationIssue>(issue).is_err());
    }
}

#[test]
fn identifier_entropy_success_retains_v4_variant_and_canonical_format() {
    let generated = new_id("entry").unwrap();
    validate_id(&generated, "entry").unwrap();
    assert_eq!(&generated[18..19], "4");
    assert!(matches!(
        generated.as_bytes()[22],
        b'8' | b'9' | b'a' | b'b'
    ));
    let operation = OperationId::generate().unwrap().to_string();
    validate_id(&operation, "op").unwrap();
    assert_eq!(&operation[15..16], "4");
    assert!(matches!(
        operation.as_bytes()[19],
        b'8' | b'9' | b'a' | b'b'
    ));
}

#[test]
#[ignore = "isolated getrandom failure; run test/contract/entropy/entropy-io.sh"]
fn entropy_fault_returns_typed_failure_without_panic_or_identity() {
    use std::error::Error as _;
    let marker = std::env::var_os("ZXS_TEST_ENTROPY_FAIL_FILE").expect("fault driver marker");
    assert!(new_id("entry").is_ok());
    std::fs::write(&marker, b"fail").unwrap();
    for error in [
        new_id("entry").unwrap_err(),
        OperationId::generate().unwrap_err(),
    ] {
        assert!(
            matches!(error, Error::IdentifierGeneration { source, .. } if source.raw_os_error() == Some(5))
        );
        assert!(error.source().is_some());
    }
    assert!(matches!(
        Config::development(["/unused".into()]).unwrap_err(),
        Error::IdentifierGeneration { .. }
    ));
    std::fs::remove_file(marker).unwrap();
    assert!(new_id("entry").is_ok());
    println!("\nZXS_ENTROPY_FAULT_COMPLETE");
}

mod borrowed_json;
