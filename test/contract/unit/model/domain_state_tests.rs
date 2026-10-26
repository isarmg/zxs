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
