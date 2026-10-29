use super::*;
use std::{error::Error as _, fs};
use storage_contract::{
    EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, EventSegment, NAMESPACE_PAGE_KIND, NamespacePage,
};

fn empty_page() -> NamespacePage {
    NamespacePage {
        format: Default::default(),
        entries: Vec::new(),
    }
}

#[test]
fn sparse_object_over_reader_limit_is_rejected_before_body_reads() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let hash = store_object(disk, NAMESPACE_PAGE_KIND, &empty_page()).unwrap();
    let path = object_path(disk, NAMESPACE_PAGE_KIND, &hash);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_METADATA as u64 + 1)
        .unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let mut reads = Vec::new();
    let error = load_authenticated_object_observed_at(
        &root,
        disk,
        NAMESPACE_PAGE_KIND,
        &hash,
        &mut |bytes| {
            reads.push(bytes);
            Ok(())
        },
    )
    .err()
    .expect("an oversized sparse file must fail before decoding or hashing");
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.message(), "metadata input exceeds limit");
    assert!(reads.is_empty(), "file size admission precedes body reads");
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_METADATA as u64 + 1);
}

#[test]
fn observed_segment_reads_charge_exact_bytes_in_bounded_bursts() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let segment = EventSegment {
        format: Default::default(),
        events: (0..EVENT_SEGMENT_SPAN)
            .map(|ordinal| Event {
                sequence: 1,
                operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
                kind: "metadata_adversarial".into(),
                object_id: format!("fixture-{ordinal:04}"),
            })
            .collect(),
    };
    let expected = canonical_json(&segment).unwrap();
    assert!(expected.len() > 64 * 1024);
    let hash = store_object(disk, EVENT_SEGMENT_KIND, &segment).unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let mut reads = Vec::new();
    let loaded = load_authenticated_object_observed_at(
        &root,
        disk,
        EVENT_SEGMENT_KIND,
        &hash,
        &mut |bytes| {
            reads.push(bytes);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(loaded.bytes(), expected);
    assert_eq!(reads.iter().sum::<usize>(), expected.len());
    assert!(reads.iter().all(|bytes| *bytes <= 64 * 1024));
    assert!(reads.iter().filter(|bytes| **bytes != 0).count() >= 2);
    assert_eq!(
        json_codec::from_value::<EventSegment>(loaded.value().clone())
            .unwrap()
            .events
            .len(),
        EVENT_SEGMENT_SPAN
    );
}

#[test]
fn missing_reference_is_reported_without_reading_or_creating_an_empty_page() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let hash = store_object(disk, NAMESPACE_PAGE_KIND, &empty_page()).unwrap();
    let path = object_path(disk, NAMESPACE_PAGE_KIND, &hash);
    fs::remove_file(&path).unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let mut reads = Vec::new();
    let error = load_authenticated_object_observed_at(
        &root,
        disk,
        NAMESPACE_PAGE_KIND,
        &hash,
        &mut |bytes| {
            reads.push(bytes);
            Ok(())
        },
    )
    .err()
    .expect("a missing immutable reference cannot resolve to an empty page");
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert_eq!(error.message(), "immutable object is missing");
    assert!(reads.is_empty());
    assert_eq!(
        fs::metadata(&path).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn truncated_json_and_wrong_digest_keep_distinct_causes_and_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let hash = store_object(disk, NAMESPACE_PAGE_KIND, &empty_page()).unwrap();
    let path = object_path(disk, NAMESPACE_PAGE_KIND, &hash);
    let root = DiskRoot::open(&disk.root).unwrap();
    for (bytes, code) in [
        (
            &b"{\"format\":\"namespace-page\",\"entries\":["[..],
            ErrorCode::InvalidJson,
        ),
        (
            &b"{\"format\":\"namespace-page\",\"entries\":[],\"extra\":true}"[..],
            ErrorCode::ObjectCorrupt,
        ),
    ] {
        fs::write(&path, bytes).unwrap();
        let mut observed = 0;
        let error = load_authenticated_object_observed_at(
            &root,
            disk,
            NAMESPACE_PAGE_KIND,
            &hash,
            &mut |count| {
                observed += count;
                Ok(())
            },
        )
        .err()
        .expect("corrupt bytes cannot authenticate at the original reference");
        assert_eq!(error.code(), code);
        if code == ErrorCode::InvalidJson {
            assert!(matches!(
                &error,
                Error::Contract(storage_contract::Error::Json { .. })
            ));
            assert!(error.source().unwrap().is::<json_codec::Error>());
        } else {
            assert_eq!(error.message(), "immutable object digest differs");
        }
        assert_eq!(observed, bytes.len());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
