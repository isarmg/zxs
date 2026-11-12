use crate::{
    Error, ErrorCode, FileNodeId, Id, IdKind, OperationId, OperationResult, canonical_json, kind,
};
use std::collections::{BTreeMap, HashSet};

fn byte_samples() -> impl Iterator<Item = [u8; 16]> {
    (0..16)
        .flat_map(|position| {
            (u8::MIN..=u8::MAX).map(move |value| {
                let mut bytes = [0; 16];
                bytes[position] = value;
                bytes
            })
        })
        .chain(std::iter::once([u8::MAX; 16]))
}

fn assert_single_prefix<K: IdKind>(prefix: &str) {
    for hex in [
        "00000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffff",
        "0123456789abcdef0123456789abcdef",
    ] {
        let wire = format!("{prefix}_{hex}");
        let id: Id<K> = wire.parse().unwrap();
        let json = format!("\"{wire}\"");
        assert_eq!(id.to_string(), wire);
        assert_eq!(json_codec::to_string(&id).unwrap(), json);
        assert_eq!(json_codec::from_str::<Id<K>>(&json).unwrap(), id);
    }
    assert_eq!(std::mem::size_of::<Id<K>>(), 16);
    let typed: BTreeMap<_, _> = byte_samples()
        .enumerate()
        .map(|(index, bytes)| (Id::<K>::from_random(bytes), index))
        .collect();
    let text: BTreeMap<_, _> = typed
        .iter()
        .rev()
        .map(|(id, index)| (id.to_string(), *index))
        .collect();
    assert_eq!(
        typed.keys().map(ToString::to_string).collect::<Vec<_>>(),
        text.keys().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        json_codec::to_vec(&typed).unwrap(),
        json_codec::to_vec(&text).unwrap()
    );
    assert_eq!(
        json_codec::from_slice::<BTreeMap<Id<K>, usize>>(&json_codec::to_vec(&text).unwrap())
            .unwrap(),
        typed
    );
}

#[test]
fn single_prefix_domains_preserve_text_and_map_iteration_order() {
    assert_single_prefix::<kind::Operation>("op");
    assert_single_prefix::<kind::Rule>("rule");
    assert_single_prefix::<kind::Repository>("repo");
    assert_single_prefix::<kind::HistoryTree>("history");
    assert_single_prefix::<kind::Branch>("br");
    assert_single_prefix::<kind::Commit>("commit");
    assert_single_prefix::<kind::Tree>("tree");
    assert_single_prefix::<kind::DirectoryTree>("dtree");
    assert_single_prefix::<kind::DirectoryBranch>("dbr");
    assert_single_prefix::<kind::Entry>("entry");
    assert_single_prefix::<kind::File>("file");
    assert_single_prefix::<kind::Version>("ver");
    assert_single_prefix::<kind::Pool>("pool");
    assert_single_prefix::<kind::Disk>("disk");
    assert_single_prefix::<kind::FileAnchor>("fnode");
    assert_single_prefix::<kind::FileMetadata>("fmeta");
    assert_single_prefix::<kind::FileDeletion>("fdel");
    assert_single_prefix::<kind::FileMerge>("fmerge");
}

#[test]
fn random_bytes_keep_browser_identity_bits() {
    for (bytes, wire) in [
        ([0; 16], "op_00000000000000000000000000000000"),
        ([u8::MAX; 16], "op_ffffffffffffffffffffffffffffffff"),
        ([0x5a; 16], "op_5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"),
    ] {
        let id = OperationId::from_random(bytes);
        assert_eq!(id.to_string(), wire);
        assert_eq!(wire.parse::<OperationId>().unwrap(), id);
    }
}

#[test]
fn identity_traits_do_not_require_traits_on_the_kind() {
    enum Marker {}
    impl IdKind for Marker {
        const PREFIX: &'static str = "marker";
    }
    fn copies<K: IdKind>(id: Id<K>) -> (Id<K>, Id<K>) {
        (id, id)
    }
    fn traits<T: Clone + Copy + std::fmt::Debug + Eq + Ord + std::hash::Hash>() {}
    fn thread_safe<T: Send + Sync>() {}

    let id = Id::<Marker>::from_random([0x2a; 16]);
    let (left, right) = copies(id);
    assert_eq!(left, right);
    traits::<Id<Marker>>();
    assert_eq!(HashSet::from([left, right]).len(), 1);
    assert_eq!(BTreeMap::from([(left, 1), (right, 2)])[&id], 2);
    assert!(!format!("{id:?}").is_empty());
    thread_safe::<Id<Marker>>();
    assert_single_prefix::<Marker>("marker");
}

