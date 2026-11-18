use super::*;

#[test]
fn ordinary_placement_fixture_preserves_file_versions_and_opaque_zip_bytes() {
    let source = tempfile::tempdir().unwrap();
    crate::fixture::initialize_management(&source, "Placement fixture password 2026!").unwrap();
    let fixture = Fixture::new(source.path(), &source.path().join("pool")).unwrap();
    let engine = Engine::open(&fixture.pool).unwrap();
    assert!(fixture.repository.file_manager_root);
    let entries = engine.list_tree(&fixture.repository.main_branch_id).unwrap();
    assert_eq!(entries.iter().find(|entry| entry.entry_id == fixture.future_directory).unwrap().path(), "future");
    let zip = entries.iter().find(|entry| entry.path() == "future/project.zxs").unwrap();
    let mut original = engine.open_content(&zip.version_id).unwrap();
    let mut bytes = Vec::new();
    original.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len(), 22);
    assert_eq!(&bytes[..4], b"PK\x05\x06");
    assert!(bytes[4..].iter().all(|byte| *byte == 0));
    assert_eq!(fixture.originals().len(), 2);
    assert_eq!(engine.state().versions.len(), fixture.originals.len());
    fixture.preserved(&engine).unwrap();
}
