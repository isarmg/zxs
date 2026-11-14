//! Source identity is not a current display name or a body hash.
use super::*;

fn file() -> Entry {
    Entry {
        entry_id: crate::new_id("entry").unwrap(),
        file_id: crate::new_id("file").unwrap(),
        version_id: crate::new_id("ver").unwrap(),
        path: PortablePath::parse("folder/report.txt").unwrap(),
        kind: NamespaceEntryKind::File,
        size: 4,
    }
}

#[test]
fn upload_source_requires_exact_occurrence_and_body_not_equal_version_alone() {
    let original = file();
    assert!(same_source(Some(&original), Some(&original)));
    let mut another_entry = original.clone();
    another_entry.entry_id = crate::new_id("entry").unwrap();
    let mut another_container = original.clone();
    another_container.file_id = crate::new_id("file").unwrap();
    let mut another_version = original.clone();
    another_version.version_id = crate::new_id("ver").unwrap();
    let mut directory = original.clone();
    directory.kind = NamespaceEntryKind::Directory;
    for changed in [another_entry, another_container, another_version, directory] {
        assert!(!same_source(Some(&original), Some(&changed)));
        assert!(!same_source(Some(&changed), Some(&original)));
    }
}

#[test]
fn upload_source_keeps_names_and_absence_distinct_from_identity() {
    let original = file();
    let mut renamed = original.clone();
    renamed.relocate(PortablePath::parse("other/REPORT.txt").unwrap());
    assert!(same_source(Some(&original), Some(&renamed)));
    assert!(same_source(None, None));
    assert!(!same_source(Some(&original), None));
    assert!(!same_source(None, Some(&original)));
}
