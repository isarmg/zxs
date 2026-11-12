use super::*;

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
    roundtrip!(DirectoryBatchKind, Move, Copy, Delete, Rename, Separate);
    roundtrip!(BranchKind, File, Full);
    roundtrip!(FileMetadataKind, Merge);
    assert!(json_codec::from_str::<FileMetadataKind>("\"tags\"").is_err());
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
    for token in ["move", "copy", "delete", "rename", "separate"] {
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

#[test]
fn stored_content_tokens_and_custom_diagnostics_remain_exact() {
    for (state, token) in [
        (ContentState::Available, "AVAILABLE"),
        (ContentState::Purged, "PURGED"),
        (ContentState::Migrated, "MIGRATED"),
        (ContentState::Retired, "RETIRED"),
    ] {
        let encoded = format!("\"{token}\"");
        assert_eq!(state.as_str(), token);
        assert_eq!(json_codec::to_string(&state).unwrap(), encoded);
        assert_eq!(
            json_codec::from_str::<ContentState>(&encoded).unwrap(),
            state
        );
        assert!(json_codec::from_str::<ContentState>(&format!("{{{encoded}:null}}")).is_err());
    }
    assert_eq!(
        json_codec::from_str::<ContentState>(r#""AV\u0041ILABLE""#).unwrap(),
        ContentState::Available
    );
    for invalid in ["\"available\"", "\"UNKNOWN\"", "null", "0", "true", "[]"] {
        assert!(json_codec::from_str::<ContentState>(invalid).is_err());
    }
    let content = json_codec::from_str::<ContentState>("0")
        .unwrap_err()
        .to_string();
    assert!(content.contains("expected AVAILABLE, PURGED, MIGRATED or RETIRED as a string"));
    let health = json_codec::from_str::<CurrentHealth>("0")
        .unwrap_err()
        .to_string();
    assert!(health.contains("expected a current health string"));
    let branch = json_codec::from_str::<BranchState>("0")
        .unwrap_err()
        .to_string();
    assert!(branch.contains("expected BranchState string"));
}

#[test]
fn namespace_entry_kind_tokens_and_diagnostics_remain_exact() {
    for (kind, token) in [
        (NamespaceEntryKind::File, "file"),
        (NamespaceEntryKind::Directory, "directory"),
    ] {
        let encoded = format!("\"{token}\"");
        assert_eq!(json_codec::to_string(&kind).unwrap(), encoded);
        assert_eq!(
            json_codec::from_str::<NamespaceEntryKind>(&encoded).unwrap(),
            kind
        );
        assert!(
            json_codec::from_str::<NamespaceEntryKind>(&format!("{{{encoded}:null}}")).is_err()
        );
    }
    for invalid in ["\"FILE\"", "\"unknown\"", "null", "0", "true", "[]"] {
        assert!(json_codec::from_str::<NamespaceEntryKind>(invalid).is_err());
    }
    let error = json_codec::from_str::<NamespaceEntryKind>("0")
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected file or directory as a string"));
}
