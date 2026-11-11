use crate::{ErrorCode, Principal, Sequence, Timestamp, TimestampError, canonical_json};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error as _,
    time::{Duration, SystemTimeError, UNIX_EPOCH},
};

#[test]
fn timestamp_preserves_full_width_unsigned_seconds() {
    for value in [
        0,
        1,
        253_402_300_800,
        (1 << 53) - 1,
        1 << 53,
        u64::MAX,
    ] {
        let timestamp = Timestamp::from_unix_seconds(value);
        let text = value.to_string();
        let bytes = format!("\"{text}\"");
        assert_eq!(timestamp.unix_seconds(), value);
        assert_eq!(timestamp.to_string(), text);
        assert_eq!(text.parse::<Timestamp>().unwrap(), timestamp);
        assert_eq!(
            json_codec::from_str::<Timestamp>(&bytes).unwrap(),
            timestamp
        );
        assert_eq!(canonical_json(&timestamp).unwrap(), bytes.as_bytes());
    }
}

#[test]
fn timestamp_parser_requires_canonical_unsigned_decimal_strings() {
    for text in [
        "",
        "00",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "1\n",
        "1.0",
        "1e0",
        "１８",
        "18446744073709551616",
        "100000000000000000000",
        "2026-10-20T00:00:00Z",
    ] {
        assert_eq!(
            text.parse::<Timestamp>().unwrap_err().code(),
            ErrorCode::InvalidInteger
        );
        let bytes = json_codec::to_string(text).unwrap();
        assert!(json_codec::from_str::<Timestamp>(&bytes).is_err());
    }
    for bytes in ["0", "1", "-1", "1.0", "true", "null", "[]", "{}"] {
        assert!(json_codec::from_str::<Timestamp>(bytes).is_err());
    }
}

#[test]
fn system_time_conversion_preserves_whole_second_semantics() {
    // Reuse the existing business clock boundary values.
    for (elapsed, expected) in [
        (Duration::ZERO, 0),
        (Duration::new(0, 999_999_999), 0),
        (Duration::new(42, 999_999_999), 42),
        (Duration::from_secs(253_402_300_800), 253_402_300_800),
    ] {
        assert_eq!(
            Timestamp::try_from(UNIX_EPOCH + elapsed).unwrap(),
            Timestamp::from_unix_seconds(expected)
        );
    }
    let error = Timestamp::try_from(UNIX_EPOCH - Duration::from_nanos(1)).unwrap_err();
    let TimestampError::BeforeUnixEpoch { source } = &error;
    assert_eq!(source.duration(), Duration::from_nanos(1));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<SystemTimeError>()
            .unwrap()
            .duration(),
        Duration::from_nanos(1)
    );
}

#[derive(Serialize, Deserialize)]
struct FileDeletion {
    author: Principal,
    sequence: Sequence,
    created_at: Timestamp,
    #[serialization(flatten)]
    fields: BTreeMap<String, json_codec::Value>,
}

#[test]
fn published_file_deletion_vector_preserves_audit_scalar_bytes() {
    // docs/contracts/vectors.json: positive/file-deletion-bodyless-frontier.
    let bytes = concat!(
        r#"{"author":"admin","branch_heads":{"#,
        r#""br_00000000000000000000000000000001":"ver_00000000000000000000000000000001","#,
        r#""br_00000000000000000000000000000002":"fmeta_00000000000000000000000000000002"},"#,
        r#""created_at":"1794000000","file_id":"file_00000000000000000000000000000001","#,
        r#""history_tree_id":"history_00000000000000000000000000000001","#,
        r#""node_id":"fdel_00000000000000000000000000000001","#,
        r#""operation_id":"op_00000000000000000000000000000001","parent_node_ids":["#,
        r#""fmeta_00000000000000000000000000000002","ver_00000000000000000000000000000001"],"#,
        r#""path":"A.txt","repository_id":"repo_00000000000000000000000000000001","#,
        r#""sequence":"3"}"#,
    );
    let deletion: FileDeletion = json_codec::from_str(bytes).unwrap();
    assert_eq!(deletion.author.as_str(), "admin");
    assert_eq!(deletion.sequence, Sequence::new(3));
    assert_eq!(
        deletion.created_at,
        Timestamp::from_unix_seconds(1_794_000_000)
    );
    assert_eq!(canonical_json(&deletion).unwrap(), bytes.as_bytes());
}