fn file_nodes(bytes: [u8; 16]) -> [FileNodeId; 4] {
    [
        FileNodeId::Version(Id::from_random(bytes)),
        FileNodeId::Anchor(Id::from_random(bytes)),
        FileNodeId::Metadata(Id::from_random(bytes)),
        FileNodeId::Merge(Id::from_random(bytes)),
    ]
}

#[test]
fn file_node_variants_preserve_prefixes_and_scalar_encoding() {
    let nodes = file_nodes([0; 16]);
    for (id, prefix) in nodes.into_iter().zip(["ver", "fnode", "fmeta", "fmerge"]) {
        let wire = format!("{prefix}_00000000000000000000000000000000");
        let json = format!("\"{wire}\"");
        assert_eq!(id.to_string(), wire);
        assert_eq!(wire.parse::<FileNodeId>().unwrap(), id);
        assert_eq!(json_codec::to_string(&id).unwrap(), json);
        assert_eq!(json_codec::from_str::<FileNodeId>(&json).unwrap(), id);
    }
}

#[test]
fn file_node_map_iteration_matches_text_order_across_variants_and_bytes() {
    let typed: BTreeMap<_, _> = byte_samples()
        .flat_map(file_nodes)
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let text: BTreeMap<_, _> = typed
        .iter()
        .rev()
        .map(|(id, index)| (id.to_string(), *index))
        .collect();
    assert_eq!(
        typed.keys().map(ToString::to_string).collect::<Vec<_>>(),
        text.keys().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        json_codec::to_vec(&typed).unwrap(),
        json_codec::to_vec(&text).unwrap()
    );
    assert_eq!(
        json_codec::from_slice::<BTreeMap<FileNodeId, usize>>(&json_codec::to_vec(&text).unwrap())
            .unwrap(),
        typed
    );
}

#[test]
fn file_node_parser_rejects_other_domains_and_noncanonical_suffixes() {
    for prefix in ["ver", "fnode", "fmeta", "fmerge"] {
        for suffix in [
            "",
            "0000000000000000000000000000000",
            "000000000000000000000000000000000",
            "0123456789ABCDEF0123456789abcdef",
            "0123456789abcdef0123456789abcdeg",
            "01234567-89ab-cdef-0123-456789abcdef",
            "０００００００００００００００００００００００００００００００００００",
        ] {
            let wire = format!("{prefix}_{suffix}");
            assert_eq!(
                wire.parse::<FileNodeId>().unwrap_err().code(),
                ErrorCode::InvalidId
            );
            assert!(json_codec::from_str::<FileNodeId>(&format!("\"{wire}\"")).is_err());
        }
    }
    for wire in [
        "",
        "fnode",
        "fdel_00000000000000000000000000000000",
        "op_00000000000000000000000000000000",
        "file_00000000000000000000000000000000",
        " fnode_00000000000000000000000000000000",
        "fnode_00000000000000000000000000000000 ",
    ] {
        assert_eq!(
            wire.parse::<FileNodeId>().unwrap_err().code(),
            ErrorCode::InvalidId
        );
        assert!(json_codec::from_str::<FileNodeId>(&format!("\"{wire}\"")).is_err());
    }
    for wire in ["null", "1", "{}", "[]", "true"] {
        assert!(json_codec::from_str::<FileNodeId>(wire).is_err());
    }
}

#[test]
fn operation_parse_retains_contract_error_and_retryability_api() {
    fn parse(value: &str) -> crate::Result<OperationId> {
        value.parse()
    }
    let error = parse("bad").unwrap_err();
    assert!(matches!(
        error,
        Error::InvalidIdentifier { prefix: "op", .. }
    ));
    assert_eq!(error.code(), ErrorCode::InvalidId);
    assert!(!error.is_retryable());
    let retryable = error.retryable();
    assert_eq!(retryable.code(), ErrorCode::InvalidId);
    assert!(retryable.is_retryable());
}

#[test]
fn operation_receipt_fixed_vector_preserves_field_and_map_key_bytes() {
    // The tracked-receipt vector contains the same identity as both key and field.
    let bytes: &[u8] = include_bytes!("../fixtures/operation-receipts.json");
    let receipts: BTreeMap<OperationId, OperationResult> = json_codec::from_slice(bytes).unwrap();
    assert_eq!(canonical_json(&receipts).unwrap(), bytes);
    for (id, receipt) in receipts {
        assert_eq!(id, receipt.operation_id);
    }
}
