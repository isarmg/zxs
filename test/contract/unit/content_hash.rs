use crate::{ContentHash, FileVersion, canonical_json};

const ABC: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";

#[test]
fn content_hash_retains_fixed_body_digest_and_text_bytes() {
    let hash = ContentHash::from_digest(blake3::hash(b"abc"));
    assert_eq!(std::mem::size_of::<ContentHash>(), 32);
    assert_eq!(hash.to_string(), ABC);
    assert_eq!(hash.digest(), blake3::hash(b"abc"));
    assert_eq!(ABC.parse::<ContentHash>().unwrap(), hash);
    let expected = format!("\"{ABC}\"");
    assert_eq!(json_codec::to_vec(&hash).unwrap(), expected.as_bytes());
    assert_eq!(canonical_json(&hash).unwrap(), expected.as_bytes());
    assert_eq!(
        json_codec::from_str::<ContentHash>(&expected).unwrap(),
        hash
    );
    for text in ["0".repeat(64), "f".repeat(64)] {
        assert_eq!(text.parse::<ContentHash>().unwrap().to_string(), text);
    }
}

#[test]
fn content_hash_rejects_noncanonical_external_text_and_json() {
    for value in [
        String::new(),
        "0".repeat(63),
        "0".repeat(65),
        ABC.to_uppercase(),
        format!(" {ABC}"),
        format!("{ABC}\n"),
        "g".repeat(64),
        "é".repeat(32),
        format!("0x{ABC}"),
    ] {
        assert!(value.parse::<ContentHash>().is_err(), "{value:?}");
        assert!(json_codec::from_value::<ContentHash>(json_codec::json!(value)).is_err());
    }
    for value in [
        json_codec::json!(null),
        json_codec::json!(0),
        json_codec::json!([]),
    ] {
        assert!(json_codec::from_value::<ContentHash>(value).is_err());
    }
}

#[test]
fn file_version_disk_record_parses_content_hash_without_changing_canonical_bytes() {
    let record = json_codec::json!({
        "history_tree_id":"history", "version_id":"version", "file_id":"file",
        "creation_primary_slot_id":"P01", "logical_size":"3", "version_sequence":"1",
        "content_hash":ABC, "checksum_chunk_bytes":65536, "checksum_manifest_hash":"a".repeat(64),
        "creation_name":"body.txt", "storage_name":"v000001_20261026T000000Z_6437b3ac.txt",
        "created_at":"2026-10-26T00:00:00Z", "source_version_ids":[], "created_as_full_checkpoint":true
    });
    let bytes = canonical_json(&record).unwrap();
    let parsed: FileVersion = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(
        parsed.content_hash,
        ContentHash::from_digest(blake3::hash(b"abc"))
    );
    assert_eq!(canonical_json(&parsed).unwrap(), bytes);
    for bad in ["".to_string(), ABC.to_uppercase(), "0".repeat(63)] {
        let mut damaged = record.clone();
        damaged["content_hash"] = json_codec::json!(bad);
        assert!(json_codec::from_value::<FileVersion>(damaged).is_err());
    }
}
