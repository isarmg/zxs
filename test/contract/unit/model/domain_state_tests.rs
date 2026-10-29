use super::*;

#[test]
fn plan_kinds_preserve_fixed_canonical_bytes_and_reject_unknown_tokens() {
    for (kind, token) in [
        (PlanKind::Purge, "purge"),
        (PlanKind::Split, "split"),
        (PlanKind::HistoricalSave, "historical_save"),
    ] {
        let plan = Plan {
            plan_id: "plan_fixture".into(),
            kind,
            principal: "admin".into(),
            expected_sequence: u64::MAX,
            digest: "digest".into(),
            input: json_codec::json!({"opaque":[1,null]}),
            state: PlanState::Ready,
        };
        let expected = format!(
            "{{\"digest\":\"digest\",\"expected_sequence\":\"18446744073709551615\",\"input\":{{\"opaque\":[1,null]}},\"kind\":\"{token}\",\"plan_id\":\"plan_fixture\",\"principal\":\"admin\",\"state\":\"ready\"}}"
        );
        assert_eq!(crate::canonical_json(&plan).unwrap(), expected.as_bytes());
        assert_eq!(
            crate::decode_strict::<Plan>(expected.as_bytes()).unwrap(),
            plan
        );
        let value: json_codec::Value = json_codec::from_str(&expected).unwrap();
        assert_eq!(
            crate::object_hash("plan-fixture", &plan).unwrap(),
            crate::object_hash_value("plan-fixture", &value).unwrap()
        );
    }
    for invalid in [
        "\"unknown\"",
        "\"Purge\"",
        "null",
        "0",
        "true",
        "[]",
        "{\"purge\":null}",
    ] {
        assert!(
            json_codec::from_str::<PlanKind>(invalid).is_err(),
            "{invalid}"
        );
    }
}

macro_rules! roundtrip {
    ($ty:ident, $($variant:ident),+ $(,)?) => {
        for value in [$($ty::$variant),+] {
            let encoded = format!("\"{}\"", value.as_str());
            assert_eq!(json_codec::to_string(&value).unwrap(), encoded);
            assert_eq!(json_codec::from_str::<$ty>(&encoded).unwrap(), value);
            let object = format!("{{{}:null}}", encoded);
            assert!(json_codec::from_str::<$ty>(&object).is_err());
        }
        for invalid in ["\"unknown_state\"", "0", "null", "true", "[]"] {
            assert!(json_codec::from_str::<$ty>(invalid).is_err());
        }
    };
}

#[test]
fn domain_states_preserve_wire_strings_without_accepting_other_shapes() {
    roundtrip!(DirectoryBatchKind, Move, Copy, Delete, Rename);
    roundtrip!(BranchKind, File, Full);
    roundtrip!(FileMetadataKind, Tags, Merge);
    roundtrip!(HistoryState, Active, Archived);
    roundtrip!(BranchState, Active, Archived, Migrated, Merged);
    roundtrip!(WorkspaceState, Open, Sealed, Committed, Aborted);
    roundtrip!(WorkspacePurpose, Normal, HistoricalBranch);
    roundtrip!(PlanState, Ready, Cancelled, Committed, Completed);
    roundtrip!(NamespaceOperationKind, Mkdir, Rename, Delete, DeleteTree);
}

#[test]
fn namespace_request_kind_is_parsed_at_the_public_boundary() {
    for kind in [
        NamespaceOperationKind::Mkdir,
        NamespaceOperationKind::Rename,
        NamespaceOperationKind::Delete,
        NamespaceOperationKind::DeleteTree,
    ] {
        assert_eq!(
            kind.as_str().parse::<NamespaceOperationKind>().unwrap(),
            kind
        );
    }
    assert_eq!(
        "invalid"
            .parse::<NamespaceOperationKind>()
            .unwrap_err()
            .code(),
        crate::ErrorCode::OperationUnsupported
    );
}

#[test]
fn configuration_mode_requires_exact_strings() {
    for (mode, token) in [
        (Mode::Production, "production"),
        (Mode::Development, "development"),
    ] {
        let encoded = format!("\"{token}\"");
        assert_eq!(json_codec::to_string(&mode).unwrap(), encoded);
        assert_eq!(json_codec::from_str::<Mode>(&encoded).unwrap(), mode);
        assert!(json_codec::from_str::<Mode>(&format!("{{{encoded}:null}}")).is_err());
    }
    for invalid in ["\"unknown\"", "null", "0", "true", "[]"] {
        assert!(json_codec::from_str::<Mode>(invalid).is_err());
    }
}

#[test]
fn directory_batch_kind_preserves_request_bytes_and_digest_input() {
    for token in ["move", "copy", "delete", "rename"] {
        let source = r#"{"directory_branch_id":"branch","expected_tree_id":"tree","path":"file.txt","entry_id":"entry"}"#;
        let target = if token == "delete" {
            "null"
        } else {
            r#"{"directory_branch_id":"branch","expected_tree_id":"tree","path":"target.txt"}"#
        };
        let wire =
            format!(r#"{{"kind":"{token}","items":[{{"source":{source},"target":{target}}}]}}"#);
        let request: DirectoryNamespaceBatch = json_codec::from_str(&wire).unwrap();
        assert_eq!(json_codec::to_vec(&request).unwrap(), wire.as_bytes());
        let original: json_codec::Value = json_codec::from_str(&wire).unwrap();
        let typed = json_codec::to_value(&request).unwrap();
        assert_eq!(typed, original);
        assert_eq!(
            crate::canonical_json(&typed).unwrap(),
            crate::canonical_json(&original).unwrap()
        );
        for invalid in [
            json_codec::json!("unknown"),
            json_codec::json!({token:null}),
            json_codec::json!(null),
            json_codec::json!(0),
            json_codec::json!([]),
        ] {
            let mut invalid_request = original.clone();
            invalid_request["kind"] = invalid;
            assert!(json_codec::from_value::<DirectoryNamespaceBatch>(invalid_request).is_err());
        }
    }
}
