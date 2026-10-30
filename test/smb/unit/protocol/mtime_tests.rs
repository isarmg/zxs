use super::*;

#[test]
fn saved_mtime_requires_canonical_decimal_utc_seconds_in_range() {
    for seconds in [0, 1, 2_147_483_648, SavedMtime::MAX_UNIX_SECONDS] {
        let time = SavedMtime::from_unix_seconds(seconds).unwrap();
        let encoded = json_codec::to_string(&time).unwrap();
        assert_eq!(encoded, format!("\"{seconds}\""));
        assert_eq!(json_codec::from_str::<SavedMtime>(&encoded).unwrap(), time);
    }
    for invalid in [
        "0",
        "null",
        "-1",
        "\"-1\"",
        "\"01\"",
        "\"+1\"",
        "\"1.0\"",
        "\"1e3\"",
        "\" 1\"",
        "\"253402300800\"",
        "\"18446744073709551615\"",
    ] {
        assert!(
            json_codec::from_str::<SavedMtime>(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(SavedMtime::from_unix_seconds(u64::MAX).is_err());
}

#[test]
fn mtime_metadata_rejects_missing_unknown_duplicate_and_old_wire_fields() {
    let valid = json_codec::json!({
        "entry_id":"entry", "kind":"file", "path":"entry", "handle":"1",
        "size":"0", "max_file_bytes":"1024", "version_id":"version", "workspace_id":null,
        "mtime":{"state":"saved","unix_seconds":"1600000000"}
    });
    let opened: OpenResult = json_codec::from_value(valid.clone()).unwrap();
    assert_eq!(
        opened.mtime,
        FileMtime::Saved {
            unix_seconds: SavedMtime::from_unix_seconds(1_600_000_000).unwrap()
        }
    );
    let mut missing = valid;
    missing.as_object_mut().unwrap().remove("mtime");
    assert!(json_codec::from_value::<OpenResult>(missing).is_err());
    for invalid in [
        r#"{}"#,
        r#"{"state":"saved"}"#,
        r#"{"state":"saved","unix_seconds":1}"#,
        r#"{"state":"saved","unix_seconds":"1","extra":true}"#,
        r#"{"state":"saved","unix_seconds":"1","unix_seconds":"2"}"#,
        r#"{"state":"unpublished","unix_seconds":"1"}"#,
        r#"{"state":"not_file","extra":true}"#,
        r#"{"state":"future"}"#,
    ] {
        assert!(
            json_codec::from_str::<FileMtime>(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(json_codec::from_str::<AuthorizedMetadata>(r#"{"authorized":true}"#).is_err());
    assert!(
        json_codec::from_str::<StatResult>(
            r#"{"entry_id":"e","kind":"file","path":"e","name":"e","size":"0"}"#
        )
        .is_err()
    );
    assert!(REQUIRED_CAPABILITIES.contains(&VERSION_MTIME_CAPABILITY));
}
