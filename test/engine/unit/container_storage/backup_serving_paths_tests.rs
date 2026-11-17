use super::*;
use super::super::VersionFile;
use crate::ErrorCode;
use storage_contract::{HistoryTreeId, VersionId};

#[test]
fn same_named_file_roots_are_distinct_but_history_parents_count_once() {
    let a = FileId::from_random([1; 16]);
    let b = FileId::from_random([2; 16]);
    let history = HistoryTreeId::from_random([3; 16]);
    let version = VersionId::from_random([4; 16]);
    let mut expected = Expected::new(&BTreeMap::from([(a, ()), (b, ())]), &AtomicBool::new(false)).unwrap();
    for file in [a, b] {
        expected.add(&Key::Owner { file }, Path::new(crate::container_storage::OWNER_MARKER)).unwrap();
    }
    let root = layout::source_version(history, version);
    for material in [VersionFile::Descriptor, VersionFile::Checksums] {
        expected.add(&Key::Metadata { file: a, history, version, material },
            &root.join(material.name().as_path())).unwrap();
    }
    // One original versions/ + one owner in EACH FILE. In the distinct
    // history store, three common directories and two separate regular files.
    assert_eq!(expected.entries, 9);
    assert_eq!(expected.management.len(), 5);
    assert_eq!(expected.files[&a].len(), 2);
    assert_eq!(expected.files[&b].len(), 2);
    assert!(expected.add(&Key::Database { file: b }, Path::new("versions")).is_err());
}

#[test]
fn complete_serving_geometry_and_retained_temporaries_share_the_real_entry_limit() {
    let file = FileId::from_random([1; 16]);
    let history = HistoryTreeId::from_random([2; 16]);
    let mut expected = Expected::new(&BTreeMap::from([(file, ())]), &AtomicBool::new(false)).unwrap();
    expected.add(&Key::Database { file }, Path::new(crate::container_storage::file_database::NAME)).unwrap();
    expected.add(&Key::Owner { file }, Path::new(crate::container_storage::OWNER_MARKER)).unwrap();
    for index in 1u128..=16_665 {
        let version = VersionId::from_random(index.to_be_bytes());
        expected.add(&Key::Body { file, version }, &PathBuf::from("versions").join(format!("{index}-body"))).unwrap();
        let directory = layout::source_version(history, version);
        for material in [VersionFile::Descriptor, VersionFile::Checksums,
            VersionFile::Commit, VersionFile::Publication]
        {
            expected.add(&Key::Metadata { file, history, version, material },
                &directory.join(material.name().as_path())).unwrap();
        }
    }
    // 3 fixed FILE names + 2 shared history parents + 6 new names per version.
    assert_eq!(expected.entries, 99_995);
    expected.require_room_for(5).unwrap();
    assert_eq!(expected.require_room_for(6).unwrap_err().code(), ErrorCode::ResourceLimit);
    // Six more legitimate future names alone exceed the complete 100k cap,
    // even on an entirely empty target with sufficient free bytes and FDs.
    let last = VersionId::from_random(16_666u128.to_be_bytes());
    expected.add(&Key::Body { file, version: last }, Path::new("versions/last-body")).unwrap();
    let directory = layout::source_version(history, last);
    for material in [VersionFile::Descriptor, VersionFile::Checksums, VersionFile::Commit] {
        expected.add(&Key::Metadata { file, history, version: last, material },
            &directory.join(material.name().as_path())).unwrap();
    }
    let material = VersionFile::Publication;
    assert_eq!(expected.add(&Key::Metadata { file, history, version: last, material },
        &directory.join(material.name().as_path())).unwrap_err().code(), ErrorCode::ResourceLimit);
}
